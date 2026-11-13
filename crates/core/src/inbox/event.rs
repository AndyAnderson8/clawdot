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
        /// When it becomes announceable again, as a wall-clock time for people and for files from before
        /// durations were kept apart. What decides is [`Timing::snooze`], which the wall clock cannot step.
        until: Ts,
    },
}

/// The durations of one event as monotonic readings ([`crate::sched::Clock::monotonic_secs`]), which mean
/// something only in this process. They are never written as they are: a file holds them as [`Rel`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Timing {
    /// When it arrived (or was last merged into), for the coalesce window, the age of a handled event and
    /// the order of events.
    pub received: i64,
    /// When its time to live runs out.
    pub expires: Option<i64>,
    /// When a snooze ends.
    pub snooze: Option<i64>,
}

/// An event's durations as a file holds them: seconds relative to the moment of the save. Read back by
/// `Inbox::load`; written by `Inbox::encode`, which adds it to the event's own fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rel {
    /// Seconds since it arrived.
    pub age: i64,
    /// Seconds until its time to live runs out (negative if it already has).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl_left: Option<i64>,
    /// Seconds until a snooze ends (negative if it already has).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snooze_left: Option<i64>,
}

/// One thing an agent told us.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    /// Identifier, unique and increasing.
    pub id: u64,
    /// The endpoint that received it.
    pub endpoint: String,
    /// The label the sender gave itself, for example `Claude Code`. Untrusted: shown as text in the portal and
    /// recorded, never spoken as the origin and never given to the summariser as fact.
    pub source: String,
    /// The origin the speaker names: the endpoint's spoken name, or its name, as the administrator had set it
    /// when the event arrived. Empty on events saved before it existed (see [`Event::origin_name`]).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub origin: String,
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
    /// A one-way announcement: a spoken reply is never sent to it (REQ-INB-023).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_reply: bool,
    /// Not spoken after this time (REQ-INB-023), as a wall-clock time. The monotonic [`Timing::expires`] is what
    /// is compared; this is what a file from before durations were kept apart holds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<Ts>,
    /// When it arrived, as a wall-clock time to show a person. Ages are measured with [`Timing::received`].
    pub received_at: Ts,
    /// The durations, as monotonic readings. Not part of the file; see [`Rel`].
    #[serde(skip)]
    pub timing: Timing,
    /// The durations as a file holds them. Present only between reading a file and `Inbox::load` finishing.
    #[serde(default, skip_serializing)]
    pub(super) rel: Option<Rel>,
    /// Where it is in its life.
    pub state: EventState,
    /// The order it was last spoken in: higher is more recent, zero is never (or before this was recorded). A
    /// spoken reply goes to the event spoken last, which is not always the one that arrived last.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub spoken_seq: u64,
}

#[allow(clippy::trivially_copy_pass_by_ref)] // serde passes a reference
fn is_zero(n: &u64) -> bool {
    *n == 0
}

impl Event {
    /// Who the speaker says the event is from: [`Event::origin`], or the endpoint's name for an old event.
    #[must_use]
    pub fn origin_name(&self) -> &str {
        if self.origin.is_empty() {
            &self.endpoint
        } else {
            &self.origin
        }
    }

    /// Approximate stored size, for the queue's byte budget.
    #[must_use]
    pub fn size(&self) -> usize {
        self.title.len()
            + self.text.len()
            + self.source.len()
            + self.origin.len()
            + self.endpoint.len()
            + 96
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
    fn events_saved_before_announcements_existed_still_load() {
        let old = r#"{"id":1,"endpoint":"cc","source":"Claude Code","kind":"done","title":"t","text":"x",
            "priority":"normal","received_at":5,"state":{"state":"new"}}"#;
        let e: Event = serde_json::from_str(old).unwrap();
        assert!(!e.no_reply && e.expires_at.is_none());
        assert_eq!(
            e.origin_name(),
            "cc",
            "an old event is spoken as its endpoint"
        );
        let json = serde_json::to_string(&e).unwrap();
        assert!(!json.contains("no_reply") && !json.contains("expires_at"));
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
