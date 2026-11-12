//! The read-back of a held action (REQ-SEC-011): the sentence the host speaks before it asks "yes or no".
//! Whatever will be sent is in it, in full, because a person who says yes must have heard what they approved.
//! Nothing is shortened or summarised. A call whose read-back would not fit in [`MAX_READ_BACK_CHARS`] is
//! not held at all (the gate refuses it with a tool error that says by how much it is over). A value that
//! the redaction rules take for a secret is the one thing not spoken; the sentence says that a hidden key is
//! in it.

use super::spoken_name;
use crate::audit::{REDACTED_MARK, is_secret_name, redact_unclipped};
use serde_json::Value;

/// The most characters a held action's read-back may have, in total (tool name, keys and values).
///
/// About 700: 600 for the arguments and the rest for "I will run ...", the tool name and the closing
/// "Yes or no?". Speech runs at roughly 15 characters a second, so this is under 50 seconds of listening,
/// which leaves most of the 120 second confirmation window for the answer. A longer text is something a person
/// cannot reasonably judge by ear; the model can shorten it or split it into several calls.
pub const MAX_READ_BACK_CHARS: usize = 700;

/// What the user hears in place of a secret.
const HIDDEN: &str = "a hidden key";

/// The note added when something was hidden.
const HIDDEN_NOTE: &str =
    "One value holds a hidden key that I will not read out; it will be sent as it is.";

/// The read-back for a tool with no description of its own: `I will run <name>. <key>: <value>. ...` with
/// every argument named and every value read in full. Keys come in sorted order, so the same arguments always
/// give the same words, and two calls that differ anywhere give different ones (apart from a secret's
/// content). Nested objects and lists are flattened into `outer inner: value` and `list item 2: value`.
#[must_use]
pub fn describe_generic(name: &str, args: &Value) -> String {
    let mut parts = Vec::new();
    let mut hidden = false;
    flatten(&mut Vec::new(), args, false, &mut parts, &mut hidden);
    let head = format!("I will run {}", spoken_name(name));
    let mut s = if parts.is_empty() {
        format!("{head} with no arguments")
    } else {
        // Each phrase ends with a full stop, unless its value already ended a sentence.
        let said: Vec<String> = parts
            .into_iter()
            .map(|p| {
                if p.ends_with(['.', '!', '?']) {
                    p
                } else {
                    format!("{p}.")
                }
            })
            .collect();
        format!("{head}. {}", said.join(" "))
    };
    if hidden {
        s.push(' ');
        s.push_str(HIDDEN_NOTE);
    }
    s
}

/// The read-back of text that is forwarded to an outside agent word for word (`inbox_reply`). `origin` is the
/// spoken name of the endpoint the reply is bound to, written by the host from the administrator's setting and
/// never taken from the event's own (untrusted) `source`; without one the sentence says "the agent".
#[must_use]
pub fn describe_agent_text(origin: Option<&str>, text: &str) -> String {
    let mut hidden = false;
    let said = speak_text(text, &mut hidden).replace('"', "'");
    let to = origin.map_or_else(|| "the agent".to_owned(), |o| speak_text(o, &mut hidden));
    let mut s = format!("I will send this to {to}, word for word: \"{said}\"");
    if hidden {
        s.push_str(". ");
        s.push_str(HIDDEN_NOTE);
    }
    s
}

/// The words for a free-text value to be read aloud, with secrets taken out. `hidden` is set if any were.
#[must_use]
pub fn speak_text(text: &str, hidden: &mut bool) -> String {
    let clean = redact_unclipped(text);
    if clean.contains(REDACTED_MARK) {
        *hidden = true;
    }
    let words: Vec<String> = clean
        .replace(REDACTED_MARK, HIDDEN)
        .split(' ')
        .map(speak_word)
        .collect();
    let s = words.join(" ");
    if s.trim().is_empty() {
        "empty".to_owned()
    } else {
        s
    }
}

/// An address is read the way it is said: `bob@example.com` is `bob at example dot com`.
fn speak_word(word: &str) -> String {
    let core = word.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', '"', '\'']);
    let tail = &word[core.len()..];
    let at = core.find('@');
    if core.contains("://") || at.is_none_or(|i| i == 0 || i + 1 == core.len()) {
        return word.to_owned();
    }
    format!("{}{tail}", core.replace('@', " at ").replace('.', " dot "))
}

