//! One conversation for each speaker (REQ-AST-026, finding C2 of the architecture review).
//!
//! A conversation holds the history the model sees, the recent actions, the stored long results and, above
//! all, the action the host is holding until its speaker says yes (REQ-SEC-011). Sharing one conversation
//! between rooms would let a "yes" in the kitchen release the action asked about in the bedroom, so each
//! speaker has its own, named by who is talking: a satellite connection (`sat:<id>`), the console
//! (`console`), or an inbox or bridge source that converses (`src:<name>`).
//!
//! The set is bounded so memory stays inside the resource budget (spec 0002): at most [`MAX_SPEAKERS`]
//! conversations exist at once, one that has been idle longer than the conversation's own idle time is
//! dropped (it would have forgotten everything by then anyway), and a speaker that arrives when the set is
//! full takes the place of the one idle the longest. A speaker whose turn is running has no conversation
//! in the set (the turn task holds it) and cannot be dropped; what it says meanwhile waits, at most
//! [`MAX_WAITING`] utterances, until the turn hands the conversation back.

use crate::session::{CONVERSATION_IDLE_SECS, Conversation};
use std::collections::{HashMap, VecDeque};

/// Most conversations kept at once. Each is bounded by [`crate::session::CONVERSATION_MAX_MESSAGES`] messages
/// and [`crate::session::CONVERSATION_MAX_BYTES`] bytes of history, so the set stays under a megabyte.
pub const MAX_SPEAKERS: usize = 8;
/// Most utterances held for a speaker whose turn is still running. More are refused.
pub const MAX_WAITING: usize = 2;
/// Seconds without activity after which an idle conversation is dropped: a conversation's own idle time.
pub const IDLE_SECS: i64 = CONVERSATION_IDLE_SECS;
// Every `now` taken below is a monotonic reading ([`crate::sched::Clock::monotonic_secs`]), not the wall clock:
// idle time is a duration, and a step of the wall clock must not keep a conversation alive or drop it.

/// Who is talking.
pub type SpeakerId = String;

struct Slot<Q> {
    /// `None` while a turn holds it.
    conv: Option<Conversation>,
    /// Monotonic seconds of the last use.
    last_used: i64,
    waiting: VecDeque<Q>,
}

/// What asking for a speaker's conversation gave.
#[derive(Debug)]
pub enum Checkout {
    /// The speaker's conversation, now out of the set until it is checked in.
    Ready(Box<Conversation>),
    /// A turn of this speaker is still running.
    Busy,
    /// Every place is taken by a speaker whose turn is running.
    Full,
}

/// The conversations, by speaker. `Q` is whatever a caller queues for a busy speaker.
pub struct Speakers<Q> {
    slots: HashMap<SpeakerId, Slot<Q>>,
    make: Box<dyn Fn() -> Conversation + Send>,
    max: usize,
}

impl<Q> std::fmt::Debug for Speakers<Q> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Speakers")
            .field("count", &self.slots.len())
            .finish_non_exhaustive()
    }
}

impl<Q> Speakers<Q> {
    /// An empty set. `make` builds a fresh conversation (persona, limits, audit log, notes).
    pub fn new(make: impl Fn() -> Conversation + Send + 'static) -> Self {
        Self::with_max(make, MAX_SPEAKERS)
    }

    /// Like [`Speakers::new`] with a different bound.
    pub fn with_max(make: impl Fn() -> Conversation + Send + 'static, max: usize) -> Self {
        Self {
            slots: HashMap::new(),
            make: Box::new(make),
            max: max.max(1),
        }
    }

    /// How many speakers have a place.
    #[must_use]
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// No speaker has a place.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Whether this speaker's turn is running.
    #[must_use]
    pub fn is_busy(&self, id: &str) -> bool {
        self.slots.get(id).is_some_and(|s| s.conv.is_none())
    }

    /// Drop every idle conversation that has been quiet for [`IDLE_SECS`].
    pub fn sweep(&mut self, now: i64) {
        self.slots
            .retain(|_, s| s.conv.is_none() || now - s.last_used <= IDLE_SECS);
    }

    /// Take a speaker's conversation for a turn, making one for a new speaker.
    pub fn checkout(&mut self, id: &str, now: i64) -> Checkout {
        self.sweep(now);
        if let Some(slot) = self.slots.get_mut(id) {
            return match slot.conv.take() {
                Some(c) => {
                    slot.last_used = now;
                    Checkout::Ready(Box::new(c))
                }
                None => Checkout::Busy,
            };
        }
        if self.slots.len() >= self.max {
            // The speaker idle the longest gives up its place.
            let oldest = self
                .slots
                .iter()
                .filter(|(_, s)| s.conv.is_some())
                .min_by_key(|(_, s)| s.last_used)
                .map(|(k, _)| k.clone());
            match oldest {
                Some(k) => {
                    self.slots.remove(&k);
                }
                None => return Checkout::Full,
            }
        }
        self.slots.insert(
            id.to_owned(),
            Slot {
                conv: None,
                last_used: now,
                waiting: VecDeque::new(),
            },
        );
        Checkout::Ready(Box::new((self.make)()))
    }

