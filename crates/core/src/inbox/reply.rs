//! Reply delivery to agent bridges (REQ-INB-008, ADR-0006).
//!
//! The device never calls out to agent hosts. A bridge opens a stream to the device and *pulls* replies.
//! Delivery is at-least-once: a reply stays queued until the bridge acknowledges it, and is re-sent to a
//! reconnecting bridge. The queue is bounded and expires old replies so a missing bridge cannot grow it
//! without limit.

use crate::sched::Ts;
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

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct State {
    next_id: u64,
    pending: Vec<Reply>,
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

    /// Restore from a store.
    ///
    /// # Errors
    /// Storage errors.
    pub fn load(store: &JsonStore, max_pending: usize, ttl_secs: i64) -> Result<Self, StoreError> {
        Ok(Self {
            state: store.load()?,
            max_pending,
            ttl_secs,
        })
    }

    /// Persist atomically.
    ///
    /// # Errors
    /// Storage errors.
    pub fn save(&self, store: &JsonStore) -> Result<(), StoreError> {
        store.save(&self.state)
    }

    /// The bytes [`ReplyQueue::save`] would write, so the caller can serialise under a lock and write later.
    ///
    /// # Errors
    /// A state that cannot be serialised.
    pub fn encode(&self, store: &JsonStore) -> Result<Vec<u8>, StoreError> {
        store.encode(&self.state)
    }

    /// Queue a reply for the endpoint's bridge.
    ///
    /// # Errors
    /// [`Full`] when `max_pending` unexpired replies are already waiting.
    pub fn push(
        &mut self,
        now: Ts,
        endpoint: &str,
        event_id: u64,
        session_ref: Option<String>,
        kind: ReplyKind,
        text: &str,
    ) -> Result<u64, Full> {
        self.expire(now);
        if self.state.pending.len() >= self.max_pending {
            return Err(Full);
        }
        self.state.next_id += 1;
        let id = self.state.next_id;
        self.state.pending.push(Reply {
            id,
            endpoint: endpoint.to_owned(),
            event_id,
            session_ref,
            kind,
            text: text.chars().take(MAX_REPLY_CHARS).collect(),
            created_at: now,
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
            .filter(|r| r.endpoint == endpoint && r.id > after)
            .collect()
    }

    /// The bridge confirmed it handled a reply. Acknowledging from the wrong endpoint does nothing.
    pub fn ack(&mut self, endpoint: &str, id: u64) -> bool {
        let before = self.state.pending.len();
        self.state
            .pending
            .retain(|r| !(r.id == id && r.endpoint == endpoint));
        self.state.pending.len() != before
    }

    /// Drop every reply waiting for `endpoint` (it was deleted); returns how many.
    pub fn drop_for(&mut self, endpoint: &str) -> usize {
        let before = self.state.pending.len();
        self.state.pending.retain(|r| r.endpoint != endpoint);
        before - self.state.pending.len()
    }

    /// Drop replies older than the time-to-live; returns how many.
    pub fn expire(&mut self, now: Ts) -> usize {
        let before = self.state.pending.len();
        let ttl = self.ttl_secs;
        self.state.pending.retain(|r| now - r.created_at <= ttl);
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
        q.save(&store).unwrap();
        let mut restored = ReplyQueue::load(&store, 10, 3600).unwrap();
        assert_eq!(restored.pending_for("cc", 0).len(), 1);
        assert_eq!(push(&mut restored, 1, "cc", "next"), 2, "ids keep counting");
    }
}
