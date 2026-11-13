//! Local fast-path intent matcher (REQ-AST-010, REQ-NFR-111).
//!
//! Simple commands ("set a timer for ten minutes", "volume up", "stop") are recognised here, without an
//! LLM round trip, so they are instant, free, and work offline. Anything not recognised falls through
//! to the LLM. The matcher is authoritative for its own grammar and bounded in time: matching is a
//! backtracking search with a hard step budget, so a pathological utterance can never stall the device.
//!
//! Templates use a small hassil-style syntax:
//!
//! * `word`          a literal word
//! * `(a|b c)`       alternatives
//! * `[a b]`         optional
//! * `{name}`        a slot; `{name:kind}` picks the kind (`text`, `number`, `duration`, `time`),
//!   and a bare name that is itself a kind uses that kind
//! * `word[s]`       a word with an optional glued suffix, meaning "word" or "words"

mod grammar;
mod intent;
mod matcher;
mod types;
mod values;

pub use grammar::GrammarError;
pub use intent::Intent;
pub use matcher::Matcher;
pub use types::{Match, Meridiem, SlotValue, Time};
pub use values::normalize;

#[cfg(test)]
mod tests;
