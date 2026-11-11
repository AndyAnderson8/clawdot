//! A question an agent asks the person out loud and waits for (REQ-EXT-012, REQ-EXT-013).
//!
//! The question is an ordinary announcement that allows a reply, so the inbox rules apply to it unchanged
//! (mute, quiet hours, minimum priority, source naming, the burst limit). What differs is where the spoken
//! answer goes: when the event it answers has a waiting caller, the answer is handed to that caller and to
//! nobody else. It never reaches the reply queue for a bridge and it never touches a held action, so it cannot
//! stand in for a confirmation (REQ-SEC-011). Whatever the person says is data to the asking agent.

use super::announce::{AuditAs, Caller};
use super::{AnnounceError, Announcement, Core, HookError, HookOutcome};
use crate::audit::{Act, decision};
use serde_json::json;
use std::time::Duration;
use tokio::sync::oneshot;

/// Questions waiting for an answer at once, over all endpoints.
pub const MAX_PENDING_ASKS: usize = 4;

/// How a question ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AskEnd {
    /// The person answered; these are their words (speech-to-text, may contain errors).
    Answered(String),
    /// The person acknowledged without saying anything.
    Acknowledged,
    /// The person dismissed the question.
    Dismissed,
    /// Nobody answered in time (including: the question was never spoken).
    TimedOut,
}

/// Why a question could not be asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AskError {
    /// The endpoint already has a question waiting, or too many are waiting.
    Busy,
    /// The endpoint's filter dropped the question (`muted`, `below_priority`, `ignored_kind`).
    Filtered(&'static str),
    /// The announce path refused it.
    Announce(AnnounceError),
}

#[derive(Debug)]
pub(super) struct PendingAsk {
    slot: u64,
    endpoint: String,
    /// 0 until the event exists.
    event_id: u64,
    tx: Option<oneshot::Sender<AskEnd>>,
}

/// A question that has been queued. Dropping it before an answer arrives withdraws the question, so a caller
/// that gives up (or disconnects) never leaves it to be spoken late.
#[derive(Debug)]
pub struct AskTicket<'a> {
    core: &'a Core,
    slot: u64,
    event_id: u64,
    endpoint: String,
    rx: oneshot::Receiver<AskEnd>,
}

impl AskTicket<'_> {
    /// The endpoint the question was queued on.
    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Wait up to `limit` for the person.
    pub async fn wait(mut self, limit: Duration) -> AskEnd {
        match tokio::time::timeout(limit, &mut self.rx).await {
            Ok(Ok(end)) => end,
            _ => AskEnd::TimedOut,
        }
    }
}

impl Drop for AskTicket<'_> {
    fn drop(&mut self) {
        self.core.ask_close(self.slot, self.event_id);
    }
}

impl Core {
    fn asks(&self) -> std::sync::MutexGuard<'_, Vec<PendingAsk>> {
        self.asks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Queue a question on the endpoint that owns `token` and return a ticket to wait on.
    ///
    /// # Errors
    /// [`AskError`].
    pub(crate) fn ask_open<'a>(
        &'a self,
        token: &str,
        a: &Announcement,
        audit_as: AuditAs<'_>,
    ) -> Result<AskTicket<'a>, AskError> {
        let endpoint = self
            .lock()
            .endpoints
            .authenticate_token(token)
            .map(|e| e.name.clone())
            .ok_or(AskError::Announce(AnnounceError::Hook(
                HookError::Unauthorized,
            )))?;
        let (tx, rx) = oneshot::channel();
        let slot = {
            let mut asks = self.asks();
            if asks.len() >= MAX_PENDING_ASKS || asks.iter().any(|p| p.endpoint == endpoint) {
                return Err(AskError::Busy);
            }
            let slot = asks.iter().map(|p| p.slot).max().unwrap_or(0) + 1;
            asks.push(PendingAsk {
                slot,
                endpoint: endpoint.clone(),
                event_id: 0,
                tx: Some(tx),
            });
            slot
        };
        let release = |this: &Self| this.asks().retain(|p| p.slot != slot);
        match self.announce_as(Caller::Token(token), a, Some(audit_as)) {
            Ok((HookOutcome::Accepted(id) | HookOutcome::Coalesced(id), _)) => {
                if let Some(p) = self.asks().iter_mut().find(|p| p.slot == slot) {
                    p.event_id = id;
                }
                Ok(AskTicket {
                    core: self,
                    slot,
                    event_id: id,
                    endpoint,
                    rx,
                })
            }
            Ok((HookOutcome::Filtered(r), _)) => {
                release(self);
                Err(AskError::Filtered(super::announce::filter_label(r)))
            }
            Err(e) => {
                release(self);
                Err(AskError::Announce(e))
            }
        }
    }

    /// The caller is done: if the question is still open, withdraw it so it is not spoken or answered later.
    fn ask_close(&self, slot: u64, event_id: u64) {
        let was_open = {
            let mut asks = self.asks();
            let before = asks.len();
            asks.retain(|p| p.slot != slot);
            asks.len() != before
        };
        if was_open && event_id != 0 {
            let mut g = self.lock();
            if g.inbox.dismiss(event_id) {
                self.persist(&g);
            }
        }
    }

    /// Hand a spoken outcome to the caller waiting on this event, if there is one.
    pub(super) fn deliver_ask(&self, event_id: u64, end: AskEnd) -> bool {
        let tx = {
            let mut asks = self.asks();
            let Some(i) = asks.iter().position(|p| p.event_id == event_id) else {
                return false;
            };
            asks.remove(i).tx.take()
        };
        if let Some(tx) = tx {
            // A caller that already gave up is simply not there; the answer is dropped.
            let _ = tx.send(end);
        }
        true
    }

    /// Whether a caller is waiting on this event (tests, diagnostics).
    #[must_use]
    pub fn ask_pending(&self, event_id: u64) -> bool {
        self.asks().iter().any(|p| p.event_id == event_id)
    }

    /// Record how a question ended. The answer itself is never written to the log, only that it came.
    pub(crate) fn audit_ask_end(&self, trigger: &str, endpoint: &str, end: &AskEnd) {
        let Some(log) = self.audit() else { return };
        let now = self.clock.now();
        let (decision, note) = match end {
            AskEnd::Answered(_) => (decision::RAN, "answered"),
            AskEnd::Acknowledged => (decision::RAN, "acknowledged"),
            AskEnd::Dismissed => (decision::RAN, "dismissed"),
            AskEnd::TimedOut => (decision::EXPIRED, "no answer in time"),
        };
        let turn = log.next_turn();
        log.record(
            now,
            now.div_euclid(86_400),
            turn,
            trigger,
            None,
            &Act {
                tool: "ask_user",
                tier: None,
                args: &json!({ "endpoint": endpoint, "result": note }),
                decision,
                held_id: None,
                outcome: None,
                note: Some(note),
            },
        );
    }
}
