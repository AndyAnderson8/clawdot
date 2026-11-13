//! The gate: everything the host decides about a turn that the model must not be able to change (REQ-SEC-010
//! to REQ-SEC-016, REQ-AST-019, REQ-AST-022, REQ-AST-025). The tier of each call, holding a risky one and
//! asking the user, releasing it only on the user's own yes, the taint rule, dry-run and the daily caps, the
//! audit record, the turn limits and cancel.
//!
//! A turn takes its calls from a model and hands them to [`Turn::dispatch`]. The realtime voice path
//! (REQ-RTV-003) has a model of its own that makes the function calls, so it calls the same gate directly
//! ([`run_proposed_calls`], [`answer_held_action`]) and no second model request is made on its behalf.

use super::*;
use crate::audit::{Act, Gate, TurnLog, decision};
use crate::llm::{ChatRequest, Completion, LlmError, Message, ToolCall, ToolSpec};
use crate::notes;
use crate::sched::Ts;
use crate::usage::{LimitReached, UsageTracker};
use serde_json::Value;
use std::time::Duration;

mod admit;
mod release;
mod rounds;
mod run;
mod settle;
mod sort;

/// One turn in progress: the pieces the gate needs, with the host's bookkeeping. Its state is private to this
/// module and the model loop in `rounds`; `run_turn_from` only opens a turn and runs it.
pub(super) struct Turn<'a, H> {
    ctl: &'a TurnCtl,
    host: &'a mut H,
    conv: &'a mut Conversation,
    usage: &'a UsageTracker,
    cfg: TurnConfig,
    turn: Option<TurnLog>,
    st: TurnState,
    result: TurnResult,
    by_user: bool,
    day: i64,
    now: Ts,
    /// Monotonic seconds: every window and idle time is measured on this, never on `now` (a wall-clock step
    /// must not lengthen or shorten a confirmation window).
    mono: i64,
}

/// An audit record for a call that was refused before it had a tier or arguments worth recording.
fn invalid<'a>(tool: &'a str, why: &'static str) -> Act<'a> {
    static NO_ARGS: Value = Value::Null;
    Act {
        tool,
        tier: None,
        args: &NO_ARGS,
        decision: decision::INVALID,
        held_id: None,
        outcome: None,
        note: Some(why),
    }
}

/// What dispatching one reply's calls left for the turn to settle.
#[derive(Default)]
struct Dispatched {
    held: Option<PendingAction>,
    ask: Option<String>,
    limit_hit: Option<Stop>,
    cancelled: bool,
    sent: Option<String>,
    not_run: Vec<String>,
}

/// One call as the gate runs or blocks it: which tool, with what arguments, at what tier, and the audit id of
/// the held record when the user's yes released it.
#[derive(Clone, Copy)]
struct Call<'a> {
    name: &'a str,
    args: &'a Value,
    tier: Tier,
    held_id: Option<u64>,
}

/// How an action that waits for the user is recorded: the audit decision and its note, its line in the
/// recent-actions list, and what the model is told.
struct Kept {
    decision: &'static str,
    why: &'static str,
    recent: &'static str,
    told: &'static str,
}

/// A costly or destructive call in a turn the user started: held, and the user is asked.
const HOLD: Kept = Kept {
    decision: decision::HELD,
    why: "asked the user to confirm",
    recent: "waiting for the user's yes",
    told: "Not run. The user is being asked to confirm; the answer arrives in their next turn.",
};

/// A call in a turn the user did not start: kept as a proposal for the user's next turn.
const PROPOSE: Kept = Kept {
    decision: decision::PROPOSED,
    why: "turn not started by the user",
    recent: "proposed, waiting for the user",
    told: "Not run: this turn was not started by the user, so it can only read. The action was saved as a proposal and the user will be asked.",
};

/// Take the result out of a turn that is ending, leaving a blank one behind.
fn take(result: &mut TurnResult) -> TurnResult {
    std::mem::take(result)
}

/// A model that is never asked: the gate needs a backend type only for the summaries of long results, and a
/// caller that turns those off (the realtime path) has none.
#[derive(Debug, Clone, Copy)]
pub(super) struct NoModel;

impl ChatBackend for NoModel {
    async fn complete(&self, _req: &ChatRequest) -> Result<Completion, LlmError> {
        Err(LlmError::Transport("no model on this path".into()))
    }
}