    /// Hand a conversation back when its turn ends.
    pub fn checkin(&mut self, id: &str, conv: Conversation, now: i64) {
        if let Some(slot) = self.slots.get_mut(id) {
            slot.conv = Some(conv);
            slot.last_used = now;
        }
    }

    /// A turn ended without handing its conversation back (the task panicked): start the speaker afresh.
    /// What was waiting for it is kept.
    pub fn reset(&mut self, id: &str, now: i64) {
        if let Some(slot) = self.slots.get_mut(id) {
            slot.conv = Some((self.make)());
            slot.last_used = now;
        }
    }

    /// Hold something for a speaker whose turn is running.
    ///
    /// # Errors
    /// The item itself, when the speaker has no turn running or is already holding [`MAX_WAITING`] things.
    pub fn defer(&mut self, id: &str, item: Q) -> Result<(), Q> {
        match self.slots.get_mut(id) {
            Some(s) if s.conv.is_none() && s.waiting.len() < MAX_WAITING => {
                s.waiting.push_back(item);
                Ok(())
            }
            _ => Err(item),
        }
    }

    /// The oldest thing held for a speaker, now that its turn has ended.
    pub fn next_deferred(&mut self, id: &str) -> Option<Q> {
        self.slots.get_mut(id).and_then(|s| s.waiting.pop_front())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn set(max: usize) -> Speakers<String> {
        Speakers::with_max(|| Conversation::standard("system"), max)
    }

    fn take(s: &mut Speakers<String>, id: &str, now: i64) -> Conversation {
        match s.checkout(id, now) {
            Checkout::Ready(c) => *c,
            other => panic!("expected a conversation for {id}, got {other:?}"),
        }
    }

    // REQ-AST-026
    #[test]
    fn each_speaker_has_its_own_conversation() {
        let mut s = set(4);
        let a = take(&mut s, "sat:a", 1);
        let b = take(&mut s, "sat:b", 1);
        assert!(a.history().is_empty() && b.history().is_empty());
        assert!(s.is_busy("sat:a") && s.is_busy("sat:b"));
        assert_eq!(s.len(), 2);
        s.checkin("sat:a", a, 2);
        assert!(!s.is_busy("sat:a") && s.is_busy("sat:b"));
        assert!(matches!(s.checkout("sat:b", 2), Checkout::Busy));
    }

    #[test]
    fn a_busy_speaker_has_a_short_queue_and_gets_it_back_in_order() {
        let mut s = set(4);
        let c = take(&mut s, "console", 1);
        assert!(s.defer("console", "one".into()).is_ok());
        assert!(s.defer("console", "two".into()).is_ok());
        assert_eq!(
            s.defer("console", "three".into()),
            Err("three".to_owned()),
            "the queue is bounded"
        );
        assert!(
            s.defer("nobody", "x".into()).is_err(),
            "nothing waits for a speaker with no turn"
        );
        s.checkin("console", c, 2);
        assert_eq!(s.next_deferred("console").as_deref(), Some("one"));
        assert_eq!(s.next_deferred("console").as_deref(), Some("two"));
        assert_eq!(s.next_deferred("console"), None);
    }

    #[test]
    fn the_set_is_bounded_and_the_longest_idle_speaker_gives_up_its_place() {
        let mut s = set(2);
        let a = take(&mut s, "a", 10);
        let b = take(&mut s, "b", 20);
        s.checkin("a", a, 11);
        s.checkin("b", b, 21);
        let c = take(&mut s, "c", 30);
        assert_eq!(s.len(), 2, "never more than the bound");
        assert!(!s.is_busy("b"), "b kept its place");
        assert!(s.checkout("a", 31).is_ready_new(), "a was the one dropped");
        drop(c);
    }

    #[test]
    fn when_every_place_has_a_running_turn_a_new_speaker_is_refused() {
        let mut s = set(2);
        let _a = take(&mut s, "a", 1);
        let _b = take(&mut s, "b", 1);
        assert!(matches!(s.checkout("c", 2), Checkout::Full));
        assert_eq!(s.len(), 2);
    }

    #[test]
    fn an_idle_conversation_is_dropped_after_its_idle_time_but_a_running_turn_is_not() {
        let mut s = set(4);
        let a = take(&mut s, "a", 100);
        let _b = take(&mut s, "b", 100);
        s.checkin("a", a, 100);
        s.sweep(100 + IDLE_SECS + 1);
        assert_eq!(s.len(), 1, "a went, b is running");
        assert!(s.is_busy("b"));
    }

    #[test]
    fn a_lost_conversation_is_replaced_and_the_queue_survives() {
        let mut s = set(4);
        let _gone = take(&mut s, "a", 1);
        assert!(s.defer("a", "later".into()).is_ok());
        s.reset("a", 2);
        assert!(!s.is_busy("a"));
        assert_eq!(s.next_deferred("a").as_deref(), Some("later"));
    }

    impl Checkout {
        fn is_ready_new(&self) -> bool {
            matches!(self, Checkout::Ready(c) if c.history().is_empty())
        }
    }
}
