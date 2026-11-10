//! The inbox queue: persistence, filtering, coalescing, eviction, and announcement order
//! (REQ-INB-004..007, 012, 014, 016).

use super::adapter::Mapped;
use super::endpoint::{Endpoint, Endpoints, QuietHours};
use super::event::{Event, EventKind, EventState, Priority};
use crate::sched::Ts;
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
    /// The source name.
    pub source: String,
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

    /// Restore from a store.
    ///
    /// # Errors
    /// Storage errors.
    pub fn load(store: &JsonStore, limits: InboxLimits) -> Result<Self, StoreError> {
        Ok(Self {
            state: store.load()?,
            limits,
            quiet: None,
        })
    }

    /// Persist atomically.
    ///
    /// # Errors
    /// Storage errors.
    pub fn save(&self, store: &JsonStore) -> Result<(), StoreError> {
        store.save(&self.state)
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
    pub fn ingest(&mut self, now: Ts, endpoint: &Endpoint, mapped: Mapped) -> Ingest {
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
                e.endpoint == endpoint.name
                    && e.session_ref == mapped.session_ref
                    && e.kind == mapped.kind
                    && e.state == EventState::New
                    && now - e.received_at <= f.coalesce_window_secs
            }) {
                existing.title = mapped.title;
                existing.text = mapped.text;
                existing.status = mapped.status;
                existing.priority = existing.priority.max(mapped.priority);
                existing.received_at = now;
                return Ingest::Coalesced(existing.id);
            }
        }

        self.state.next_id += 1;
        let id = self.state.next_id;
        self.state.events.push(Event {
            id,
            endpoint: endpoint.name.clone(),
            source: mapped.source,
            kind: mapped.kind,
            title: mapped.title,
            text: mapped.text,
            status: mapped.status,
            priority: mapped.priority,
            session_ref: mapped.session_ref,
            no_reply: mapped.no_reply,
            expires_at: mapped.ttl_secs.map(|t| now.saturating_add(t)),
            received_at: now,
            state: EventState::New,
        });
        self.enforce_limits(id);
        Ingest::Accepted(id)
    }

    /// Evict until within limits. Order: finished-with events first, then announced, then the least
    /// important unheard. The event just added is evicted only as a last resort.
    fn enforce_limits(&mut self, protect: u64) {
        let rank = |e: &Event| -> (u8, Priority, Ts) {
            let class = match e.state {
                EventState::Dismissed | EventState::Acknowledged => 0,
                EventState::Announced => 1,
                EventState::Snoozed { .. } => 2,
                EventState::New => 3,
            };
            (class, e.priority, e.received_at)
        };
        while self.state.events.len() > self.limits.max_events
            || self.total_bytes() > self.limits.max_bytes
        {
            let victim = self
                .state
                .events
                .iter()
                .filter(|e| e.id != protect)
                .min_by_key(|e| rank(e))
                .map(|e| e.id)
                .or_else(|| self.state.events.first().map(|e| e.id));
            match victim {
                Some(id) => self.state.events.retain(|e| e.id != id),
                None => break,
            }
        }
    }

    /// Drop finished-with events older than `keep_secs`.
    pub fn prune(&mut self, now: Ts, keep_secs: i64) {
        self.state.events.retain(|e| {
            !matches!(e.state, EventState::Dismissed | EventState::Acknowledged)
                || now - e.received_at <= keep_secs
        });
    }

    /// Drop events whose time to live ran out before they were spoken (REQ-INB-023). Returns how many.
    pub fn expire(&mut self, now: Ts) -> usize {
        let before = self.state.events.len();
        self.state.events.retain(|e| !Self::lapsed(e, now));
        before - self.state.events.len()
    }

    /// Unspoken and past its time to live.
    fn lapsed(e: &Event, now: Ts) -> bool {
        matches!(e.state, EventState::New | EventState::Snoozed { .. })
            && e.expires_at.is_some_and(|t| t <= now)
    }

    fn announceable(&self, e: &Event, now: Ts, minute_of_day: u16, endpoints: &Endpoints) -> bool {
        if Self::lapsed(e, now) {
            return false;
        }
        let due = match e.state {
            EventState::New => true,
            EventState::Snoozed { until } => until <= now,
            _ => false,
        };
        if !due {
            return false;
        }
        let quiet = self.quiet.is_some_and(|q| q.contains(minute_of_day))
            || endpoints
                .get(&e.endpoint)
                .and_then(|ep| ep.filter.quiet_hours)
                .is_some_and(|q| q.contains(minute_of_day));
        !quiet || e.priority == Priority::Urgent
    }

    /// What to announce now, or `None`. Nothing is announced while the device is busy (REQ-INB-006).
    #[must_use]
    pub fn plan(
        &self,
        now: Ts,
        device_idle: bool,
        minute_of_day: u16,
        endpoints: &Endpoints,
    ) -> Option<Plan> {
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
                e.received_at,
                e.id,
            )
        });
        let (lead, rest) = due.split_first()?;
        Some(Plan {
            lead: lead.id,
            rest: rest.iter().map(|e| e.id).collect(),
        })
    }

    /// Mark events as spoken to the user.
    pub fn mark_announced(&mut self, ids: &[u64]) {
        for id in ids {
            if let Some(e) = self.get_mut(*id) {
                if matches!(e.state, EventState::New | EventState::Snoozed { .. }) {
                    e.state = EventState::Announced;
                }
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

    /// Postpone an event.
    pub fn snooze(&mut self, id: u64, until: Ts) -> bool {
        self.set_state(id, EventState::Snoozed { until })
    }

    fn set_state(&mut self, id: u64, state: EventState) -> bool {
        match self.get_mut(id) {
            Some(e) => {
                e.state = state;
                true
            }
            None => false,
        }
    }

    /// The event a spoken reply should go to: the most recently announced one still open. A one-way
    /// announcement is never a target, and it does not hand the reply to an older event either, so an
    /// answer meant for a doorbell message cannot land on a coding agent (REQ-INB-023).
    #[must_use]
    pub fn reply_target(&self) -> Option<&Event> {
        self.state
            .events
            .iter()
            .rev()
            .find(|e| e.state == EventState::Announced)
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
            let idx = match out.iter().position(|s| s.source == e.source) {
                Some(i) => i,
                None => {
                    out.push(SourceStatus {
                        source: e.source.clone(),
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
mod tests {
    use super::*;
    use crate::inbox::endpoint::QuietHours;
    use crate::store::testutil::TempDir;

    fn mapped(kind: EventKind, priority: Priority, title: &str, session: Option<&str>) -> Mapped {
        Mapped {
            source: "Claude Code".into(),
            kind,
            title: title.into(),
            text: format!("text of {title}"),
            status: None,
            priority,
            session_ref: session.map(str::to_owned),
            no_reply: false,
            ttl_secs: None,
        }
    }

    fn setup() -> (Inbox, Endpoints) {
        let mut eps = Endpoints::default();
        eps.create("cc", "claude-code", 0).unwrap();
        (Inbox::new(InboxLimits::default()), eps)
    }

    fn ep(eps: &Endpoints) -> Endpoint {
        eps.get("cc").unwrap().clone()
    }

    #[test]
    fn events_are_queued_in_order_with_increasing_ids() {
        let (mut inbox, eps) = setup();
        let a = inbox.ingest(
            10,
            &ep(&eps),
            mapped(EventKind::Done, Priority::Normal, "a", None),
        );
        let b = inbox.ingest(
            11,
            &ep(&eps),
            mapped(EventKind::Done, Priority::Normal, "b", None),
        );
        assert_eq!((a, b), (Ingest::Accepted(1), Ingest::Accepted(2)));
        assert_eq!(inbox.events().len(), 2);
    }

    #[test]
    fn endpoint_filters_drop_unwanted_events() {
        let (mut inbox, mut eps) = setup();
        eps.get_mut("cc").unwrap().filter.min_priority = Priority::Normal;
        eps.get_mut("cc").unwrap().filter.ignore_kinds = vec![EventKind::Question];
        let e = ep(&eps);
        assert_eq!(
            inbox.ingest(1, &e, mapped(EventKind::Done, Priority::Low, "x", None)),
            Ingest::Filtered(FilterReason::BelowPriority)
        );
        assert_eq!(
            inbox.ingest(
                1,
                &e,
                mapped(EventKind::Question, Priority::Normal, "x", None)
            ),
            Ingest::Filtered(FilterReason::IgnoredKind)
        );
        eps.get_mut("cc").unwrap().filter.muted = true;
        assert_eq!(
            inbox.ingest(
                1,
                &ep(&eps),
                mapped(EventKind::Done, Priority::Urgent, "x", None)
            ),
            Ingest::Filtered(FilterReason::Muted)
        );
        assert!(inbox.events().is_empty());
    }

    #[test]
    fn a_burst_of_turn_end_hooks_from_one_session_becomes_one_event() {
        let (mut inbox, eps) = setup();
        let e = ep(&eps);
        let first = inbox.ingest(
            100,
            &e,
            mapped(EventKind::Done, Priority::Normal, "t1", Some("s1")),
        );
        let second = inbox.ingest(
            110,
            &e,
            mapped(EventKind::Done, Priority::Normal, "t2", Some("s1")),
        );
        assert_eq!((first, second), (Ingest::Accepted(1), Ingest::Coalesced(1)));
        assert_eq!(inbox.events().len(), 1);
        assert_eq!(
            inbox.get(1).unwrap().title,
            "t2",
            "keeps the latest content"
        );
        // A different session, or outside the window, is a new event.
        assert_eq!(
            inbox.ingest(
                111,
                &e,
                mapped(EventKind::Done, Priority::Normal, "o", Some("s2"))
            ),
            Ingest::Accepted(2)
        );
        assert_eq!(
            inbox.ingest(
                500,
                &e,
                mapped(EventKind::Done, Priority::Normal, "late", Some("s1"))
            ),
            Ingest::Accepted(3)
        );
    }

    #[test]
    fn announced_events_are_not_merged_into() {
        let (mut inbox, eps) = setup();
        let e = ep(&eps);
        inbox.ingest(
            100,
            &e,
            mapped(EventKind::Done, Priority::Normal, "t1", Some("s1")),
        );
        inbox.mark_announced(&[1]);
        assert_eq!(
            inbox.ingest(
                105,
                &e,
                mapped(EventKind::Done, Priority::Normal, "t2", Some("s1"))
            ),
            Ingest::Accepted(2)
        );
    }

    #[test]
    fn coalescing_keeps_the_higher_priority() {
        let (mut inbox, eps) = setup();
        let e = ep(&eps);
        inbox.ingest(
            100,
            &e,
            mapped(EventKind::Done, Priority::Low, "a", Some("s")),
        );
        inbox.ingest(
            101,
            &e,
            mapped(EventKind::Done, Priority::Urgent, "b", Some("s")),
        );
        assert_eq!(inbox.get(1).unwrap().priority, Priority::Urgent);
    }

    #[test]
    fn the_queue_is_bounded_and_evicts_the_least_important_first() {
        let (mut inbox, eps) = setup();
        inbox.limits.max_events = 3;
        let e = ep(&eps);
        inbox.ingest(
            1,
            &e,
            mapped(EventKind::Done, Priority::Urgent, "urgent", Some("a")),
        );
        inbox.ingest(
            2,
            &e,
            mapped(EventKind::Info, Priority::Low, "low", Some("b")),
        );
        inbox.ingest(
            3,
            &e,
            mapped(EventKind::Done, Priority::Normal, "normal", Some("c")),
        );
        inbox.ingest(
            4,
            &e,
            mapped(EventKind::Done, Priority::Normal, "newest", Some("d")),
        );
        let titles: Vec<_> = inbox.events().iter().map(|x| x.title.as_str()).collect();
        assert_eq!(
            titles,
            vec!["urgent", "normal", "newest"],
            "the low-priority one went"
        );
    }

    #[test]
    fn handled_events_are_evicted_before_unheard_ones() {
        let (mut inbox, eps) = setup();
        inbox.limits.max_events = 2;
        let e = ep(&eps);
        inbox.ingest(
            1,
            &e,
            mapped(EventKind::Done, Priority::Normal, "old-handled", Some("a")),
        );
        inbox.dismiss(1);
        inbox.ingest(
            2,
            &e,
            mapped(EventKind::Done, Priority::Normal, "unheard", Some("b")),
        );
        inbox.ingest(
            3,
            &e,
            mapped(EventKind::Done, Priority::Normal, "newer", Some("c")),
        );
        let titles: Vec<_> = inbox.events().iter().map(|x| x.title.as_str()).collect();
        assert_eq!(titles, vec!["unheard", "newer"]);
    }

    #[test]
    fn the_byte_budget_is_enforced_too() {
        let (mut inbox, eps) = setup();
        inbox.limits.max_bytes = 2_000;
        let e = ep(&eps);
        for i in 0..20 {
            let mut m = mapped(
                EventKind::Done,
                Priority::Normal,
                &format!("e{i}"),
                Some(&format!("s{i}")),
            );
            m.text = "x".repeat(500);
            inbox.ingest(i, &e, m);
        }
        assert!(
            inbox.total_bytes() <= 2_000,
            "{} bytes",
            inbox.total_bytes()
        );
        assert!(!inbox.events().is_empty());
    }

    #[test]
    fn announcement_order_is_urgent_then_blocked_then_oldest() {
        let (mut inbox, eps) = setup();
        let e = ep(&eps);
        inbox.ingest(
            1,
            &e,
            mapped(EventKind::Done, Priority::Normal, "done-old", Some("a")),
        );
        inbox.ingest(
            2,
            &e,
            mapped(
                EventKind::NeedsInput,
                Priority::Normal,
                "blocked",
                Some("b"),
            ),
        );
        inbox.ingest(
            3,
            &e,
            mapped(EventKind::Done, Priority::Urgent, "urgent-done", Some("c")),
        );
        let plan = inbox.plan(10, true, 600, &eps).unwrap();
        assert_eq!(inbox.get(plan.lead).unwrap().title, "urgent-done");
        assert_eq!(plan.rest.len(), 2);
        assert_eq!(inbox.get(plan.rest[0]).unwrap().title, "blocked");
        assert_eq!(inbox.get(plan.rest[1]).unwrap().title, "done-old");
    }

    #[test]
    fn nothing_is_announced_while_the_device_is_busy() {
        let (mut inbox, eps) = setup();
        inbox.ingest(
            1,
            &ep(&eps),
            mapped(EventKind::Done, Priority::Urgent, "x", None),
        );
        assert!(inbox.plan(10, false, 600, &eps).is_none());
        assert!(inbox.plan(10, true, 600, &eps).is_some());
    }

    #[test]
    fn quiet_hours_hold_back_everything_but_urgent() {
        let (mut inbox, mut eps) = setup();
        eps.get_mut("cc").unwrap().filter.quiet_hours = Some(QuietHours {
            start: 22 * 60,
            end: 7 * 60,
        });
        let e = ep(&eps);
        inbox.ingest(
            1,
            &e,
            mapped(EventKind::Done, Priority::Normal, "normal", Some("a")),
        );
        let night = 23 * 60;
        assert!(inbox.plan(10, true, night, &eps).is_none());
        inbox.ingest(
            2,
            &e,
            mapped(EventKind::NeedsInput, Priority::Urgent, "urgent", Some("b")),
        );
        let plan = inbox.plan(10, true, night, &eps).unwrap();
        assert_eq!(inbox.get(plan.lead).unwrap().title, "urgent");
        assert!(plan.rest.is_empty());
        assert!(
            inbox.plan(10, true, 12 * 60, &eps).unwrap().rest.len() == 1,
            "normal is released in the morning"
        );
    }

    // REQ-INB-040
    #[test]
    fn the_device_wide_window_holds_back_everything_but_urgent_and_can_be_cleared() {
        let (mut inbox, eps) = setup();
        let e = ep(&eps);
        inbox.ingest(
            1,
            &e,
            mapped(EventKind::Done, Priority::Normal, "normal", Some("a")),
        );
        let night = 23 * 60;
        assert!(inbox.plan(10, true, night, &eps).is_some(), "no window yet");
        inbox.set_quiet(Some(QuietHours {
            start: 22 * 60,
            end: 7 * 60,
        }));
        assert!(inbox.plan(10, true, night, &eps).is_none(), "held at night");
        assert!(
            inbox.plan(10, true, 3 * 60, &eps).is_none(),
            "the window spans midnight"
        );
        inbox.ingest(
            2,
            &e,
            mapped(EventKind::NeedsInput, Priority::Urgent, "urgent", Some("b")),
        );
        let plan = inbox.plan(10, true, night, &eps).unwrap();
        assert_eq!(inbox.get(plan.lead).unwrap().title, "urgent");
        assert!(plan.rest.is_empty(), "the normal one waits");
        assert_eq!(
            inbox.plan(10, true, 12 * 60, &eps).unwrap().rest.len(),
            1,
            "released after the window"
        );
        inbox.set_quiet(None);
        assert_eq!(
            inbox.plan(10, true, night, &eps).unwrap().rest.len(),
            1,
            "cleared at once"
        );
    }

    #[test]
    fn announcing_acknowledging_dismissing_and_snoozing() {
        let (mut inbox, eps) = setup();
        let e = ep(&eps);
        inbox.ingest(
            1,
            &e,
            mapped(EventKind::Done, Priority::Normal, "a", Some("a")),
        );
        inbox.ingest(
            2,
            &e,
            mapped(EventKind::Done, Priority::Normal, "b", Some("b")),
        );
        let plan = inbox.plan(10, true, 600, &eps).unwrap();
        inbox.mark_announced(&[plan.lead]);
        assert_eq!(inbox.reply_target().unwrap().id, plan.lead);
        assert!(inbox.snooze(plan.lead, 1000));
        assert_eq!(
            inbox.plan(10, true, 600, &eps).unwrap().lead,
            2,
            "snoozed one waits"
        );
        assert_eq!(
            inbox.plan(1000, true, 600, &eps).unwrap().lead,
            1,
            "and returns when due"
        );
        assert!(inbox.acknowledge(2));
        assert!(inbox.dismiss(1));
        assert!(inbox.plan(2000, true, 600, &eps).is_none());
        assert!(!inbox.acknowledge(99), "unknown ids are reported");
    }

    #[test]
    fn what_are_my_agents_doing_groups_open_events_by_source() {
        let (mut inbox, eps) = setup();
        let e = ep(&eps);
        inbox.ingest(
            1,
            &e,
            mapped(
                EventKind::Done,
                Priority::Normal,
                "build finished",
                Some("a"),
            ),
        );
        inbox.ingest(
            2,
            &e,
            mapped(
                EventKind::NeedsInput,
                Priority::Urgent,
                "needs permission",
                Some("b"),
            ),
        );
        let mut other = mapped(EventKind::Done, Priority::Low, "backup ok", None);
        other.source = "Backup".into();
        inbox.ingest(3, &e, other);
        inbox.dismiss(3);
        let status = inbox.status_by_source();
        assert_eq!(status.len(), 1, "dismissed events do not count");
        assert_eq!(
            (
                status[0].waiting_on_you,
                status[0].finished,
                status[0].unheard
            ),
            (1, 1, 2)
        );
        assert_eq!(status[0].latest, "needs permission");
    }

    #[test]
    fn prune_removes_old_handled_events_only() {
        let (mut inbox, eps) = setup();
        let e = ep(&eps);
        inbox.ingest(
            0,
            &e,
            mapped(EventKind::Done, Priority::Normal, "handled-old", Some("a")),
        );
        inbox.dismiss(1);
        inbox.ingest(
            0,
            &e,
            mapped(EventKind::Done, Priority::Normal, "unheard-old", Some("b")),
        );
        inbox.prune(100_000, 86_400);
        let titles: Vec<_> = inbox.events().iter().map(|x| x.title.as_str()).collect();
        assert_eq!(titles, vec!["unheard-old"]);
    }

    #[test]
    fn the_inbox_survives_a_restart() {
        let dir = TempDir::new("inbox");
        let store = JsonStore::new(dir.0.join("inbox.json"), SCHEMA);
        let (mut inbox, eps) = setup();
        inbox.ingest(
            5,
            &ep(&eps),
            mapped(
                EventKind::NeedsInput,
                Priority::Urgent,
                "keep me",
                Some("s"),
            ),
        );
        inbox.save(&store).unwrap();
        let mut restored = Inbox::load(&store, InboxLimits::default()).unwrap();
        assert_eq!(restored.events().len(), 1);
        assert_eq!(
            restored.ingest(
                6,
                &ep(&eps),
                mapped(EventKind::Info, Priority::Low, "next", Some("t"))
            ),
            Ingest::Accepted(2),
            "ids keep counting"
        );
    }
}
