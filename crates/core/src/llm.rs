//! OpenAI-compatible chat completions: request building, response and stream parsing, tool-call
//! accumulation, and sentence splitting for streamed speech (REQ-AST-001, 002, 003, 009).
//!
//! This module is transport-free: it turns values into JSON and JSON (or SSE lines) back into values, so
//! it is tested without a network. [`crate::transport`] carries it over HTTP.

use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fmt;

/// Who said it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Instructions to the model.
    System,
    /// The user.
    User,
    /// The model.
    Assistant,
    /// A tool's result.
    Tool,
}

impl Role {
    fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Tool => "tool",
        }
    }
}

/// A model's request to run a tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    /// Provider-assigned identifier, echoed back with the result.
    pub id: String,
    /// Tool name.
    pub name: String,
    /// Arguments as JSON text (models sometimes produce invalid JSON; parse defensively).
    pub arguments: String,
}

/// One message in a conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    /// Who said it.
    pub role: Role,
    /// The text.
    pub content: String,
    /// Tool calls requested (assistant messages only).
    pub tool_calls: Vec<ToolCall>,
    /// Which call this answers (tool messages only).
    pub tool_call_id: Option<String>,
}

impl Message {
    fn plain(role: Role, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }
    /// A system message.
    pub fn system(content: impl Into<String>) -> Self {
        Self::plain(Role::System, content)
    }
    /// A user message.
    pub fn user(content: impl Into<String>) -> Self {
        Self::plain(Role::User, content)
    }
    /// A plain assistant reply.
    pub fn assistant(content: impl Into<String>) -> Self {
        Self::plain(Role::Assistant, content)
    }
    /// An assistant message that requests tool calls.
    #[must_use]
    pub fn assistant_calls(content: impl Into<String>, calls: Vec<ToolCall>) -> Self {
        Self {
            tool_calls: calls,
            ..Self::plain(Role::Assistant, content)
        }
    }
    /// A tool's result.
    pub fn tool_result(call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            tool_call_id: Some(call_id.into()),
            ..Self::plain(Role::Tool, content)
        }
    }

    /// The wire form.
    #[must_use]
    pub fn to_json(&self) -> Value {
        let mut m = json!({ "role": self.role.as_str() });
        if self.tool_calls.is_empty() {
            m["content"] = json!(self.content);
        } else {
            m["content"] = if self.content.is_empty() {
                Value::Null
            } else {
                json!(self.content)
            };
            m["tool_calls"] = Value::Array(
                self.tool_calls
                    .iter()
                    .map(|c| {
                        json!({ "id": c.id, "type": "function",
                                "function": { "name": c.name, "arguments": c.arguments } })
                    })
                    .collect(),
            );
        }
        if let Some(id) = &self.tool_call_id {
            m["tool_call_id"] = json!(id);
        }
        m
    }

    /// Approximate size for history budgeting.
    #[must_use]
    pub fn size(&self) -> usize {
        self.content.len()
            + self
                .tool_calls
                .iter()
                .map(|c| c.name.len() + c.arguments.len() + 24)
                .sum::<usize>()
            + 16
    }
}

/// A tool the model may call.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolSpec {
    /// Name, `[a-z0-9_]`.
    pub name: String,
    /// What it does, for the model.
    pub description: String,
    /// JSON Schema of the arguments.
    pub parameters: Value,
}

impl ToolSpec {
    /// The wire form.
    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({ "type": "function",
                "function": { "name": self.name, "description": self.description, "parameters": self.parameters } })
    }
}

/// A chat completion request.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatRequest {
    /// Model identifier.
    pub model: String,
    /// The conversation so far.
    pub messages: Vec<Message>,
    /// Tools offered.
    pub tools: Vec<ToolSpec>,
    /// Cap on generated tokens (REQ-AST-011).
    pub max_tokens: u32,
}

impl ChatRequest {
    /// The request body.
    #[must_use]
    pub fn to_json(&self, stream: bool) -> Value {
        let mut body = json!({
            "model": self.model,
            "messages": self.messages.iter().map(Message::to_json).collect::<Vec<_>>(),
            "max_tokens": self.max_tokens,
            "stream": stream,
        });
        if !self.tools.is_empty() {
            body["tools"] = Value::Array(self.tools.iter().map(ToolSpec::to_json).collect());
            body["tool_choice"] = json!("auto");
        }
        if stream {
            body["stream_options"] = json!({ "include_usage": true });
        }
        body
    }
}

