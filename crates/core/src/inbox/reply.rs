//! Reply delivery to agent bridges (REQ-INB-008, ADR-0006).
//!
//! The device never calls out to agent hosts. A bridge opens a stream to the device and *pulls* replies.
//! Delivery is at-least-once: a reply stays queued until the bridge acknowledges it, and is re-sent to a
//! reconnecting bridge. The queue is bounded and expires old replies so a missing bridge cannot grow it
//! without limit.

use super::{Moment, Restore, saved_at};
use crate::sched::{Clock, Ts};
use crate::store::{JsonStore, StoreError};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Schema version of the persisted reply queue.
pub const SCHEMA: u32 = 1;
/// Longest reply text kept.
pub const MAX_REPLY_CHARS: usize = 4_000;

// The wire types live in the protocol crate, shared with the agent bridge.
pub use clawdot_protocol::bridge::{Reply, ReplyKind};

/// The queue is full of unexpired replies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Full;

impl fmt::Display for Full {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "too many replies are waiting for a bridge")
    }
}

impl std::error::Error for Full {}

/// A reply and how long it has waited. `created_at` on the reply is the wall time the bridge is shown; the wait
/// is measured on the monotonic clock, which the wall clock cannot step.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct Queued {
    #[serde(flatten)]
    reply: Reply,
    /// Seconds it had waited when the file was written. Read at load only; a file from before this form has
    /// none, and the wait is then worked out from `created_at`.
    #[serde(default)]
    age_secs: Option<i64>,
    /// The monotonic reading at which it was queued. Not part of the file.
    #[serde(skip)]
    born: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
struct State {
    next_id: u64,
    pending: Vec<Queued>,
    /// The wall time of the save, when the clock was valid then (see `Restore`). Read at load only.
    #[serde(default)]
    saved_at: Option<Ts>,
}

/// A reply as it is written: its own fields and how long it had waited at the save.
#[derive(Serialize)]
struct QueuedOut<'a> {
    #[serde(flatten)]
    reply: &'a Reply,
    age_secs: i64,
}

/// The queue as it is written.
#[derive(Serialize)]
struct Saved<'a> {
    next_id: u64,
    pending: Vec<QueuedOut<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    saved_at: Option<Ts>,
}

/// Replies waiting for bridges.
#[derive(Debug, Clone)]
pub struct ReplyQueue {
    state: State,
    max_pending: usize,
    ttl_secs: i64,
}

impl ReplyQueue {
    /// An empty queue holding at most `max_pending` replies, each for at most `ttl_secs`.
    #[must_use]
    pub fn new(max_pending: usize, ttl_secs: i64) -> Self {
        Self {
            state: State::default(),
            max_pending,
            ttl_secs,
        }
    }

    /// Restore from a store. How long each reply has waited is put back on the monotonic clock: the file holds
    /// it as seconds, and the time the program was off is added with the wall clock as far as it can be trusted
    /// (see `Restore`). A file from before this form is read from `created_at`, as far as the wall clock is
    /// valid.
    ///
    /// # Errors
    /// Storage errors.
    pub fn load(
        store: &JsonStore,
        max_pending: usize,
        ttl_secs: i64,
        clock: &dyn Clock,
    ) -> Result<Self, StoreError> {
        let mut state: State = store.load()?;
        let restore = Restore::new(state.saved_at.take(), clock);
        for q in &mut state.pending {
            let age = q
                .age_secs
                .take()
                .unwrap_or_else(|| restore.wall.saturating_sub(q.reply.created_at).max(0));
            q.born = restore.past(age);
        }
        Ok(Self {
            state,
            max_pending,
            ttl_secs,
        })
    }

    /// Persist atomically.
    ///
    /// # Errors
    /// Storage errors.
    pub fn save(&self, store: &JsonStore, clock: &dyn Clock) -> Result<(), StoreError> {
        store.write(&self.encode(store, clock)?)
    }

    /// The bytes [`ReplyQueue::save`] would write, so the caller can serialise under a lock and write later.
    ///
    /// # Errors
    /// A state that cannot be serialised.
    pub fn encode(&self, store: &JsonStore, clock: &dyn Clock) -> Result<Vec<u8>, StoreError> {
        let mono = clock.monotonic_secs();
        store.encode(&Saved {
            next_id: self.state.next_id,
            pending: self
                .state
                .pending
                .iter()
                .map(|q| QueuedOut {
                    reply: &q.reply,
                    age_secs: mono.saturating_sub(q.born),
                })
                .collect(),
            saved_at: saved_at(clock),
        })
    }

