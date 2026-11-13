//! The inbox queue: persistence, filtering, coalescing, eviction, and announcement order
//! (REQ-INB-004..007, 012, 014, 016).

use super::adapter::Mapped;
use super::endpoint::{Endpoint, Endpoints, QuietHours};
use super::event::{Event, EventKind, EventState, Priority, Rel, Timing};
use super::{Moment, Restore, saved_at};
use crate::sched::{Clock, Ts};
use crate::store::{JsonStore, StoreError};
use serde::{Deserialize, Serialize};

/// Schema version of the persisted inbox.
pub const SCHEMA: u32 = 1;

/// Hard bounds so a flood of events can never exhaust memory or storage (REQ-NFR-110).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InboxLimits {
    /// Most events kept.
    pub max_events: usize,
    /// Most bytes of event text kept.
    pub max_bytes: usize,
}

impl Default for InboxLimits {
    fn default() -> Self {
        Self {
            max_events: 64,
            max_bytes: 256 * 1024,
        }
    }
}

/// Why an event was not queued.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterReason {
    /// The endpoint is muted.
    Muted,
    /// Below the endpoint's minimum priority.
    BelowPriority,
    /// The endpoint ignores this kind.
    IgnoredKind,
}

/// What happened to an incoming event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ingest {
    /// Queued as a new event.
    Accepted(u64),
    /// Merged into a recent unannounced event from the same session.
    Coalesced(u64),
    /// Dropped by the endpoint's filter.
    Filtered(FilterReason),
}

/// The serialisable part of the inbox.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxState {
    next_id: u64,
    events: Vec<Event>,
    /// The last value given to an event's `spoken_seq`.
    #[serde(default)]
    spoken_seq: u64,
    /// The wall time of the save, when the clock was valid then (see `Restore`). Read at load only.
    #[serde(default, skip_serializing)]
    saved_at: Option<Ts>,
}

/// An event as it is written: its own fields and its durations relative to the save.
#[derive(Serialize)]
struct EventOut<'a> {
    #[serde(flatten)]
    event: &'a Event,
    rel: Rel,
}

/// The inbox as it is written. Borrows the events, so saving copies no event text.
#[derive(Serialize)]
struct Saved<'a> {
    next_id: u64,
    events: Vec<EventOut<'a>>,
    spoken_seq: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    saved_at: Option<Ts>,
}

/// An event's durations from a file: the relative form if it has one, otherwise (a file from before durations
/// were kept apart) from its wall-clock times. A clock that is not valid cannot place an old wall time: a time
/// to live then starts over from the event's arrival, and a snooze ends at once, so nothing is lost or kept
/// for years.
fn timing_from(e: &Event, rel: Option<Rel>, r: &Restore) -> Timing {
    let mut t = match rel {
        Some(rel) => Timing {
            received: r.past(rel.age),
            expires: rel.ttl_left.map(|l| r.ahead(l)),
            snooze: rel.snooze_left.map(|l| r.ahead(l)),
        },
        None => Timing {
            received: r.past(r.wall.saturating_sub(e.received_at).max(0)),
            expires: e.expires_at.map(|t| {
                let from = if r.valid { r.wall } else { e.received_at };
                r.ahead(t.saturating_sub(from))
            }),
            snooze: match e.state {
                EventState::Snoozed { until } => Some(r.ahead(if r.valid {
                    until.saturating_sub(r.wall)
                } else {
                    0
                })),
                _ => None,
            },
        },
    };
    match e.state {
        EventState::Snoozed { .. } => t.snooze = t.snooze.or(Some(r.mono)),
        _ => t.snooze = None,
    }
    t
}

/// What to announce next: the most important event, and how many others are waiting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The event to speak about now.
    pub lead: u64,
    /// Others waiting, most important first.
    pub rest: Vec<u64>,
}

/// A per-source roll-up for "what are my agents doing?" (REQ-INB-016).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceStatus {
    /// Who the speaker names: the endpoint's spoken name, or its name.
    pub origin: String,
    /// Events blocked on the user (not yet acknowledged or dismissed).
    pub waiting_on_you: usize,
    /// Finished events not yet acknowledged or dismissed.
    pub finished: usize,
    /// Events not yet announced.
    pub unheard: usize,
    /// Headline of the most recent event.
    pub latest: String,
}

