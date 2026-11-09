//! A cooperative cancel signal for one turn (REQ-AST-022).
//!
//! The satellite session holds one token per running turn and sets it when the user says stop (or presses the
//! button); the session loop looks at it between rounds, before each call, and while it waits for the model or
//! a tool. Cloning shares the signal. Setting it twice is harmless.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Notify;

#[derive(Debug, Default)]
struct Inner {
    flag: AtomicBool,
    wake: Notify,
}

/// A signal that a turn should stop.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<Inner>);

impl CancelToken {
    /// A token that has not been set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Ask the turn to stop.
    pub fn cancel(&self) {
        self.0.flag.store(true, Ordering::SeqCst);
        self.0.wake.notify_waiters();
    }

    /// Whether the turn was asked to stop.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.flag.load(Ordering::SeqCst)
    }

    /// Completes once the token is set (immediately if it already is).
    pub async fn cancelled(&self) {
        loop {
            let waiting = self.0.wake.notified();
            tokio::pin!(waiting);
            // Register before looking at the flag so a cancel between the two is not missed.
            waiting.as_mut().enable();
            if self.is_cancelled() {
                return;
            }
            waiting.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test(flavor = "current_thread")]
    async fn a_set_token_is_seen_by_every_clone_and_wakes_a_waiter() {
        let t = CancelToken::new();
        let c = t.clone();
        assert!(!c.is_cancelled());
        let waiter = tokio::spawn({
            let c = c.clone();
            async move { c.cancelled().await }
        });
        tokio::time::sleep(Duration::from_millis(5)).await;
        t.cancel();
        t.cancel();
        assert!(c.is_cancelled());
        tokio::time::timeout(Duration::from_secs(2), waiter)
            .await
            .unwrap()
            .unwrap();
        // Already set: completes at once.
        tokio::time::timeout(Duration::from_secs(2), c.cancelled())
            .await
            .unwrap();
    }
}