/// Token and cost accounting for one response.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Usage {
    /// Prompt tokens.
    pub prompt_tokens: u32,
    /// Generated tokens.
    pub completion_tokens: u32,
    /// Cost in US dollars when the provider reports it (OpenRouter does).
    pub cost_usd: Option<f64>,
}

/// A finished response.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Completion {
    /// The text.
    pub text: String,
    /// Tools the model wants run.
    pub tool_calls: Vec<ToolCall>,
    /// Why it stopped (`stop`, `tool_calls`, `length`, ...).
    pub finish_reason: Option<String>,
    /// Accounting, if reported.
    pub usage: Option<Usage>,
}

/// Why a request failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LlmError {
    /// Network or HTTP failure.
    Transport(String),
    /// The provider returned an error.
    Provider(String),
    /// The response was not understood.
    Malformed(String),
}

impl fmt::Display for LlmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(e) => write!(f, "could not reach the language model: {e}"),
            Self::Provider(e) => write!(f, "the language model reported an error: {e}"),
            Self::Malformed(e) => write!(f, "unexpected response from the language model: {e}"),
        }
    }
}

impl std::error::Error for LlmError {}

fn parse_usage(v: &Value) -> Option<Usage> {
    let u = v.get("usage")?;
    Some(Usage {
        prompt_tokens: u
            .get("prompt_tokens")
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(0),
        completion_tokens: u
            .get("completion_tokens")
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(0),
        cost_usd: u.get("cost").and_then(Value::as_f64),
    })
}

fn provider_error(v: &Value) -> Option<LlmError> {
    let e = v.get("error")?;
    let msg = e
        .get("message")
        .and_then(Value::as_str)
        .map_or_else(|| e.to_string(), str::to_owned);
    Some(LlmError::Provider(msg))
}