/// The inbox.
#[derive(Debug, Clone)]
pub struct Inbox {
    state: InboxState,
    limits: InboxLimits,
    /// The device-wide quiet-hours window (REQ-INB-040), on top of each endpoint's own.
    quiet: Option<QuietHours>,
}

impl Inbox {
    /// An empty inbox.
    #[must_use]
    pub fn new(limits: InboxLimits) -> Self {
        Self {
            state: InboxState::default(),
            limits,
            quiet: None,
        }
    }

    /// Set or clear the device-wide quiet-hours window: until it ends, only urgent events are announced
    /// (REQ-INB-040).
    pub fn set_quiet(&mut self, quiet: Option<QuietHours>) {
        self.quiet = quiet;
    }

    /// The device-wide quiet-hours window, if one is set.
    #[must_use]
    pub fn quiet(&self) -> Option<QuietHours> {
        self.quiet
    }

    /// Restore from a store and put the durations (time to live, snooze, age) back on the monotonic clock.
    ///
    /// A file holds each duration as seconds relative to the save, with the wall time of the save. The time
    /// the program was off is taken off with the wall clock as far as it can be trusted (a clock that is not
    /// valid, or is behind the save, takes off nothing). A file from before this form is read from its
    /// wall-clock times (see `timing_from`).
    ///
    /// # Errors
    /// Storage errors.
    pub fn load(
        store: &JsonStore,
        limits: InboxLimits,
        clock: &dyn Clock,
    ) -> Result<Self, StoreError> {
        let mut state: InboxState = store.load()?;
        let restore = Restore::new(state.saved_at.take(), clock);
        for e in &mut state.events {
            let rel = e.rel.take();
            e.timing = timing_from(e, rel, &restore);
        }
        Ok(Self {
            state,
            limits,
            quiet: None,
        })
    }

    /// Tie events saved before endpoint ids existed to an endpoint: each event with no id takes the id of the
    /// live endpoint of its name, and is dropped when there is none (that endpoint was deleted, and the events
    /// it left are history nothing can reach). Events that have an id are not touched. Returns how many events
    /// changed or went, so the caller knows to save.
    pub fn adopt_endpoint_ids(&mut self, endpoints: &Endpoints) -> usize {
        let mut changed = 0;
        self.state.events.retain_mut(|e| {
            if !e.endpoint_id.is_empty() {
                return true;
            }
            changed += 1;
            match endpoints.get(&e.endpoint) {
                Some(ep) => {
                    e.endpoint_id.clone_from(&ep.id);
                    true
                }
                None => false,
            }
        });
        changed
    }

    /// Persist atomically.
    ///
    /// # Errors
    /// Storage errors.
    pub fn save(&self, store: &JsonStore, clock: &dyn Clock) -> Result<(), StoreError> {
        store.write(&self.encode(store, clock)?)
    }

    /// The bytes [`Inbox::save`] would write, so the caller can serialise under a lock and write later. The
    /// durations are worked out afresh at every save, because the file is rewritten only when the inbox
    /// changes and an old figure would be wrong.
    ///
    /// # Errors
    /// A state that cannot be serialised.
    pub fn encode(&self, store: &JsonStore, clock: &dyn Clock) -> Result<Vec<u8>, StoreError> {
        let mono = clock.monotonic_secs();
        let events = self
            .state
            .events
            .iter()
            .map(|e| EventOut {
                event: e,
                rel: Rel {
                    age: mono.saturating_sub(e.timing.received),
                    ttl_left: e.timing.expires.map(|t| t.saturating_sub(mono)),
                    snooze_left: e.timing.snooze.map(|t| t.saturating_sub(mono)),
                },
            })
            .collect();
        store.encode(&Saved {
            next_id: self.state.next_id,
            events,
            spoken_seq: self.state.spoken_seq,
            saved_at: saved_at(clock),
        })
    }

    /// All events, oldest first.
    #[must_use]
    pub fn events(&self) -> &[Event] {
        &self.state.events
    }