    /// Queue a reply for the endpoint's bridge.
    ///
    /// # Errors
    /// [`Full`] when `max_pending` unexpired replies are already waiting.
    pub fn push(
        &mut self,
        now: impl Into<Moment>,
        endpoint: &str,
        event_id: u64,
        session_ref: Option<String>,
        kind: ReplyKind,
        text: &str,
    ) -> Result<u64, Full> {
        let now = now.into();
        self.expire(now);
        if self.state.pending.len() >= self.max_pending {
            return Err(Full);
        }
        self.state.next_id += 1;
        let id = self.state.next_id;
        self.state.pending.push(Queued {
            reply: Reply {
                id,
                endpoint: endpoint.to_owned(),
                event_id,
                session_ref,
                kind,
                text: text.chars().take(MAX_REPLY_CHARS).collect(),
                created_at: now.wall,
            },
            age_secs: None,
            born: now.mono,
        });
        Ok(id)
    }

    /// Unacknowledged replies for an endpoint with an id greater than `after`, oldest first. A new
    /// connection passes 0 and so receives everything still unacknowledged (at-least-once).
    #[must_use]
    pub fn pending_for(&self, endpoint: &str, after: u64) -> Vec<&Reply> {
        self.state
            .pending
            .iter()
            .map(|q| &q.reply)
            .filter(|r| r.endpoint == endpoint && r.id > after)
            .collect()
    }

    /// The bridge confirmed it handled a reply. Acknowledging from the wrong endpoint does nothing.
    pub fn ack(&mut self, endpoint: &str, id: u64) -> bool {
        let before = self.state.pending.len();
        self.state
            .pending
            .retain(|q| !(q.reply.id == id && q.reply.endpoint == endpoint));
        self.state.pending.len() != before
    }

    /// Drop every reply waiting for `endpoint` (it was deleted); returns how many.
    pub fn drop_for(&mut self, endpoint: &str) -> usize {
        let before = self.state.pending.len();
        self.state.pending.retain(|q| q.reply.endpoint != endpoint);
        before - self.state.pending.len()
    }

    /// Drop replies that have waited longer than the time-to-live, counted on the monotonic clock; returns how
    /// many.
    pub fn expire(&mut self, now: impl Into<Moment>) -> usize {
        let mono = now.into().mono;
        let before = self.state.pending.len();
        let ttl = self.ttl_secs;
        self.state
            .pending
            .retain(|q| mono.saturating_sub(q.born) <= ttl);
        before - self.state.pending.len()
    }

    /// How many replies are waiting in total.
    #[must_use]
    pub fn len(&self) -> usize {
        self.state.pending.len()
    }

    /// Whether nothing is waiting.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.state.pending.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sched::testclock::StepClock;
    use crate::store::testutil::TempDir;

    fn push(q: &mut ReplyQueue, now: Ts, endpoint: &str, text: &str) -> u64 {
        q.push(
            now,
            endpoint,
            1,
            Some("s".into()),
            ReplyKind::Instruct,
            text,
        )
        .unwrap()
    }

    #[test]
    fn replies_are_delivered_in_order_and_only_to_their_endpoint() {
        let mut q = ReplyQueue::new(10, 3600);
        let a = push(&mut q, 0, "cc", "first");
        let b = push(&mut q, 1, "cc", "second");
        let _other = push(&mut q, 2, "ci", "elsewhere");
        let ids: Vec<u64> = q.pending_for("cc", 0).iter().map(|r| r.id).collect();
        assert_eq!(ids, vec![a, b]);
        assert_eq!(
            q.pending_for("cc", a).len(),
            1,
            "cursor within a connection"
        );
        assert_eq!(q.pending_for("nobody", 0).len(), 0);
    }

    #[test]
    fn unacknowledged_replies_are_resent_to_a_reconnecting_bridge() {
        let mut q = ReplyQueue::new(10, 3600);
        let id = push(&mut q, 0, "cc", "do it");
        // First connection saw it but died before acknowledging.
        assert_eq!(q.pending_for("cc", 0).len(), 1);
        // Reconnect: still there.
        assert_eq!(q.pending_for("cc", 0).len(), 1);
        assert!(q.ack("cc", id));
        assert!(q.pending_for("cc", 0).is_empty());
        assert!(!q.ack("cc", id), "a second ack is harmless");
    }

    #[test]
    fn a_bridge_cannot_acknowledge_another_endpoints_reply() {
        let mut q = ReplyQueue::new(10, 3600);
        let id = push(&mut q, 0, "cc", "x");
        assert!(!q.ack("ci", id));
        assert_eq!(q.len(), 1);
    }

    #[test]
    fn old_replies_expire_so_a_missing_bridge_cannot_grow_the_queue() {
        let mut q = ReplyQueue::new(10, 100);
        push(&mut q, 0, "cc", "stale");
        push(&mut q, 90, "cc", "fresh");
        assert_eq!(q.expire(150), 1);
        assert_eq!(q.pending_for("cc", 0)[0].text, "fresh");
    }

    #[test]
    fn the_queue_is_bounded_and_says_so() {
        let mut q = ReplyQueue::new(2, 3600);
        push(&mut q, 0, "cc", "a");
        push(&mut q, 0, "cc", "b");
        assert_eq!(q.push(0, "cc", 1, None, ReplyKind::Ack, ""), Err(Full));
        // Expired replies make room.
        assert!(q.push(10_000, "cc", 1, None, ReplyKind::Ack, "").is_ok());
    }

