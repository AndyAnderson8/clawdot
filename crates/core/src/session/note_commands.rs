//! The user's note commands, answered by the host without asking the model (REQ-AST-023, REQ-SEC-017).

use super::*;
use crate::notes;
use crate::sched::Ts;

/// The user's note commands, answered by the host without asking the model (REQ-AST-023, REQ-SEC-017).
pub(super) fn run_note_command(
    conv: &mut Conversation,
    cmd: notes::Cmd,
    now: Ts,
    mono: i64,
) -> String {
    use notes::{Cmd, NoteError};
    let Some(store) = conv.notes.clone() else {
        return NoteError::Off.spoken();
    };
    match cmd {
        Cmd::Remember(text) => match store.add(&text, now) {
            Ok(n) => format!(
                "Okay, I'll remember that: {}.",
                n.text.trim_end_matches('.')
            ),
            Err(e) => e.spoken(),
        },
        Cmd::List => spoken_list(&store),
        Cmd::ForgetNth(n) => match store.forget_nth(n) {
            Ok(note) => format!(
                "Okay, I forgot note {n}: {}.",
                note.text.trim_end_matches('.')
            ),
            Err(e) => e.spoken(),
        },
        Cmd::ForgetText(t) => match store.forget_matching(&t) {
            Ok(note) => format!("Okay, I forgot: {}.", note.text.trim_end_matches('.')),
            Err(e) => e.spoken(),
        },
        Cmd::ForgetAll => {
            let n = store.list().len();
            if n == 0 {
                "I don't have any notes.".to_owned()
            } else {
                conv.forget_all_asked = Some(mono);
                format!(
                    "Forget all {n} note{}? Yes or no?",
                    if n == 1 { "" } else { "s" }
                )
            }
        }
    }
}

/// The spoken answer to "what are my notes": the count and the first five.
fn spoken_list(store: &notes::NoteStore) -> String {
    use notes::NoteError;
    let all = store.list();
    if all.is_empty() {
        return if store.enabled() {
            "I don't have any notes.".to_owned()
        } else {
            NoteError::Off.spoken()
        };
    }
    let mut s = format!(
        "I have {} note{}.",
        all.len(),
        if all.len() == 1 { "" } else { "s" }
    );
    for (i, n) in all.iter().take(5).enumerate() {
        s.push_str(&format!(" {}: {}.", i + 1, n.text.trim_end_matches('.')));
    }
    if all.len() > 5 {
        s.push_str(&format!(" And {} more.", all.len() - 5));
    }
    s
}
