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
    /// Tools, connections, and skills.
    Ext(crate::extensions::ExtOp),
    /// Apply the effective values of the settings that change without a restart (REQ-CFG-039).
    Settings(serde_json::Map<String, Value>),
    /// The language-model key was stored, replaced or removed: build the client again with it.
    Rekey,
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

    /// Hand over the audit log the portal reads (REQ-SEC-014). Only the first call has an effect.
    pub fn set_audit(&self, log: std::sync::Arc<crate::audit::AuditLog>) {
        let _ = self.audit.set(log);
    }

    /// The audit log, if the assistant has one.
    #[must_use]
    pub fn audit(&self) -> Option<std::sync::Arc<crate::audit::AuditLog>> {
        self.audit.get().cloned()
    }

    /// Hand over the notes the portal lists and clears (REQ-CFG-044). Only the first call has an effect.
    pub fn set_notes(&self, notes: std::sync::Arc<crate::notes::NoteStore>) {
        let _ = self.notes.set(notes);
    }

    /// The notes, if the assistant keeps any.
    #[must_use]
    pub fn notes(&self) -> Option<std::sync::Arc<crate::notes::NoteStore>> {
        self.notes.get().cloned()
    }

    /// Hand over the settings state (all layers and the saved file). Nothing in it is a secret.
    pub fn set_settings(&self, state: crate::settings::SettingsState) {
        *self.settings.lock().unwrap_or_else(PoisonError::into_inner) = Some(state);
    }

    /// What the Settings tab shows (`null` before the settings are handed over).
    #[must_use]
    pub fn ui_settings(&self) -> Value {
        self.settings
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map_or(Value::Null, crate::settings::SettingsState::json)
    }

    /// The effective value of one setting (`null` when unset or before the settings are handed over).
    #[must_use]
    pub fn setting(&self, key: &str) -> Value {
        self.settings
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map_or(Value::Null, |s| s.get(key))
    }

    /// The saved settings as an export file (`null` before the settings are handed over).
    #[must_use]
    pub fn export_settings(&self) -> Value {
        self.settings
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map_or(Value::Null, crate::settings::SettingsState::export)
    }

    /// Validate and save a change to settings (REQ-CFG-040). Returns the new listing and the values the
    /// main loop must apply now.
    ///
    /// # Errors
    /// [`Fail::Invalid`] with a message that says what to fix; [`Fail::Unavailable`] before settings exist.
    pub fn change_settings(
        &self,
        patch: &Value,
    ) -> Result<(Value, serde_json::Map<String, Value>), Fail> {
        let mut guard = self.settings.lock().unwrap_or_else(PoisonError::into_inner);
        let st = guard
            .as_mut()
            .ok_or_else(|| Fail::Unavailable("Settings are not available yet.".into()))?;
        st.change(patch).map_err(Fail::Invalid)?;
        Ok((st.json(), st.live_values()))
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
