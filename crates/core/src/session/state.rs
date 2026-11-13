//! What the host tracks while a turn runs, and the helpers that end one.

use super::*;
use crate::llm::Message;
use crate::sched::Ts;
use std::time::Duration;

/// Calls that fail in a row before the host ends the turn (REQ-AST-025).
pub(super) const MAX_FAILURES_IN_ROW: usize = 2;
/// Seconds between two host phrases (REQ-AST-020).
pub(super) const PROGRESS_GAP_SECS: Ts = 10;

/// How one step of a turn ended, for the spoken account of where the turn got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum How {
    Done,
    Failed,
    TimedOut,
    Asked,
}

/// What the host tracks while a turn runs.
#[derive(Debug)]
pub(super) struct TurnState {
    pub(super) started: Ts,
    pub(super) steps: Vec<(String, How)>,
    pub(super) fail_streak: usize,
    pub(super) last_fail: Option<(String, How, Tier)>,
    /// The tool of a call refused because its read-back was too long to say, until something else is held or
    /// the same tool runs.
    pub(super) too_long: Option<String>,
    pub(super) last_spoke: Option<Ts>,
    pub(super) spoke_round: bool,
}

impl TurnState {
    pub(super) fn new(started: Ts) -> Self {
        Self {
            started,
            steps: Vec::new(),
            fail_streak: 0,
            last_fail: None,
            too_long: None,
            last_spoke: None,
            spoke_round: false,
        }
    }

    pub(super) fn over_deadline(&self, ctl: &TurnCtl, cfg: &TurnConfig) -> bool {
        ctl.now() - self.started >= i64::try_from(cfg.deadline_secs).unwrap_or(i64::MAX)
    }

    /// How long the next tool call may take: the call timeout, but never past the deadline.
    pub(super) fn call_budget(&self, ctl: &TurnCtl, cfg: &TurnConfig) -> Duration {
        let left = i64::try_from(cfg.deadline_secs)
            .unwrap_or(i64::MAX)
            .saturating_sub(ctl.now() - self.started);
        let left = Duration::from_secs(u64::try_from(left).unwrap_or(0).max(1));
        cfg.tool_timeout.min(left)
    }

    pub(super) fn record(&mut self, tool: &str, how: How, tier: Tier) {
        self.steps.push((tool.to_owned(), how));
        if matches!(how, How::Failed | How::TimedOut) {
            self.fail_streak += 1;
            self.last_fail = Some((tool.to_owned(), how, tier));
        } else {
            self.fail_streak = 0;
            self.last_fail = None;
            if how == How::Done && self.too_long.as_deref() == Some(tool) {
                self.too_long = None;
            }
        }
    }
}

/// Say `text` now: send it to the speech sink and remember it for the caller (REQ-AST-020).
pub(super) fn say_early(ctl: &TurnCtl, result: &mut TurnResult, st: &mut TurnState, text: String) {
    if let Some(tx) = &ctl.speak {
        let _ = tx.send(text.clone());
    }
    result.early.push(text);
    st.last_spoke = Some(ctl.now());
    st.spoke_round = true;
}

/// End a turn at a limit and say which one (REQ-AST-019).
pub(super) fn stopped_turn(
    conv: &mut Conversation,
    result: TurnResult,
    why: Stop,
    cfg: &TurnConfig,
    st: &TurnState,
    not_run: &[String],
) -> TurnResult {
    let mut r = result;
    r.end = TurnEnd::Stopped(why);
    let speech = stop_speech(why, cfg, &st.steps, not_run, None);
    finish_with(conv, r, &speech)
}

/// End a turn because calls keep failing (REQ-AST-025).
pub(super) fn failed_turn(
    conv: &mut Conversation,
    result: TurnResult,
    st: &TurnState,
) -> TurnResult {
    let mut r = result;
    r.end = TurnEnd::Stopped(Stop::Failures);
    let speech = match &st.last_fail {
        Some((tool, how, tier)) => failure_speech(tool, *how, *tier, st.fail_streak.max(1)),
        None => {
            "Something went wrong and I could not finish. You can ask me to try again.".to_owned()
        }
    };
    finish_with(conv, r, &speech)
}

/// How a finished call is described in the recent-actions list.
pub(super) fn status_of(kind: Kind) -> &'static str {
    match kind {
        Kind::Ok => "ran",
        Kind::Failed => "failed",
        Kind::TimedOut => "timed out",
        Kind::Cancelled => "cancelled",
        Kind::NotRun => "not run",
    }
}

/// How a call ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Ok,
    Failed,
    TimedOut,
    Cancelled,
    NotRun,
}

/// What a call produced, for the model and for the turn's own bookkeeping.
#[derive(Debug)]
pub(super) struct Executed {
    pub(super) text: String,
    pub(super) kind: Kind,
    /// A costly or destructive call went out and the user said stop meanwhile: it cannot be recalled.
    pub(super) sent: bool,
}

/// End a turn with words the host chose, keeping the history well formed.
pub(super) fn finish_with(
    conv: &mut Conversation,
    mut result: TurnResult,
    speech: &str,
) -> TurnResult {
    speech.clone_into(&mut result.speech);
    conv.history.push(Message::assistant(speech));
    conv.trim();
    result
}
