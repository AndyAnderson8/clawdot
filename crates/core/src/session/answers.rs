//! The words a user answers with: yes, no and stop, read in the language of the turn (REQ-SEC-011,
//! REQ-AST-022).

use crate::lang::Language;

/// How long a held action waits for the user's answer.
pub const CONFIRM_WINDOW_SECS: i64 = 120;

/// Whether the user's whole utterance is a plain yes. Deliberately strict: a confirmation must be the
/// user's own short answer, not a sentence that merely contains "yes".
#[must_use]
pub fn is_yes(lang: &Language, utterance: &str) -> bool {
    lang.answers
        .yes
        .contains(&normalise(lang, utterance).as_str())
}

/// Whether the user's whole utterance is a plain no or cancel.
#[must_use]
pub fn is_no(lang: &Language, utterance: &str) -> bool {
    lang.answers
        .no
        .contains(&normalise(lang, utterance).as_str())
}

pub(super) fn normalise(lang: &Language, utterance: &str) -> String {
    let norm: String = utterance
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '\'' {
                c
            } else {
                ' '
            }
        })
        .collect();
    norm.split_whitespace()
        .filter(|w| !lang.answers.ignored.contains(w))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether the user's whole utterance asks to stop what is happening (REQ-AST-022, REQ-VOI-020).
#[must_use]
pub fn is_cancel(lang: &Language, utterance: &str) -> bool {
    lang.answers
        .cancel
        .contains(&normalise(lang, utterance).as_str())
}