impl<'a, H: ToolHost> Turn<'a, H> {
    /// Open a turn: the audit record, the idle expiry, the host told the user's words, and the words added to
    /// the history (an event's text is framed as untrusted data instead).
    pub(super) fn begin(
        ctl: &'a TurnCtl,
        trigger: Trigger,
        label: &str,
        inputs: TurnInputs<'a, H>,
        user_text: &str,
    ) -> Self {
        let TurnInputs {
            host,
            conv,
            cfg,
            usage,
            day,
            now,
            mono,
        } = inputs;
        let cfg = cfg.clamped();
        let turn: Option<TurnLog> = conv
            .audit
            .as_ref()
            .map(|log| TurnLog::begin(log, label, day, now, usage.spend_today()));
        if let Some(dropped) = conv.expire_if_idle(mono) {
            note_resolved(
                turn.as_ref(),
                usage,
                &dropped,
                decision::EXPIRED,
                "conversation expired",
            );
        }
        let by_user = trigger == Trigger::User;
        if by_user {
            host.begin_turn(user_text);
            conv.history.push(Message::user(user_text));
        } else {
            conv.history.push(Message::user(framed_event(user_text)));
            conv.tainted = true;
            if let Some(old) = conv
                .pending
                .take_if(|p| mono - p.proposed_at > CONFIRM_WINDOW_SECS)
            {
                note_resolved(
                    turn.as_ref(),
                    usage,
                    &old,
                    decision::EXPIRED,
                    "no answer in time",
                );
            }
        }
        Self {
            ctl,
            host,
            conv,
            usage,
            cfg,
            turn,
            st: TurnState::new(ctl.now()),
            result: TurnResult::default(),
            by_user,
            day,
            now,
            mono,
        }
    }

    /// Record how a held or proposed action ended without running.
    fn resolved(&self, p: &PendingAction, how: &str, why: &str) {
        note_resolved(self.turn.as_ref(), self.usage, p, how, why);
    }

    /// Write one audit record for this turn, if the conversation has a log.
    fn note_act(&self, a: &Act<'_>) -> Option<u64> {
        note(self.turn.as_ref(), self.usage, a)
    }
}

/// What one reply's calls share while they go through the gate: the model that may summarise long results,
/// the tools on offer, and whether enough time has passed since the last spoken phrase for another.
struct Batch<'c, B> {
    backend: Option<&'c B>,
    tools: &'c [ToolSpec],
    spaced: bool,
}

impl<B> Batch<'_, B> {
    /// The offered tool of this name, if there is one.
    fn spec(&self, name: &str) -> Option<&ToolSpec> {
        self.tools.iter().find(|t| t.name == name)
    }
}

/// The common opening of the two realtime entry points: a voice turn for a model outside the turn loop
/// (REQ-RTV-003), with the summaries of long results off. Without `calls`, the words are read as the answer
/// to a question about forgetting notes or to a held action, and the turn ends there. With `calls`, the
/// words are not read as an answer; the calls are run, and a held action stays held (REQ-SEC-011).
async fn voice_turn<H: ToolHost>(
    inputs: TurnInputs<'_, H>,
    user_text: &str,
    calls: Option<&[ToolCall]>,
) -> TurnResult {
    let ctl = TurnCtl::default();
    let cfg = TurnConfig {
        summarize_over: 0,
        ..inputs.cfg.clone()
    };
    let inputs = TurnInputs {
        cfg: &cfg,
        ..inputs
    };
    let mut t = Turn::begin(&ctl, Trigger::User, "voice", inputs, user_text);
    if calls.is_some() {
        // A model's calls are never an answer. `user_text` here only names the turn (it scopes the tools),
        // and may be the words of an earlier utterance, so it must not be read as a yes or a no. A held
        // action stays until the user's own words answer it (`answer_held_action`) or its window ends.
        t.drop_held_if_stale();
    } else if let Some(r) = t.answer_first(user_text).await {
        return r;
    }
    if let Some(calls) = calls {
        let mut tools = offered(&*t.host, &t.cfg);
        tools.extend(pseudo_tools(t.conv, t.by_user));
        // The calls enter the history as a model reply would have put them, so every call has its result.
        t.conv
            .history
            .push(Message::assistant_calls(String::new(), calls.to_vec()));
        let d = t.dispatch(None::<&NoModel>, calls, &tools, true).await;
        if let Some(r) = t.settle(d) {
            return r;
        }
    }
    t.close_after_calls()
}

/// Run calls that a model outside the turn loop proposed (the realtime voice model, REQ-RTV-003) through
/// the gate exactly as a turn's own calls: tier, hold, taint, dry-run, caps, audit, limits. No model request
/// is made and none is counted. `inputs.cfg` is used as given, with the summaries of long results off.
pub async fn run_proposed_calls<H: ToolHost>(
    inputs: TurnInputs<'_, H>,
    user_text: &str,
    calls: &[ToolCall],
) -> TurnResult {
    voice_turn(inputs, user_text, Some(calls)).await
}

/// Read the user's own words as the answer to a held action (REQ-SEC-011) for a caller whose model is not
/// ours (the realtime voice): a yes runs the stored action, a no or anything else drops it. Nothing is sent
/// to any model. When nothing was held, the result is the plain "Done." that closes any turn without calls.
pub async fn answer_held_action<H: ToolHost>(
    inputs: TurnInputs<'_, H>,
    user_text: &str,
) -> TurnResult {
    voice_turn(inputs, user_text, None).await
}