/// Parse a non-streaming response body.
///
/// # Errors
/// [`LlmError::Provider`] for an error object, [`LlmError::Malformed`] otherwise.
pub fn parse_completion(body: &str) -> Result<Completion, LlmError> {
    let v: Value = serde_json::from_str(body).map_err(|e| LlmError::Malformed(e.to_string()))?;
    if let Some(err) = provider_error(&v) {
        return Err(err);
    }
    let choice = v
        .pointer("/choices/0")
        .ok_or_else(|| LlmError::Malformed("no choices".into()))?;
    let msg = choice.get("message").unwrap_or(&Value::Null);
    let tool_calls = msg
        .get("tool_calls")
        .and_then(Value::as_array)
        .map(|calls| {
            calls
                .iter()
                .map(|c| ToolCall {
                    id: c.get("id").and_then(Value::as_str).unwrap_or("").to_owned(),
                    name: c
                        .pointer("/function/name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned(),
                    arguments: c
                        .pointer("/function/arguments")
                        .and_then(Value::as_str)
                        .unwrap_or("{}")
                        .to_owned(),
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Completion {
        text: msg
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned(),
        tool_calls,
        finish_reason: choice
            .get("finish_reason")
            .and_then(Value::as_str)
            .map(str::to_owned),
        usage: parse_usage(&v),
    })
}

/// A piece of a streamed response, as it arrives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamEvent {
    /// More text; feed it to a [`SentenceSplitter`] for streamed speech.
    Text(String),
    /// The stream is finished.
    Done,
}

#[derive(Debug, Default)]
struct PartialCall {
    id: String,
    name: String,
    arguments: String,
}

/// Accumulates a streamed (SSE) response line by line.
#[derive(Debug, Default)]
pub struct StreamAccumulator {
    text: String,
    calls: BTreeMap<usize, PartialCall>,
    finish_reason: Option<String>,
    usage: Option<Usage>,
}

impl StreamAccumulator {
    /// Process one SSE line (without its line ending).
    ///
    /// # Errors
    /// A provider error object, or JSON that cannot be parsed.
    pub fn feed_line(&mut self, line: &str) -> Result<Vec<StreamEvent>, LlmError> {
        let line = line.trim_end_matches('\r');
        let Some(data) = line.strip_prefix("data:") else {
            return Ok(Vec::new()); // comments (": OPENROUTER PROCESSING"), blank lines, other fields
        };
        let data = data.trim();
        if data == "[DONE]" {
            return Ok(vec![StreamEvent::Done]);
        }
        let v: Value =
            serde_json::from_str(data).map_err(|e| LlmError::Malformed(e.to_string()))?;
        if let Some(err) = provider_error(&v) {
            return Err(err);
        }
        if let Some(u) = parse_usage(&v) {
            self.usage = Some(u);
        }
        let mut events = Vec::new();
        if let Some(choice) = v.pointer("/choices/0") {
            if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
                self.finish_reason = Some(reason.to_owned());
            }
            let delta = choice.get("delta").unwrap_or(&Value::Null);
            if let Some(t) = delta.get("content").and_then(Value::as_str) {
                if !t.is_empty() {
                    self.text.push_str(t);
                    events.push(StreamEvent::Text(t.to_owned()));
                }
            }
            if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
                for c in calls {
                    let index = c
                        .get("index")
                        .and_then(Value::as_u64)
                        .and_then(|i| usize::try_from(i).ok())
                        .unwrap_or(0);
                    let p = self.calls.entry(index).or_default();
                    if let Some(id) = c.get("id").and_then(Value::as_str) {
                        p.id = id.to_owned();
                    }
                    if let Some(name) = c.pointer("/function/name").and_then(Value::as_str) {
                        p.name.push_str(name);
                    }
                    if let Some(args) = c.pointer("/function/arguments").and_then(Value::as_str) {
                        p.arguments.push_str(args);
                    }
                }
            }
        }
        Ok(events)
    }

    /// The finished response.
    #[must_use]
    pub fn finish(self) -> Completion {
        Completion {
            text: self.text,
            tool_calls: self
                .calls
                .into_values()
                .filter(|c| !c.name.is_empty())
                .map(|c| ToolCall {
                    id: c.id,
                    name: c.name,
                    arguments: if c.arguments.trim().is_empty() {
                        "{}".into()
                    } else {
                        c.arguments
                    },
                })
                .collect(),
            finish_reason: self.finish_reason,
            usage: self.usage,
        }
    }
}

/// Cuts streamed text into speakable sentences so TTS can start before the reply is finished
/// (REQ-VOI-004).
#[derive(Debug, Default)]
pub struct SentenceSplitter {
    buf: String,
}

/// A sentence is emitted early at a comma or space once it passes this length.
const LONG_RUN: usize = 160;
const ABBREVIATIONS: &[&str] = &[
    "mr", "mrs", "ms", "dr", "st", "vs", "etc", "e.g", "i.e", "approx", "no",
];

impl SentenceSplitter {
    fn ends_sentence(s: &str, i: usize) -> bool {
        let bytes = s.as_bytes();
        if !matches!(bytes[i], b'.' | b'!' | b'?') {
            return false;
        }
        match bytes.get(i + 1) {
            Some(b) if !b.is_ascii_whitespace() => return false, // "3.5", "example.com", "...."
            None => return false,                                // wait for more text
            _ => {}
        }
        if bytes[i] == b'.' {
            let word_start = s[..i]
                .rfind(|c: char| c.is_whitespace())
                .map_or(0, |p| p + 1);
            let word = s[word_start..i].to_ascii_lowercase();
            if ABBREVIATIONS.contains(&word.as_str()) {
                return false;
            }
        }
        true
    }

    /// Add text; returns any complete sentences.
    pub fn push(&mut self, text: &str) -> Vec<String> {
        self.buf.push_str(text);
        let mut out = Vec::new();
        loop {
            let cut = (0..self.buf.len())
                .filter(|&i| self.buf.is_char_boundary(i))
                .find(|&i| Self::ends_sentence(&self.buf, i))
                .map(|i| i + 1)
                .or_else(|| {
                    // No terminator for a long time: split at the last comma or space.
                    (self.buf.len() > LONG_RUN)
                        .then(|| {
                            self.buf[..LONG_RUN.min(self.buf.len())]
                                .rfind([',', ';', ' '])
                                .map(|p| p + 1)
                        })
                        .flatten()
                });
            let Some(cut) = cut else { break };
            let sentence: String = self.buf.drain(..cut).collect();
            let sentence = sentence.trim();
            if !sentence.is_empty() {
                out.push(sentence.to_owned());
            }
        }
        out
    }

    /// The remainder, at the end of the stream.
    pub fn flush(&mut self) -> Option<String> {
        let rest = std::mem::take(&mut self.buf);
        let rest = rest.trim();
        (!rest.is_empty()).then(|| rest.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_have_the_openai_shape() {
        let req = ChatRequest {
            model: "openai/gpt-4o-mini".into(),
            messages: vec![Message::system("be brief"), Message::user("hi")],
            tools: vec![ToolSpec {
                name: "get_time".into(),
                description: "current time".into(),
                parameters: json!({"type":"object","properties":{}}),
            }],
            max_tokens: 200,
        };
        let j = req.to_json(true);
        assert_eq!(j["model"], "openai/gpt-4o-mini");
        assert_eq!(j["max_tokens"], 200);
        assert_eq!(j["stream"], true);
        assert_eq!(j["stream_options"]["include_usage"], true);
        assert_eq!(j["messages"][1], json!({"role":"user","content":"hi"}));
        assert_eq!(j["tools"][0]["function"]["name"], "get_time");
        assert_eq!(j["tool_choice"], "auto");
        let plain = ChatRequest {
            tools: vec![],
            ..req
        }
        .to_json(false);
        assert!(plain.get("tools").is_none() && plain.get("stream_options").is_none());
    }

    #[test]
    fn tool_messages_serialise_correctly() {
        let calls = vec![ToolCall {
            id: "c1".into(),
            name: "set_timer".into(),
            arguments: "{\"seconds\":60}".into(),
        }];
        let a = Message::assistant_calls("", calls).to_json();
        assert_eq!(a["content"], Value::Null);
        assert_eq!(
            a["tool_calls"][0]["function"]["arguments"],
            "{\"seconds\":60}"
        );
        let t = Message::tool_result("c1", "done").to_json();
        assert_eq!(
            (t["role"].as_str(), t["tool_call_id"].as_str()),
            (Some("tool"), Some("c1"))
        );
    }

    #[test]
    fn a_plain_response_parses() {
        let body = r#"{"choices":[{"message":{"role":"assistant","content":"Hello there."},"finish_reason":"stop"}],
                        "usage":{"prompt_tokens":12,"completion_tokens":3,"cost":0.00002}}"#;
        let c = parse_completion(body).unwrap();
        assert_eq!(
            (c.text.as_str(), c.finish_reason.as_deref()),
            ("Hello there.", Some("stop"))
        );
        assert_eq!(
            c.usage,
            Some(Usage {
                prompt_tokens: 12,
                completion_tokens: 3,
                cost_usd: Some(0.00002)
            })
        );
    }

    #[test]
    fn a_tool_call_response_parses() {
        let body = r#"{"choices":[{"message":{"content":null,"tool_calls":[
            {"id":"call_1","type":"function","function":{"name":"set_timer","arguments":"{\"seconds\":600}"}}]},
            "finish_reason":"tool_calls"}]}"#;
        let c = parse_completion(body).unwrap();
        assert_eq!(c.text, "");
        assert_eq!(
            c.tool_calls,
            vec![ToolCall {
                id: "call_1".into(),
                name: "set_timer".into(),
                arguments: "{\"seconds\":600}".into()
            }]
        );
    }

    #[test]
    fn provider_errors_and_garbage_are_errors_not_panics() {
        assert!(
            matches!(parse_completion(r#"{"error":{"message":"rate limited","code":429}}"#), Err(LlmError::Provider(m)) if m == "rate limited")
        );
        assert!(matches!(
            parse_completion("<html>"),
            Err(LlmError::Malformed(_))
        ));
        assert!(matches!(
            parse_completion(r#"{"choices":[]}"#),
            Err(LlmError::Malformed(_))
        ));
    }

    fn stream(lines: &[&str]) -> Result<(Vec<StreamEvent>, Completion), LlmError> {
        let mut acc = StreamAccumulator::default();
        let mut events = Vec::new();
        for l in lines {
            events.extend(acc.feed_line(l)?);
        }
        Ok((events, acc.finish()))
    }

    #[test]
    fn a_streamed_text_reply_accumulates_and_reports_usage() {
        let (events, c) = stream(&[
            ": OPENROUTER PROCESSING",
            r#"data: {"choices":[{"delta":{"role":"assistant","content":"Hel"}}]}"#,
            "",
            r#"data: {"choices":[{"delta":{"content":"lo."}}]}"#,
            r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
            r#"data: {"choices":[],"usage":{"prompt_tokens":9,"completion_tokens":2}}"#,
            "data: [DONE]",
        ])
        .unwrap();
        assert_eq!(c.text, "Hello.");
        assert_eq!(c.finish_reason.as_deref(), Some("stop"));
        assert_eq!(c.usage.map(|u| u.completion_tokens), Some(2));
        assert_eq!(
            events,
            vec![
                StreamEvent::Text("Hel".into()),
                StreamEvent::Text("lo.".into()),
                StreamEvent::Done
            ]
        );
    }

    #[test]
    fn streamed_tool_calls_are_stitched_together_by_index() {
        let (_, c) = stream(&[
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"a","function":{"name":"set_timer","arguments":""}}]}}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"sec"}}]}}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":1,"id":"b","function":{"name":"get_time"}}]}}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"onds\":30}"}}]}}]}"#,
            r#"data: {"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
        ])
        .unwrap();
        assert_eq!(c.tool_calls.len(), 2);
        assert_eq!(
            (
                c.tool_calls[0].name.as_str(),
                c.tool_calls[0].arguments.as_str()
            ),
            ("set_timer", "{\"seconds\":30}")
        );
        assert_eq!(
            (
                c.tool_calls[1].id.as_str(),
                c.tool_calls[1].arguments.as_str()
            ),
            ("b", "{}"),
            "no arguments becomes an empty object"
        );
    }

    #[test]
    fn mid_stream_errors_surface() {
        let err = stream(&[r#"data: {"error":{"message":"upstream failed"}}"#]).unwrap_err();
        assert_eq!(err, LlmError::Provider("upstream failed".into()));
        assert!(matches!(
            stream(&["data: {not json"]),
            Err(LlmError::Malformed(_))
        ));
    }

    fn split_all(chunks: &[&str]) -> Vec<String> {
        let mut s = SentenceSplitter::default();
        let mut out: Vec<String> = chunks.iter().flat_map(|c| s.push(c)).collect();
        out.extend(s.flush());
        out
    }

    #[test]
    fn sentences_are_emitted_as_soon_as_they_complete() {
        assert_eq!(
            split_all(&[
                "The timer is set. ",
                "I will tell you",
                " when it is done! Anything else?"
            ]),
            vec![
                "The timer is set.",
                "I will tell you when it is done!",
                "Anything else?"
            ]
        );
    }

    #[test]
    fn decimals_and_abbreviations_do_not_split() {
        assert_eq!(
            split_all(&["It is 3.5 degrees. Dr. Smith agrees, e.g. today. Yes."]),
            vec![
                "It is 3.5 degrees.",
                "Dr. Smith agrees, e.g. today.",
                "Yes."
            ]
        );
        assert_eq!(
            split_all(&["Visit example.com now. Thanks."]),
            vec!["Visit example.com now.", "Thanks."]
        );
    }

    #[test]
    fn a_sentence_ending_in_a_single_letter_word_still_splits() {
        assert_eq!(
            split_all(&["You like it, so do I. Good."]),
            vec!["You like it, so do I.", "Good."]
        );
    }

    #[test]
    fn a_sentence_split_across_chunks_waits_for_its_end() {
        let mut s = SentenceSplitter::default();
        assert!(s.push("Hello wor").is_empty());
        assert!(
            s.push("ld.").is_empty(),
            "a final period is not yet known to end the sentence"
        );
        assert_eq!(s.push(" Next"), vec!["Hello world."]);
        assert_eq!(s.flush().as_deref(), Some("Next"));
    }

    #[test]
    fn very_long_runs_are_split_so_speech_can_start() {
        let long = "word, ".repeat(60);
        let parts = split_all(&[&long]);
        assert!(
            parts.len() >= 2 && parts.iter().all(|p| p.len() <= 170),
            "{parts:?}"
        );
    }
}