/// A key as a person says it.
fn spoken_key(key: &str) -> String {
    let s = key
        .replace(['_', '-', '.'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if s.is_empty() { "value".to_owned() } else { s }
}

/// Walk the arguments, adding one `path: value` phrase per leaf. A secret-sounding key hides everything
/// below it.
fn flatten(
    path: &mut Vec<String>,
    v: &Value,
    secret: bool,
    out: &mut Vec<String>,
    hidden: &mut bool,
) {
    let label = |path: &[String]| {
        if path.is_empty() {
            "value".to_owned()
        } else {
            path.join(" ")
        }
    };
    match v {
        Value::Object(m) if !m.is_empty() => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            for k in keys {
                path.push(spoken_key(k));
                flatten(path, &m[k], secret || is_secret_name(k), out, hidden);
                path.pop();
            }
        }
        Value::Array(a) if !a.is_empty() => {
            for (i, x) in a.iter().enumerate() {
                path.push(format!("item {}", i + 1));
                flatten(path, x, secret, out, hidden);
                path.pop();
            }
        }
        Value::Null if path.is_empty() => {}
        Value::Object(_) | Value::Array(_) if path.is_empty() => {}
        leaf => {
            let words = if secret {
                *hidden = true;
                HIDDEN.to_owned()
            } else {
                match leaf {
                    Value::String(s) => speak_text(s, hidden),
                    Value::Number(n) => n.to_string(),
                    Value::Bool(b) => b.to_string(),
                    Value::Null => "empty".to_owned(),
                    // An empty object or list.
                    _ => "none".to_owned(),
                }
            };
            out.push(format!("{}: {words}", label(path)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_argument_is_named_in_a_stable_order_and_read_as_speech() {
        let a = json!({"to": "bob@example.com", "body": "Lunch at noon.", "urgent": true, "copies": 2,
            "cc": ["amy@example.org", "li"], "meta": {"reply_to": null, "tags": []}});
        let s = describe_generic("send_message", &a);
        assert_eq!(
            s,
            "I will run send message. body: Lunch at noon. cc item 1: amy at example dot org. \
             cc item 2: li. copies: 2. meta reply to: empty. meta tags: none. \
             to: bob at example dot com. urgent: true."
        );
        assert_eq!(s, describe_generic("send_message", &a), "stable");
        assert!(
            !s.contains(['{', '}', '[', ']', '"']),
            "no JSON punctuation: {s}"
        );
    }

    /// The built-in tools a turn can only propose (or a dry run only describe) say what they would do, not
    /// just their name; the agent reply reads its text in full and says it is going to an agent.
    #[test]
    fn built_in_descriptions_state_their_arguments() {
        use crate::assistant::builtin_describe_action as say;
        let s = say(
            "set_reminder",
            &json!({"text": "call the bank", "minutes": 20}),
        );
        assert!(
            s.contains("text: call the bank") && s.contains("minutes: 20"),
            "{s}"
        );
        let long = "x ".repeat(100);
        let s = say(
            "inbox_reply",
            &json!({"kind": "instruct", "text": long.trim()}),
        );
        assert!(
            s.contains(long.trim()) && s.starts_with("I will send this to the agent"),
            "{s}"
        );
        let s = say("inbox_reply", &json!({"kind": "ack"}));
        assert!(s.contains("handle this yourself"), "{s}");
        let s = say(
            "inbox_reply",
            &json!({"kind": "instruct", "text": "go", "pinned_origin": "Build Server"}),
        );
        assert_eq!(s, "I will send this to Build Server, word for word: \"go\"");
    }

    #[test]
    fn no_arguments_and_a_secret_are_said_plainly() {
        assert_eq!(
            describe_generic("ping", &json!({})),
            "I will run ping with no arguments"
        );
        let s = describe_generic(
            "call",
            &json!({"api_key": "hunter2", "q": "ok sk-abcdefgh12345678"}),
        );
        assert!(!s.contains("hunter2") && !s.contains("abcdefgh"), "{s}");
        assert!(
            s.contains("api key: a hidden key") && s.contains("q: ok a hidden key"),
            "{s}"
        );
        assert!(s.ends_with("it will be sent as it is."), "{s}");
    }
}