    /// One event.
    #[must_use]
    pub fn get(&self, id: u64) -> Option<&Event> {
        self.state.events.iter().find(|e| e.id == id)
    }

    fn get_mut(&mut self, id: u64) -> Option<&mut Event> {
        self.state.events.iter_mut().find(|e| e.id == id)
    }

    fn total_bytes(&self) -> usize {
        self.state.events.iter().map(Event::size).sum()
    }

    /// Queue an event from an endpoint, applying its filter.
    pub fn ingest(
        &mut self,
        now: impl Into<Moment>,
        endpoint: &Endpoint,
        mapped: Mapped,
    ) -> Ingest {
        let now = now.into();
        let f = &endpoint.filter;
        if f.muted {
            return Ingest::Filtered(FilterReason::Muted);
        }
        if mapped.priority < f.min_priority {
            return Ingest::Filtered(FilterReason::BelowPriority);
        }
        if f.ignore_kinds.contains(&mapped.kind) {
            return Ingest::Filtered(FilterReason::IgnoredKind);
        }

        // Agents such as Claude Code fire a hook at the end of every turn; merge bursts (REQ-INB-014).
        if mapped.session_ref.is_some() {
            if let Some(existing) = self.state.events.iter_mut().rev().find(|e| {
                e.endpoint_id == endpoint.id
                    && e.session_ref == mapped.session_ref
                    && e.kind == mapped.kind
                    && e.state == EventState::New
                    && now.mono.saturating_sub(e.timing.received) <= f.coalesce_window_secs
            }) {
                existing.title = mapped.title;
                existing.text = mapped.text;
                existing.status = mapped.status;
                existing.priority = existing.priority.max(mapped.priority);
                existing.received_at = now.wall;
                existing.timing.received = now.mono;
                return Ingest::Coalesced(existing.id);
            }
        }

        self.state.next_id += 1;
        let id = self.state.next_id;
        self.state.events.push(Event {
            id,
            endpoint: endpoint.name.clone(),
            endpoint_id: endpoint.id.clone(),
            source: mapped.source,
            origin: endpoint.origin().to_owned(),
            kind: mapped.kind,
            title: mapped.title,
            text: mapped.text,
            status: mapped.status,
            priority: mapped.priority,
            session_ref: mapped.session_ref,
            no_reply: mapped.no_reply,
            expires_at: mapped.ttl_secs.map(|t| now.wall.saturating_add(t)),
            received_at: now.wall,
            timing: Timing {
                received: now.mono,
                expires: mapped.ttl_secs.map(|t| now.mono.saturating_add(t)),
                snooze: None,
            },
            rel: None,
            state: EventState::New,
            spoken_seq: 0,
        });
        self.enforce_limits(id);
        Ingest::Accepted(id)
    }

    /// The endpoint that should give up an event now: the one holding the most of whichever limit is broken
    /// (events, or bytes if only the byte budget is), provided it holds more than half of that limit. One token
    /// holder who floods the queue therefore evicts its own events, not those of an endpoint that holds half the
    /// queue or less. `None` when no endpoint is past half (a single endpoint always is, once the queue is full),
    /// and the order below then applies to every event.
    fn heaviest_endpoint(&self) -> Option<String> {
        let over_count = self.state.events.len() > self.limits.max_events;
        let (limit, weigh): (usize, fn(&Event) -> usize) = if over_count {
            (self.limits.max_events, |_| 1)
        } else {
            (self.limits.max_bytes, Event::size)
        };
        let mut held: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
        for e in &self.state.events {
            *held.entry(e.endpoint_id.as_str()).or_default() += weigh(e);
        }
        held.into_iter()
            .max_by_key(|&(_, n)| n)
            .filter(|&(_, n)| n > limit / 2)
            .map(|(name, _)| name.to_owned())
    }

