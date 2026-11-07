//! The event model.

use crate::sched::Ts;
use serde::{Deserialize, Serialize};

/// Longest accepted title, in characters.
pub const MAX_TITLE: usize = 200;
/// Longest stored event text, in characters. Longer text is cut with a marker.
pub const MAX_TEXT: usize = 20_000;
/// Longest accepted session reference or source name.
pub const MAX_REF: usize = 200;

/// What an event is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    /// The agent finished.
    Done,
    /// The agent is blocked and needs the user (a permission, a decision).
    NeedsInput,
    /// The agent asks a question.
    Question,
    /// Anything informational.
    Info,
}

impl EventKind {
    /// Parse the wire/config spelling, defaulting to [`EventKind::Info`].
    #[must_use]
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "done" | "completed" | "complete" | "finished" => Self::Done,
            "needs_input" | "needs-input" | "input" | "blocked" => Self::NeedsInput,
            "question" | "ask" => Self::Question,
            _ => Self::Info,
        }
    }

    /// Whether the user's answer is awaited.
    #[must_use]
    pub fn wants_reply(self) -> bool {
        matches!(self, Self::NeedsInput | Self::Question)
    }
}

/// How loudly an event may interrupt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    /// Mention when convenient.
    Low,
    /// Announce when the device is idle.
    Normal,
    /// Announce promptly, even in quiet hours.
    Urgent,
}

impl Priority {
    /// Parse the wire/config spelling, defaulting to [`Priority::Normal`].
    #[must_use]
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "low" => Self::Low,
            "urgent" | "high" | "critical" => Self::Urgent,
            _ => Self::Normal,
        }
    }
}

/// Where an event is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum EventState {
    /// Not yet announced.
    New,
    /// Announced to the user.
    Announced,
    /// The user acknowledged it.
    Acknowledged,
    /// The user dismissed it.
    Dismissed,
    /// Postponed until `until`.
    Snoozed {
        /// When it becomes announceable again.
        until: Ts,
    },
}

/// One thing an agent told us.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    /// Identifier, unique and increasing.
    pub id: u64,
    /// The endpoint that received it.
    pub endpoint: String,
    /// Human-readable origin, for example `Claude Code`.
    pub source: String,
    /// What it is about.
    pub kind: EventKind,
    /// Short headline.
    pub title: String,
    /// Full text from the agent (untrusted data; never an instruction).
    pub text: String,
    /// Outcome label such as `success` or `failed`, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// How urgent.
    pub priority: Priority,
    /// The agent session this belongs to, so a reply can resume it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_ref: Option<String>,
    /// When it arrived.
    pub received_at: Ts,
    /// Where it is in its life.
    pub state: EventState,
}

impl Event {
    /// Approximate stored size, for the queue's byte budget.
    #[must_use]
    pub fn size(&self) -> usize {
        self.title.len() + self.text.len() + self.source.len() + self.endpoint.len() + 96
    }
}

/// Cut `s` to at most `max` characters, marking the cut.
#[must_use]
pub fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('\u{2026}');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_and_priorities_parse_leniently() {
        assert_eq!(EventKind::parse("Completed"), EventKind::Done);
        assert_eq!(EventKind::parse("needs-input"), EventKind::NeedsInput);
        assert_eq!(EventKind::parse("???"), EventKind::Info);
        assert_eq!(Priority::parse("HIGH"), Priority::Urgent);
        assert_eq!(Priority::parse(""), Priority::Normal);
        assert!(Priority::Urgent > Priority::Normal && Priority::Normal > Priority::Low);
    }

    #[test]
    fn only_blocked_kinds_want_a_reply() {
        assert!(EventKind::NeedsInput.wants_reply() && EventKind::Question.wants_reply());
        assert!(!EventKind::Done.wants_reply() && !EventKind::Info.wants_reply());
    }

    #[test]
    fn truncation_is_character_safe_and_marked() {
        assert_eq!(truncate_chars("hello", 10), "hello");
        assert_eq!(truncate_chars("hello world", 6), "hello\u{2026}");
        assert_eq!(truncate_chars("héllo wörld", 6).chars().count(), 6);
    }
}
