//! One conversational turn with the language model: bounded history, a bounded tool loop, enforced
//! confirmation for risky tools, a fallback model, and usage limits (REQ-AST-002, 003, 006..009, 011).
//!
//! The model is never trusted: tool arguments are validated, unknown tools are refused, a tool that needs
//! confirmation is *held by the host* (REQ-SEC-011): the host records the exact tool and arguments, asks the
//! user itself, and runs the stored action only when a later user turn is a plain yes. Nothing the model
//! writes, and nothing a tool returns, can release it. The loop cannot run away.
//!
//! The files by concern: `host` (tiers, risk and the traits a host implements), `answers` (yes, no and stop),
//! `conversation` (history, the held action, the question being asked), `turn` (settings, inputs, results and
//! the entry points), `state` and `speech` (what the host tracks and says during a turn), `tools` and
//! `note_commands` (what the host answers itself), and `gate` (the checks no model can change).

mod answers;
mod conversation;
mod gate;
mod host;
mod note_commands;
mod readback;
mod speech;
mod state;
mod tools;
mod turn;

pub use answers::{CONFIRM_WINDOW_SECS, is_cancel, is_no, is_yes};
pub use conversation::{
    CONVERSATION_IDLE_SECS, CONVERSATION_MAX_BYTES, CONVERSATION_MAX_MESSAGES, Conversation,
    PendingAction,
};
pub use gate::{answer_held_action, run_proposed_calls};
pub use host::{ChatBackend, Risk, Tier, ToolHost};
pub use readback::{MAX_READ_BACK_CHARS, describe_agent_text, describe_generic};
pub use tools::{ASK_USER, RECALL, RESULT_DETAILS, tools_for};
pub use turn::{
    HeldOutcome, MAX_CALLS_CEILING, MAX_DEADLINE_CEILING_SECS, MAX_ROUNDS_CEILING, Stop, Trigger,
    TurnConfig, TurnCtl, TurnEnd, TurnInputs, TurnResult, run_event_turn_with, run_turn_with,
};

// What the files of this module share with each other, and with `gate` and the tests.
use conversation::*;
use gate::Turn;
use note_commands::*;
use speech::*;
use state::*;
use tools::*;

#[cfg(test)]
mod tests;
