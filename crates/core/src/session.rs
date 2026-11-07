//! One conversational turn with the language model: bounded history, a bounded tool loop, enforced
//! confirmation for risky tools, a fallback model, and usage limits (REQ-AST-002, 003, 006..009, 011).
//!
//! The model is never trusted: tool arguments are validated, unknown tools are refused, a tool that needs
//! confirmation is not run until the *user* has confirmed (enforced here, not left to the tool), and the
//! loop cannot run away.

use crate::llm::{ChatRequest, Completion, LlmError, Message, Role, ToolSpec};
use crate::sched::Ts;
use crate::usage::{LimitReached, UsageTracker};
use serde_json::Value;
use std::future::Future;

/// Where completions come from. Implemented over HTTP in [`crate::transport`] and by fakes in tests.
pub trait ChatBackend {
    /// Send a request and return the finished response.
    fn complete(
        &self,
        req: &ChatRequest,
    ) -> impl Future<Output = Result<Completion, LlmError>> + Send;
}

/// The tools a turn can use.
pub trait ToolHost {
    /// Tools to offer the model.
    fn tool_specs(&self) -> Vec<ToolSpec>;
    /// Whether the user must confirm before this tool runs (REQ-AST-008).
    fn needs_confirmation(&self, name: &str) -> bool;
    /// Run a tool. `Err` text is shown to the model, not hidden.
    ///
    /// # Errors
    /// A message describing why the tool failed.
    fn call_tool(&mut self, name: &str, args: &Value) -> Result<String, String>;
}

/// Longest tool result kept in the conversation.
const MAX_TOOL_RESULT: usize = 2_000;

/// Conversation state with a hard size bound and idle expiry (REQ-AST-007).
#[derive(Debug, Clone)]
pub struct Conversation {
    system: String,
    history: Vec<Message>,
    max_messages: usize,
    max_bytes: usize,
    idle_secs: i64,
    last_activity: Ts,
}

impl Conversation {
    /// A conversation with a system prompt and limits.
    #[must_use]
    pub fn new(
        system: impl Into<String>,
        max_messages: usize,
        max_bytes: usize,
        idle_secs: i64,
    ) -> Self {
        Self {
            system: system.into(),
            history: Vec::new(),
            max_messages,
            max_bytes,
            idle_secs,
            last_activity: 0,
        }
    }

    /// The messages after the system prompt.
    #[must_use]
    pub fn history(&self) -> &[Message] {
        &self.history
    }

    fn expire_if_idle(&mut self, now: Ts) {
        if self.last_activity != 0 && now - self.last_activity > self.idle_secs {
            self.history.clear();
        }
        self.last_activity = now;
    }

    /// Drop the oldest turns until within limits, never leaving a tool result without its call.
    fn trim(&mut self) {
        let total = |h: &[Message]| h.iter().map(Message::size).sum::<usize>();
        while (self.history.len() > self.max_messages || total(&self.history) > self.max_bytes)
            && self.history.len() > 1
        {
            self.history.remove(0);
            // Re-anchor at the next user message so the model never sees a dangling tool result.
            while self.history.first().is_some_and(|m| m.role != Role::User)
                && self.history.len() > 1
            {
                self.history.remove(0);
            }
        }
    }

    fn request(&self, model: &str, tools: Vec<ToolSpec>, max_tokens: u32) -> ChatRequest {
        let mut messages = Vec::with_capacity(self.history.len() + 1);
        messages.push(Message::system(self.system.clone()));
        messages.extend(self.history.iter().cloned());
        ChatRequest {
            model: model.to_owned(),
            messages,
            tools,
            max_tokens,
        }
    }
}

/// Turn settings.
#[derive(Debug, Clone)]
pub struct TurnConfig {
    /// Primary model.
    pub model: String,
    /// Model to try if the primary fails (REQ-AST-009).
    pub fallback_model: Option<String>,
    /// Cap on generated tokens.
    pub max_tokens: u32,
    /// Most tool rounds per turn (REQ-AST-006).
    pub max_tool_loops: usize,
}

