//! Turn settings and inputs, how a turn ends, and the entry points that start one (REQ-AST-006,
//! REQ-AST-019, REQ-AST-020, REQ-AST-022).

use super::*;
use crate::cancel::CancelToken;
use crate::lang::Language;
use crate::llm::LlmError;
use crate::sched::{Clock, Ts};
use crate::usage::{LimitReached, UsageTracker};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;

/// Never more model rounds than this in one turn, whatever the configuration says (REQ-AST-019).
pub const MAX_ROUNDS_CEILING: usize = 20;
/// Never more tool calls than this in one turn.
pub const MAX_CALLS_CEILING: usize = 20;
/// Never a longer deadline than this, in seconds.
pub const MAX_DEADLINE_CEILING_SECS: u64 = 300;

/// Turn settings.
#[derive(Debug, Clone)]
pub struct TurnConfig {
    /// Primary model.
    pub model: String,
    /// Model to try if the primary fails (REQ-AST-009).
    pub fallback_model: Option<String>,
    /// Cap on generated tokens.
    pub max_tokens: u32,
    /// Most model rounds per turn (REQ-AST-006, REQ-AST-019). At most [`MAX_ROUNDS_CEILING`].
    pub max_tool_loops: usize,
    /// Most tool calls per turn (REQ-AST-019). At most [`MAX_CALLS_CEILING`].
    pub max_tool_calls: usize,
    /// Wall-clock seconds a turn may take (REQ-AST-019). At most [`MAX_DEADLINE_CEILING_SECS`].
    pub deadline_secs: u64,
    /// Longest one tool call may take; a timeout is an error result (REQ-AST-019).
    pub tool_timeout: Duration,
    /// Say the plan before a multi-step request (REQ-AST-020).
    pub plan_speech: bool,
    /// Say short progress phrases while tools run (REQ-AST-020).
    pub progress_speech: bool,
    /// Summarise tool results longer than this many characters; 0 turns it off (REQ-AST-024).
    pub summarize_over: usize,
    /// Model for those summaries; the primary model when unset.
    pub summary_model: Option<String>,
    /// Whether destructive tools are offered at all (REQ-SEC-010). Off unless the administrator enables it.
    pub allow_destructive: bool,
    /// The language the user's words are read in: yes, no, note commands (REQ-SEC-011, REQ-SEC-017).
    pub lang: &'static Language,
}

impl Default for TurnConfig {
    fn default() -> Self {
        Self {
            model: String::new(),
            fallback_model: None,
            max_tokens: 300,
            max_tool_loops: 8,
            max_tool_calls: 12,
            deadline_secs: 60,
            tool_timeout: Duration::from_secs(45),
            plan_speech: true,
            progress_speech: true,
            summarize_over: crate::audit::LONG_RESULT_CHARS,
            summary_model: None,
            allow_destructive: false,
            lang: crate::lang::DEFAULT,
        }
    }
}

impl TurnConfig {
    /// The same settings held to their ceilings and floors (REQ-AST-019): nothing a configuration file or a
    /// setting says can raise a limit above the ceiling or turn one into zero.
    #[must_use]
    pub fn clamped(&self) -> Self {
        let mut c = self.clone();
        c.max_tool_loops = c.max_tool_loops.clamp(1, MAX_ROUNDS_CEILING);
        c.max_tool_calls = c.max_tool_calls.clamp(1, MAX_CALLS_CEILING);
        c.deadline_secs = c.deadline_secs.clamp(5, MAX_DEADLINE_CEILING_SECS);
        c.tool_timeout = c
            .tool_timeout
            .clamp(Duration::from_millis(1), Duration::from_secs(120));
        c
    }
}

/// Which limit ended a turn early.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// The round limit.
    Rounds,
    /// The tool-call limit.
    Calls,
    /// The wall-clock deadline.
    Deadline,
    /// The user said stop.
    Cancelled,
    /// Calls kept failing.
    Failures,
}

/// How a turn ended.
#[derive(Debug, Clone, PartialEq)]
pub enum TurnEnd {
    /// The model answered.
    Answered,
    /// The assistant asked a clarifying question; the next user turn answers it (REQ-AST-021).
    Asked,
    /// The host ended the turn early and said why (REQ-AST-019, 022, 025).
    Stopped(Stop),
    /// A usage limit stopped the turn.
    Limit(LimitReached),
    /// Every model attempt failed.
    Failed(LlmError),
}

/// What to say, and how the turn went.
#[derive(Debug, Clone, PartialEq)]
pub struct TurnResult {
    /// Text to speak once the turn is over.
    pub speech: String,
    /// How it ended.
    pub end: TurnEnd,
    /// Tool calls made.
    pub tool_calls: usize,
    /// Model requests made.
    pub model_requests: u32,
    /// What was said while the turn ran (plan and progress), in order. Already sent to the speech sink if the
    /// caller gave one; a caller without a sink should speak these before `speech`.
    pub early: Vec<String>,
    /// The turn ended with a question for the user (a clarifying question or a held action), so the follow-up
    /// window should be longer (REQ-AST-021).
    pub expects_reply: bool,
    /// The turn held an action and asked the user about it: the answer is still awaited (REQ-SEC-011). Unlike
    /// `held`, which says what became of an action the user already answered, this is true on the turn that
    /// asks, so a caller never has to read the spoken question to tell.
    pub pending: bool,
    /// What became of the action held for the user's yes, when this turn answered one (REQ-SEC-011). `None` when
    /// none was held.
    pub held: Option<HeldOutcome>,
}