    /// Evict until within limits. Order: finished-with events first, then announced, then the least
    /// important unheard, taken from the endpoint holding the most when it holds more than half the queue (see
    /// [`Inbox::heaviest_endpoint`]). The event just added is evicted only as a last resort.
    fn enforce_limits(&mut self, protect: u64) {
        let rank = |e: &Event| -> (u8, Priority, Ts) {
            let class = match e.state {
                EventState::Dismissed | EventState::Acknowledged => 0,
                EventState::Announced => 1,
                EventState::Snoozed { .. } => 2,
                EventState::New => 3,
            };
            (class, e.priority, e.timing.received)
        };
        while self.state.events.len() > self.limits.max_events
            || self.total_bytes() > self.limits.max_bytes
        {
            let heaviest = self.heaviest_endpoint();
            let pick = |only: Option<&str>| {
                self.state
                    .events
                    .iter()
                    .filter(|e| e.id != protect && only.is_none_or(|n| e.endpoint_id == n))
                    .min_by_key(|e| rank(e))
                    .map(|e| e.id)
            };
            let victim = pick(heaviest.as_deref())
                .or_else(|| pick(None))
                .or_else(|| self.state.events.first().map(|e| e.id));
            match victim {
                Some(id) => self.state.events.retain(|e| e.id != id),
                None => break,
            }
        }
    }

    /// An endpoint's spoken name changed: events already queued are spoken under the new one.
    pub fn set_origin_for(&mut self, endpoint_id: &str, origin: &str) {
        for e in self
            .state
            .events
            .iter_mut()
            .filter(|e| e.endpoint_id == endpoint_id)
        {
            origin.clone_into(&mut e.origin);
        }
    }

    /// Drop the events of a deleted endpoint that are still open (not yet spoken, spoken and waiting, or
    /// snoozed); returns how many. Handled events stay as history until [`Inbox::prune`] drops them.
    pub fn drop_open_for(&mut self, endpoint_id: &str) -> usize {
        let before = self.state.events.len();
        self.state.events.retain(|e| {
            e.endpoint_id != endpoint_id
                || matches!(e.state, EventState::Dismissed | EventState::Acknowledged)
        });
        before - self.state.events.len()
    }

    /// Drop finished-with events older than `keep_secs`. Returns how many, so the caller knows to save.
    pub fn prune(&mut self, now: impl Into<Moment>, keep_secs: i64) -> usize {
        let now = now.into();
        let before = self.state.events.len();
        self.state.events.retain(|e| {
            !matches!(e.state, EventState::Dismissed | EventState::Acknowledged)
                || now.mono.saturating_sub(e.timing.received) <= keep_secs
        });
        before - self.state.events.len()
    }

    /// Drop events whose time to live ran out before they were spoken (REQ-INB-023). Returns how many.
    pub fn expire(&mut self, now: impl Into<Moment>) -> usize {
        let now = now.into();
        let before = self.state.events.len();
        self.state.events.retain(|e| !Self::lapsed(e, now));
        before - self.state.events.len()
    }

    /// Unspoken and past its time to live.
    fn lapsed(e: &Event, now: Moment) -> bool {
        matches!(e.state, EventState::New | EventState::Snoozed { .. })
            && e.timing.expires.is_some_and(|t| t <= now.mono)
    }

    fn announceable(
        &self,
        e: &Event,
        now: Moment,
        minute_of_day: u16,
        endpoints: &Endpoints,
    ) -> bool {
        if Self::lapsed(e, now) {
            return false;
        }
        let due = match e.state {
            EventState::New => true,
            EventState::Snoozed { .. } => e.timing.snooze.is_none_or(|t| t <= now.mono),
            _ => false,
        };
        if !due {
            return false;
        }
        let quiet = self.quiet.is_some_and(|q| q.contains(minute_of_day))
            || endpoints
                .get_by_id(&e.endpoint_id)
                .and_then(|ep| ep.filter.quiet_hours)
                .is_some_and(|q| q.contains(minute_of_day));
        !quiet || e.priority == Priority::Urgent
    }

    /// What to announce now, or `None`. Nothing is announced while the device is busy (REQ-INB-006).
    #[must_use]
    pub fn plan(
        &self,
        now: impl Into<Moment>,
        device_idle: bool,
        minute_of_day: u16,
        endpoints: &Endpoints,
    ) -> Option<Plan> {
        let now = now.into();
        if !device_idle {
            return None;
        }
        let mut due: Vec<&Event> = self
            .state
            .events
            .iter()
            .filter(|e| self.announceable(e, now, minute_of_day, endpoints))
            .collect();
        // Most important first: urgent, then things blocked on the user, then oldest first.
        due.sort_by_key(|e| {
            (
                std::cmp::Reverse(e.priority),
                !e.kind.wants_reply(),
                e.timing.received,
                e.id,
            )
        });
        let (lead, rest) = due.split_first()?;
        Some(Plan {
            lead: lead.id,
            rest: rest.iter().map(|e| e.id).collect(),
        })
    }