impl Default for TurnConfig {
    fn default() -> Self {
        Self {
            model: String::new(),
            fallback_model: None,
            max_tokens: 300,
            max_tool_loops: 5,
        }
    }
}

/// How a turn ended.
#[derive(Debug, Clone, PartialEq)]
pub enum TurnEnd {
    /// The model answered.
    Answered,
    /// The model kept calling tools past the loop bound.
    ToolLoopLimit,
    /// A usage limit stopped the turn.
    Limit(LimitReached),
    /// Every model attempt failed.
    Failed(LlmError),
}

/// What to say, and how the turn went.
#[derive(Debug, Clone, PartialEq)]
pub struct TurnResult {
    /// Text to speak.
    pub speech: String,
    /// How it ended.
    pub end: TurnEnd,
    /// Tool calls made.
    pub tool_calls: usize,
    /// Model requests made.
    pub model_requests: u32,
}

fn clip(s: &str) -> String {
    if s.len() <= MAX_TOOL_RESULT {
        return s.to_owned();
    }
    let mut end = MAX_TOOL_RESULT;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}... (cut)", &s[..end])
}

/// Run one turn: the user's words in, the words to speak out.
#[allow(clippy::too_many_arguments)]
pub async fn run_turn<B: ChatBackend, H: ToolHost>(
    backend: &B,
    host: &mut H,
    conv: &mut Conversation,
    cfg: &TurnConfig,
    usage: &mut UsageTracker,
    day: i64,
    now: Ts,
    user_text: &str,
) -> TurnResult {
    conv.expire_if_idle(now);
    conv.history.push(Message::user(user_text));
    let mut result = TurnResult {
        speech: String::new(),
        end: TurnEnd::Answered,
        tool_calls: 0,
        model_requests: 0,
    };

    for round in 0..=cfg.max_tool_loops {
        if round == cfg.max_tool_loops {
            result.speech = "I got stuck trying to do that.".to_owned();
            result.end = TurnEnd::ToolLoopLimit;
            return result;
        }
        if let Err(limit) = usage.check(day) {
            result.speech = limit.to_string();
            result.end = TurnEnd::Limit(limit);
            return result;
        }

        let tools = host.tool_specs();
        let mut models = vec![cfg.model.as_str()];
        models.extend(cfg.fallback_model.as_deref());
        let mut completion = None;
        let mut last_err = None;
        for model in models {
            if usage.check(day).is_err() {
                break;
            }
            let req = conv.request(model, tools.clone(), cfg.max_tokens);
            result.model_requests += 1;
            match backend.complete(&req).await {
                Ok(c) => {
                    usage.record(day, c.usage.as_ref());
                    completion = Some(c);
                    break;
                }
                Err(e) => {
                    usage.record(day, None);
                    last_err = Some(e);
                }
            }
        }
        let Some(completion) = completion else {
            result.speech = "Sorry, I couldn't reach the language model.".to_owned();
            result.end = last_err.map_or(TurnEnd::Limit(LimitReached::Requests), TurnEnd::Failed);
            return result;
        };

        if completion.tool_calls.is_empty() {
            let text = completion.text.trim();
            result.speech = if text.is_empty() {
                "Done.".to_owned()
            } else {
                text.to_owned()
            };
            conv.history.push(Message::assistant(result.speech.clone()));
            conv.trim();
            return result;
        }

        conv.history.push(Message::assistant_calls(
            completion.text.clone(),
            completion.tool_calls.clone(),
        ));
        let known: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();
        for call in &completion.tool_calls {
            result.tool_calls += 1;
            let outcome = match serde_json::from_str::<Value>(&call.arguments) {
                Err(_) => "Error: the arguments were not valid JSON.".to_owned(),
                Ok(_) if !known.contains(&call.name) => format!("Error: there is no tool called `{}`.", call.name),
                Ok(args) if host.needs_confirmation(&call.name) && args.get("confirmed") != Some(&Value::Bool(true)) => {
                    "Not executed. This action needs the user's confirmation: ask them to confirm out loud, then call the tool again with confirmed set to true."
                        .to_owned()
                }
                Ok(args) => host.call_tool(&call.name, &args).unwrap_or_else(|e| format!("Error: {e}")),
            };
            conv.history
                .push(Message::tool_result(call.id.clone(), clip(&outcome)));
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::ToolCall;
    use crate::usage::UsageLimits;
    use serde_json::json;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    struct Script {
        replies: Mutex<VecDeque<Result<Completion, LlmError>>>,
        seen: Mutex<Vec<ChatRequest>>,
    }

    impl Script {
        fn new(replies: Vec<Result<Completion, LlmError>>) -> Self {
            Self {
                replies: Mutex::new(replies.into()),
                seen: Mutex::new(Vec::new()),
            }
        }
        fn requests(&self) -> Vec<ChatRequest> {
            self.seen.lock().unwrap().clone()
        }
    }

    impl ChatBackend for Script {
        fn complete(
            &self,
            req: &ChatRequest,
        ) -> impl Future<Output = Result<Completion, LlmError>> + Send {
            self.seen.lock().unwrap().push(req.clone());
            let next = self.replies.lock().unwrap().pop_front();
            async move { next.unwrap_or_else(|| Err(LlmError::Transport("script exhausted".into()))) }
        }
    }

    #[derive(Default)]
    struct Host {
        ran: Vec<(String, Value)>,
        confirm: Vec<&'static str>,
    }

    impl ToolHost for Host {
        fn tool_specs(&self) -> Vec<ToolSpec> {
            ["set_timer", "send_money"]
                .iter()
                .map(|n| ToolSpec {
                    name: (*n).into(),
                    description: String::new(),
                    parameters: json!({"type":"object"}),
                })
                .collect()
        }
        fn needs_confirmation(&self, name: &str) -> bool {
            self.confirm.contains(&name)
        }
        fn call_tool(&mut self, name: &str, args: &Value) -> Result<String, String> {
            self.ran.push((name.to_owned(), args.clone()));
            if name == "set_timer" && args.get("seconds").is_none() {
                return Err("seconds is required".into());
            }
            Ok(format!("{name} ok"))
        }
    }

    fn text(t: &str) -> Result<Completion, LlmError> {
        Ok(Completion {
            text: t.into(),
            ..Completion::default()
        })
    }

    fn calls(cs: &[(&str, &str, &str)]) -> Result<Completion, LlmError> {
        Ok(Completion {
            tool_calls: cs
                .iter()
                .map(|(id, n, a)| ToolCall {
                    id: (*id).into(),
                    name: (*n).into(),
                    arguments: (*a).into(),
                })
                .collect(),
            ..Completion::default()
        })
    }

    fn cfg() -> TurnConfig {
        TurnConfig {
            model: "primary".into(),
            ..TurnConfig::default()
        }
    }

    fn conv() -> Conversation {
        Conversation::new("You are a speaker.", 20, 20_000, 600)
    }

    async fn turn(
        b: &Script,
        h: &mut Host,
        c: &mut Conversation,
        u: &mut UsageTracker,
        say: &str,
    ) -> TurnResult {
        run_turn(b, h, c, &cfg(), u, 1, 1000, say).await
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_plain_answer_is_returned_and_remembered() {
        let b = Script::new(vec![text("The capital is Canberra.")]);
        let (mut h, mut c, mut u) = (
            Host::default(),
            conv(),
            UsageTracker::new(UsageLimits::default()),
        );
        let r = turn(&b, &mut h, &mut c, &mut u, "capital of australia").await;
        assert_eq!(
            (r.speech.as_str(), r.end.clone(), r.model_requests),
            ("The capital is Canberra.", TurnEnd::Answered, 1)
        );
        assert_eq!(c.history().len(), 2);
        let req = &b.requests()[0];
        assert_eq!(
            (req.model.as_str(), req.max_tokens, req.messages[0].role),
            ("primary", 300, Role::System)
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_tool_call_runs_then_the_model_answers_with_the_result() {
        let b = Script::new(vec![
            calls(&[("c1", "set_timer", r#"{"seconds":600}"#)]),
            text("Timer set for ten minutes."),
        ]);
        let (mut h, mut c, mut u) = (
            Host::default(),
            conv(),
            UsageTracker::new(UsageLimits::default()),
        );
        let r = turn(&b, &mut h, &mut c, &mut u, "ten minute timer").await;
        assert_eq!(
            (r.speech.as_str(), r.tool_calls, r.model_requests),
            ("Timer set for ten minutes.", 1, 2)
        );
        assert_eq!(
            h.ran,
            vec![("set_timer".to_owned(), json!({"seconds":600}))]
        );
        let second = &b.requests()[1];
        let last = second.messages.last().unwrap();
        assert_eq!(
            (
                last.role,
                last.content.as_str(),
                last.tool_call_id.as_deref()
            ),
            (Role::Tool, "set_timer ok", Some("c1"))
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn tools_that_need_confirmation_wait_for_the_user() {
        let b = Script::new(vec![
            calls(&[("c1", "send_money", r#"{"amount":5}"#)]),
            text("Do you want me to send five dollars?"),
            calls(&[("c2", "send_money", r#"{"amount":5,"confirmed":true}"#)]),
            text("Sent."),
        ]);
        let (mut h, mut c, mut u) = (
            Host {
                confirm: vec!["send_money"],
                ..Host::default()
            },
            conv(),
            UsageTracker::new(UsageLimits::default()),
        );
        let r1 = turn(&b, &mut h, &mut c, &mut u, "send five dollars").await;
        assert_eq!(r1.speech, "Do you want me to send five dollars?");
        assert!(h.ran.is_empty(), "nothing ran before the user confirmed");
        assert!(
            b.requests()[1]
                .messages
                .last()
                .unwrap()
                .content
                .starts_with("Not executed")
        );
        let r2 = turn(&b, &mut h, &mut c, &mut u, "yes").await;
        assert_eq!(r2.speech, "Sent.");
        assert_eq!(h.ran.len(), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn bad_tool_calls_become_errors_the_model_can_see() {
        let b = Script::new(vec![
            calls(&[
                ("a", "set_timer", "{not json"),
                ("b", "nonexistent", "{}"),
                ("c", "set_timer", "{}"),
            ]),
            text("Sorry, that did not work."),
        ]);
        let (mut h, mut c, mut u) = (
            Host::default(),
            conv(),
            UsageTracker::new(UsageLimits::default()),
        );
        turn(&b, &mut h, &mut c, &mut u, "x").await;
        let msgs = &b.requests()[1].messages;
        let tool_msgs: Vec<&str> = msgs
            .iter()
            .filter(|m| m.role == Role::Tool)
            .map(|m| m.content.as_str())
            .collect();
        assert!(tool_msgs[0].contains("not valid JSON"));
        assert!(tool_msgs[1].contains("no tool called `nonexistent`"));
        assert!(tool_msgs[2].starts_with("Error: seconds is required"));
        assert_eq!(h.ran.len(), 1, "only the well-formed call reached the tool");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_runaway_tool_loop_is_cut_off() {
        let b = Script::new(
            (0..20)
                .map(|_| calls(&[("c", "set_timer", r#"{"seconds":1}"#)]))
                .collect(),
        );
        let (mut h, mut c, mut u) = (
            Host::default(),
            conv(),
            UsageTracker::new(UsageLimits::default()),
        );
        let r = turn(&b, &mut h, &mut c, &mut u, "loop forever").await;
        assert_eq!(r.end, TurnEnd::ToolLoopLimit);
        assert_eq!(b.requests().len(), 5, "exactly max_tool_loops requests");
        assert_eq!(r.speech, "I got stuck trying to do that.");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn the_fallback_model_is_used_when_the_primary_fails() {
        let b = Script::new(vec![
            Err(LlmError::Provider("overloaded".into())),
            text("From the backup."),
        ]);
        let (mut h, mut c, mut u) = (
            Host::default(),
            conv(),
            UsageTracker::new(UsageLimits::default()),
        );
        let config = TurnConfig {
            fallback_model: Some("backup".into()),
            ..cfg()
        };
        let r = run_turn(&b, &mut h, &mut c, &config, &mut u, 1, 1000, "hi").await;
        assert_eq!(r.speech, "From the backup.");
        let models: Vec<_> = b.requests().iter().map(|r| r.model.clone()).collect();
        assert_eq!(models, vec!["primary", "backup"]);
        assert_eq!(u.requests_today(), 2);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn when_every_model_fails_the_turn_says_so() {
        let b = Script::new(vec![Err(LlmError::Transport("down".into()))]);
        let (mut h, mut c, mut u) = (
            Host::default(),
            conv(),
            UsageTracker::new(UsageLimits::default()),
        );
        let r = turn(&b, &mut h, &mut c, &mut u, "hi").await;
        assert_eq!(r.speech, "Sorry, I couldn't reach the language model.");
        assert!(matches!(r.end, TurnEnd::Failed(LlmError::Transport(_))));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn the_daily_limit_stops_the_turn_before_any_request() {
        let b = Script::new(vec![text("never sent")]);
        let (mut h, mut c, mut u) = (
            Host::default(),
            conv(),
            UsageTracker::new(UsageLimits {
                max_requests_per_day: 0,
                ..UsageLimits::default()
            }),
        );
        let r = turn(&b, &mut h, &mut c, &mut u, "hi").await;
        assert_eq!(r.end, TurnEnd::Limit(LimitReached::Requests));
        assert!(r.speech.contains("Timers, alarms"));
        assert!(b.requests().is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn history_is_bounded_and_never_starts_with_a_dangling_tool_result() {
        let replies: Vec<_> = (0..30)
            .flat_map(|i| {
                [
                    calls(&[("c", "set_timer", r#"{"seconds":1}"#)]),
                    text(&format!("answer {i}")),
                ]
            })
            .collect();
        let b = Script::new(replies);
        let (mut h, mut u) = (Host::default(), UsageTracker::new(UsageLimits::default()));
        let mut c = Conversation::new("s", 6, 20_000, 600);
        for i in 0..30 {
            run_turn(
                &b,
                &mut h,
                &mut c,
                &cfg(),
                &mut u,
                1,
                1000 + i,
                &format!("q{i}"),
            )
            .await;
            assert!(c.history().len() <= 6 + 4, "bounded: {}", c.history().len());
            assert_eq!(
                c.history()[0].role,
                Role::User,
                "anchored at a user message"
            );
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn an_idle_conversation_is_forgotten() {
        let b = Script::new(vec![text("one"), text("two")]);
        let (mut h, mut c, mut u) = (
            Host::default(),
            conv(),
            UsageTracker::new(UsageLimits::default()),
        );
        run_turn(&b, &mut h, &mut c, &cfg(), &mut u, 1, 1000, "a").await;
        run_turn(&b, &mut h, &mut c, &cfg(), &mut u, 1, 1000 + 601, "b").await;
        assert_eq!(c.history().len(), 2, "the first exchange expired");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn oversized_tool_results_are_clipped() {
        struct Big;
        impl ToolHost for Big {
            fn tool_specs(&self) -> Vec<ToolSpec> {
                vec![ToolSpec {
                    name: "dump".into(),
                    description: String::new(),
                    parameters: json!({}),
                }]
            }
            fn needs_confirmation(&self, _: &str) -> bool {
                false
            }
            fn call_tool(&mut self, _: &str, _: &Value) -> Result<String, String> {
                Ok("é".repeat(5_000))
            }
        }
        let b = Script::new(vec![calls(&[("c", "dump", "{}")]), text("ok")]);
        let (mut c, mut u) = (conv(), UsageTracker::new(UsageLimits::default()));
        run_turn(&b, &mut Big, &mut c, &cfg(), &mut u, 1, 1000, "dump it").await;
        let tool = b.requests()[1]
            .messages
            .iter()
            .find(|m| m.role == Role::Tool)
            .unwrap()
            .content
            .clone();
        assert!(tool.len() < 2_100 && tool.ends_with("(cut)"));
    }
}
