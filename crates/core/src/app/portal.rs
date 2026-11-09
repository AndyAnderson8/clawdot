//! What the web portal needs from the core beyond the inbox: published listings (tools, settings) and a
//! request channel into the main loop, which owns the scheduler and the tool sets (REQ-CFG-027, 029, 030).

use super::Core;
use serde_json::Value;
use std::sync::PoisonError;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

/// How long a portal request waits for the main loop. The loop is busy while the assistant is mid-answer.
const CALL_TIMEOUT: Duration = Duration::from_secs(5);

/// A change to timers, alarms, and reminders asked for from the portal.
#[derive(Debug, Clone, PartialEq)]
pub enum SchedOp {
    /// List everything.
    List,
    /// Add one; the body describes it (see `assistant::schedule_api`).
    Add(Value),
    /// Replace item `id` with the described one.
    Edit(u64, Value),
    /// Cancel item `id`.
    Cancel(u64),
}

/// Why a portal request failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fail {
    /// There is no such item.
    NotFound(String),
    /// The request is not acceptable; the message says how to fix it.
    Invalid(String),
    /// The assistant could not take the request now.
    Unavailable(String),
}

impl std::fmt::Display for Fail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (Self::NotFound(m) | Self::Invalid(m) | Self::Unavailable(m)) = self;
        f.write_str(m)
    }
}

/// What the main loop is asked to do.
#[derive(Debug)]
pub enum Op {
    /// Timers, alarms, and reminders.
    Sched(SchedOp),
}

/// One request and where to send the answer.
#[derive(Debug)]
pub struct Job {
    /// The request.
    pub op: Op,
    /// The answer: a JSON body for the page.
    pub reply: oneshot::Sender<Result<Value, Fail>>,
}

impl Core {
    /// Publish the tool listing the Tools and Connections tabs show (replaces the previous one).
    pub fn set_ui_tools(&self, tools: Value) {
        *self.tools.lock().unwrap_or_else(PoisonError::into_inner) = tools;
    }

    /// The latest tool listing (`null` before the first).
    #[must_use]
    pub fn ui_tools(&self) -> Value {
        self.tools
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Publish the settings the device is running with. Never put a secret in it.
    pub fn set_ui_settings(&self, settings: Value) {
        *self.settings.lock().unwrap_or_else(PoisonError::into_inner) = settings;
    }

    /// The settings the device is running with (`null` before they are published).
    #[must_use]
    pub fn ui_settings(&self) -> Value {
        self.settings
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Open the request channel. The main loop owns the returned receiver and answers each [`Job`].
    #[must_use]
    pub fn attach_portal(&self) -> mpsc::Receiver<Job> {
        let (tx, rx) = mpsc::channel(8);
        *self.jobs.lock().unwrap_or_else(PoisonError::into_inner) = Some(tx);
        rx
    }

    /// Ask the main loop to do something and wait for the answer.
    ///
    /// # Errors
    /// [`Fail::Unavailable`] when nothing is listening or the assistant is too busy; otherwise whatever the
    /// handler refused with.
    pub async fn portal_call(&self, op: Op) -> Result<Value, Fail> {
        let tx = self
            .jobs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .ok_or_else(|| Fail::Unavailable("The assistant is not running yet.".into()))?;
        let (reply, answer) = oneshot::channel();
        tx.send(Job { op, reply })
            .await
            .map_err(|_| Fail::Unavailable("The assistant is not running.".into()))?;
        match tokio::time::timeout(CALL_TIMEOUT, answer).await {
            Ok(Ok(r)) => r,
            _ => Err(Fail::Unavailable(
                "The assistant is busy with a conversation. Try again in a few seconds.".into(),
            )),
        }
    }
}