/// What became of a held action when the user answered it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeldOutcome {
    /// The user said yes in time and the tool ran and succeeded.
    Ran,
    /// The user said yes in time and the tool failed, timed out or was not run (dry run, a cap).
    Failed,
    /// The user said yes in time but the tool is no longer offered, so nothing ran.
    Gone,
    /// The user said no, or asked for something unrelated; the action was dropped.
    Declined,
    /// The user said yes after the confirmation window; the action was dropped.
    Expired,
}

impl Default for TurnResult {
    /// A turn that has said nothing yet: no speech, answered, no calls.
    fn default() -> Self {
        Self {
            speech: String::new(),
            end: TurnEnd::Answered,
            tool_calls: 0,
            model_requests: 0,
            early: Vec::new(),
            expects_reply: false,
            pending: false,
            held: None,
        }
    }
}

/// What a running turn can be told, and where it can speak.
#[derive(Clone, Default)]
pub struct TurnCtl {
    /// Set to stop the turn (REQ-AST-022).
    pub cancel: CancelToken,
    /// Where plan and progress phrases go as they arise (REQ-AST-020).
    pub speak: Option<UnboundedSender<String>>,
    /// The clock the deadline and phrase spacing use; a monotonic clock when unset.
    pub clock: Option<Arc<dyn Clock + Send + Sync>>,
}

impl std::fmt::Debug for TurnCtl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TurnCtl")
            .field("cancel", &self.cancel)
            .field("speak", &self.speak.is_some())
            .field("clock", &self.clock.is_some())
            .finish()
    }
}

impl TurnCtl {
    /// Seconds on the turn's monotonic clock: the one given, or the system's, so that setting the wall clock
    /// (a device with no battery clock gets its time late, REQ-TMR-009) cannot end a turn early or stretch it.
    pub(super) fn now(&self) -> i64 {
        match &self.clock {
            Some(c) => c.monotonic_secs(),
            None => crate::sched::SystemClock.monotonic_secs(),
        }
    }
}

/// What started a turn (REQ-SEC-012).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// The user spoke. Only such a turn can answer a held action.
    User,
    /// Something else: an inbox event, a schedule, a rule. Its text is not the user's words. Such a turn can
    /// read, and can propose an action for the user to approve in a later turn, but cannot act.
    Event,
}

/// What one turn works on, borrowed for its duration: the tools, the conversation, the limits, the counters,
/// and the day and time it runs at. These travel together through every entry point of a turn.
#[derive(Debug)]
pub struct TurnInputs<'a, H> {
    /// The tools the turn may call, and what the host decides about them.
    pub host: &'a mut H,
    /// The speaker's conversation.
    pub conv: &'a mut Conversation,
    /// The turn's limits and models.
    pub cfg: &'a TurnConfig,
    /// The daily request and spend counters.
    pub usage: &'a UsageTracker,
    /// The day number the counters are kept by.
    pub day: i64,
    /// The wall-clock time the turn runs at, in unix seconds: for timestamps (audit records, notes).
    pub now: Ts,
    /// The monotonic time the turn runs at ([`Clock::monotonic_secs`]): the confirmation and clarify windows
    /// and the conversation's idle time are measured on it, so a step of the wall clock cannot stretch or cut
    /// them.
    pub mono: i64,
}

/// Run one turn: the user's words in, the words to speak out. `ctl` carries a cancel token, a speech sink and
/// a clock (REQ-AST-020, REQ-AST-022); `TurnCtl::default()` has none of them.
pub async fn run_turn_with<B: ChatBackend, H: ToolHost>(
    ctl: &TurnCtl,
    backend: &B,
    inputs: TurnInputs<'_, H>,
    user_text: &str,
) -> TurnResult {
    run_turn_from(ctl, Trigger::User, "voice", backend, inputs, user_text).await
}

/// Run one turn that something other than the user's voice started (REQ-SEC-012). `text` is what the
/// trigger says (an event's summary): untrusted data, shown to the model as such, never read as a yes.
/// `source` names what started it (an endpoint, a schedule) for the audit log; it is clipped and made
/// printable there, and never trusted.
pub async fn run_event_turn_with<B: ChatBackend, H: ToolHost>(
    ctl: &TurnCtl,
    source: &str,
    backend: &B,
    inputs: TurnInputs<'_, H>,
    text: &str,
) -> TurnResult {
    let label = if source == "event" || source.is_empty() {
        "event".to_owned()
    } else {
        format!("event:{source}")
    };
    run_turn_from(ctl, Trigger::Event, &label, backend, inputs, text).await
}

pub(super) async fn run_turn_from<B: ChatBackend, H: ToolHost>(
    ctl: &TurnCtl,
    trigger: Trigger,
    label: &str,
    backend: &B,
    inputs: TurnInputs<'_, H>,
    user_text: &str,
) -> TurnResult {
    Turn::begin(ctl, trigger, label, inputs, user_text)
        .run(backend, user_text)
        .await
}