    #[test]
    fn long_replies_are_clamped() {
        let mut q = ReplyQueue::new(2, 3600);
        push(&mut q, 0, "cc", &"x".repeat(MAX_REPLY_CHARS * 2));
        assert_eq!(
            q.pending_for("cc", 0)[0].text.chars().count(),
            MAX_REPLY_CHARS
        );
    }

    #[test]
    fn replies_survive_a_restart() {
        let dir = TempDir::new("replies");
        let store = JsonStore::new(dir.0.join("replies.json"), SCHEMA);
        let mut q = ReplyQueue::new(10, 3600);
        push(&mut q, 0, "cc", "keep");
        let clock = StepClock::at(T0);
        q.save(&store, &clock).unwrap();
        let mut restored = ReplyQueue::load(&store, 10, 3600, &clock).unwrap();
        assert_eq!(restored.pending_for("cc", 0).len(), 1);
        assert_eq!(push(&mut restored, 1, "cc", "next"), 2, "ids keep counting");
    }

    const T0: Ts = 1_800_000_000;
    const JUMP: i64 = 5 * 365 * 86_400;

    fn queue_at(q: &mut ReplyQueue, clock: &StepClock) -> u64 {
        q.push(
            Moment::of(clock),
            "cc",
            1,
            None,
            ReplyKind::Instruct,
            "do it",
        )
        .unwrap()
    }

    // REQ-INB-008: a reply waits for its time to live on the monotonic clock, so network time arriving does
    // not drop it, and a clock set back does not keep it.
    #[test]
    fn a_reply_expires_by_the_monotonic_clock() {
        let mut q = ReplyQueue::new(10, 100);
        let clock = StepClock::at(T0);
        queue_at(&mut q, &clock);
        clock.advance(50);
        clock.step_wall(JUMP);
        assert_eq!(q.expire(Moment::of(&clock)), 0);
        assert_eq!(
            q.pending_for("cc", 0)[0].created_at,
            T0,
            "the bridge sees wall time"
        );
        clock.advance(51);
        clock.step_wall(-2 * JUMP);
        assert_eq!(q.expire(Moment::of(&clock)), 1);
    }

    // REQ-INB-008: the wait survives a restart, and the time off counts when the wall clock can be trusted.
    #[test]
    fn how_long_a_reply_has_waited_survives_a_restart() {
        let dir = TempDir::new("replies-age");
        let store = JsonStore::new(dir.0.join("replies.json"), SCHEMA);
        let mut q = ReplyQueue::new(10, 100);
        let clock = StepClock::at(T0);
        queue_at(&mut q, &clock);
        clock.advance(30);
        q.save(&store, &clock).unwrap();

        // Back after 20 s off: 50 s waited, 50 to go.
        let back = StepClock::with_mono(T0 + 50, 3);
        let mut r = ReplyQueue::load(&store, 10, 100, &back).unwrap();
        back.advance(49);
        assert_eq!(r.expire(Moment::of(&back)), 0);
        back.advance(2);
        assert_eq!(r.expire(Moment::of(&back)), 1);

        // Back with the wall clock not set: nothing is taken off, and setting it later changes nothing.
        let unset = StepClock::with_mono(5, 3);
        let mut r = ReplyQueue::load(&store, 10, 100, &unset).unwrap();
        unset.step_wall(T0 + JUMP);
        unset.advance(69);
        assert_eq!(r.expire(Moment::of(&unset)), 0, "30 s waited, 99 now");
        unset.advance(2);
        assert_eq!(r.expire(Moment::of(&unset)), 1);
    }

    // REQ-NFR-010: a file written before this form still loads, aged by its `created_at`.
    #[test]
    fn a_reply_file_from_before_durations_were_kept_apart_still_loads() {
        let dir = TempDir::new("replies-legacy");
        let store = JsonStore::new(dir.0.join("replies.json"), SCHEMA);
        let old = format!(
            r#"{{"schema":1,"data":{{"next_id":1,"pending":[{{"id":1,"endpoint":"cc","event_id":1,
            "session_ref":null,"kind":"ack","text":"","created_at":{}}}]}}}}"#,
            T0 - 80
        );
        std::fs::write(store.path(), old).unwrap();
        let clock = StepClock::at(T0);
        let mut r = ReplyQueue::load(&store, 10, 100, &clock).unwrap();
        assert_eq!(r.pending_for("cc", 0).len(), 1);
        clock.advance(19);
        assert_eq!(r.expire(Moment::of(&clock)), 0);
        clock.advance(2);
        assert_eq!(r.expire(Moment::of(&clock)), 1);
        // Where the wall clock is not set, the age cannot be known and counts from the start.
        let unset = StepClock::with_mono(5, 3);
        let r = ReplyQueue::load(&store, 10, 100, &unset).unwrap();
        assert_eq!(r.len(), 1);
    }
}