    /// Mark events as spoken to the user, in the order given. An event spoken again (a repeat, its details)
    /// becomes the most recently spoken one.
    pub fn mark_announced(&mut self, ids: &[u64]) {
        for id in ids {
            let seq = self.state.spoken_seq + 1;
            let Some(e) = self.state.events.iter_mut().find(|e| e.id == *id) else {
                continue;
            };
            if matches!(e.state, EventState::New | EventState::Snoozed { .. }) {
                e.state = EventState::Announced;
                e.timing.snooze = None;
            }
            if e.state == EventState::Announced {
                e.spoken_seq = seq;
                self.state.spoken_seq = seq;
            }
        }
    }

    /// The user acknowledged an event ("thanks, I'll handle it").
    pub fn acknowledge(&mut self, id: u64) -> bool {
        self.set_state(id, EventState::Acknowledged)
    }

    /// The user dismissed an event.
    pub fn dismiss(&mut self, id: u64) -> bool {
        self.set_state(id, EventState::Dismissed)
    }

    /// Postpone an event for `secs` seconds, counted on the monotonic clock.
    pub fn snooze(&mut self, id: u64, now: impl Into<Moment>, secs: i64) -> bool {
        let now = now.into();
        let done = self.set_state(
            id,
            EventState::Snoozed {
                until: now.wall.saturating_add(secs),
            },
        );
        if let Some(e) = self.get_mut(id) {
            e.timing.snooze = Some(now.mono.saturating_add(secs));
        }
        done
    }

    /// Change an event's state. A snooze belongs to the state, so any other state ends it.
    fn set_state(&mut self, id: u64, state: EventState) -> bool {
        match self.get_mut(id) {
            Some(e) => {
                e.state = state;
                e.timing.snooze = None;
                true
            }
            None => false,
        }
    }

    /// The event a spoken reply should go to: the one spoken most recently that is still open, which is not
    /// always the one that arrived last (an urgent event is spoken before an older one; a snoozed one comes
    /// back later). A one-way announcement is never a target, and it does not hand the reply to an older
    /// event either, so an answer meant for a doorbell message cannot land on a coding agent (REQ-INB-023).
    #[must_use]
    pub fn reply_target(&self) -> Option<&Event> {
        self.state
            .events
            .iter()
            .enumerate()
            .filter(|(_, e)| e.state == EventState::Announced)
            // Events saved before the order was recorded all have zero, and fall back to arrival order.
            .max_by_key(|(i, e)| (e.spoken_seq, *i))
            .map(|(_, e)| e)
            .filter(|e| !e.no_reply)
    }

    /// Per-source roll-up of what is open (REQ-INB-016), busiest first.
    #[must_use]
    pub fn status_by_source(&self) -> Vec<SourceStatus> {
        let mut out: Vec<SourceStatus> = Vec::new();
        for e in &self.state.events {
            if matches!(e.state, EventState::Dismissed | EventState::Acknowledged) {
                continue;
            }
            let idx = match out.iter().position(|s| s.origin == e.origin_name()) {
                Some(i) => i,
                None => {
                    out.push(SourceStatus {
                        origin: e.origin_name().to_owned(),
                        waiting_on_you: 0,
                        finished: 0,
                        unheard: 0,
                        latest: String::new(),
                    });
                    out.len() - 1
                }
            };
            let s = &mut out[idx];
            if e.kind.wants_reply() {
                s.waiting_on_you += 1;
            }
            if e.kind == EventKind::Done {
                s.finished += 1;
            }
            if e.state == EventState::New {
                s.unheard += 1;
            }
            s.latest.clone_from(&e.title);
        }
        out.sort_by_key(|s| std::cmp::Reverse((s.waiting_on_you, s.finished + s.unheard)));
        out
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod id_tests;
