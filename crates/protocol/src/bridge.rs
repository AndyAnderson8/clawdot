//! Wire types shared by the core and agent bridges (ADR-0006, REQ-INB-008).
//!
//! A bridge opens `GET /bridge/{endpoint}/replies` on the device and receives one SSE `reply` event per
//! [`Reply`]; it confirms each with `POST /bridge/{endpoint}/replies/{id}/ack`. Delivery is
//! at-least-once, so a bridge must tolerate seeing a reply twice (de-duplicate on [`Reply::id`]).

use serde::{Deserialize, Serialize};

/// What the user wants the agent to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplyKind {
    /// "Thanks, I'll handle it": the user took over; the agent should stop waiting.
    Ack,
    /// A free-text instruction to continue the work.
    Instruct,
}

/// One reply for an agent bridge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reply {
    /// Identifier, unique and increasing; also the SSE event id.
    pub id: u64,
    /// The endpoint the original event came from.
    pub endpoint: String,
    /// The event being answered.
    pub event_id: u64,
    /// The agent session to resume.
    pub session_ref: Option<String>,
    /// What to do.
    pub kind: ReplyKind,
    /// The user's words (empty for [`ReplyKind::Ack`]). Speech-to-text output: may contain errors.
    pub text: String,
    /// When it was queued, unix seconds.
    pub created_at: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Reply {
        Reply {
            id: 7,
            endpoint: "claude-code".into(),
            event_id: 3,
            session_ref: Some("abc-123".into()),
            kind: ReplyKind::Instruct,
            text: "keep going".into(),
            created_at: 1_800_000_000,
        }
    }

    #[test]
    fn the_wire_format_is_stable() {
        // Bridges in the wild parse exactly this; changing it is a breaking protocol change.
        assert_eq!(
            serde_json::to_string(&sample()).unwrap(),
            r#"{"id":7,"endpoint":"claude-code","event_id":3,"session_ref":"abc-123","kind":"instruct","text":"keep going","created_at":1800000000}"#
        );
    }

    #[test]
    fn replies_round_trip_and_ack_has_no_text_requirement() {
        let r = sample();
        assert_eq!(
            serde_json::from_str::<Reply>(&serde_json::to_string(&r).unwrap()).unwrap(),
            r
        );
        let ack = r#"{"id":1,"endpoint":"x","event_id":1,"session_ref":null,"kind":"ack","text":"","created_at":0}"#;
        assert_eq!(
            serde_json::from_str::<Reply>(ack).unwrap().kind,
            ReplyKind::Ack
        );
    }

    #[test]
    fn unknown_kinds_are_rejected_rather_than_guessed() {
        let bad = r#"{"id":1,"endpoint":"x","event_id":1,"session_ref":null,"kind":"shell","text":"rm -rf /","created_at":0}"#;
        assert!(serde_json::from_str::<Reply>(bad).is_err());
    }
}
