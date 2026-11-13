//! The OpenAI Realtime WebSocket dialect (ADR-0020).
//!
//! Everything that names this provider's protocol lives in this file: the request path and `Authorization`
//! header, the JSON event names and shapes, and which of its error codes mean what. The engine in the parent
//! module sees only the neutral [`Event`]s and [`ErrorKind`]s, so a second provider is a second file like this
//! one plus a branch where the dialect is chosen ([`crate::rtvoice::PROVIDER`] and `serve::serve`). It has been
//! exercised only against an in-process fake server that follows the documented events, not the real service
//! (see docs/guides/realtime-voice.md).

use super::{Dialect, ErrorKind, Event, FnCall, RealtimeConfig, RealtimeError, SAMPLE_RATE, b64};
use crate::llm::ToolSpec;
use serde_json::{Value, json};

/// Characters other than letters and digits that a model name may contain; the name goes into the URL query.
const MODEL_NAME_PUNCTUATION: &str = "-_.:";

/// Provider error codes that do not end the session (a cancel or a second `response.create` that raced).
const HARMLESS: [&str; 2] = [
    "conversation_already_has_active_response",
    "response_cancel_not_active",
];

/// The OpenAI Realtime WebSocket protocol.
#[derive(Debug, Clone, Copy, Default)]
pub struct OpenAi;

impl Dialect for OpenAi {
    fn path(&self, cfg: &RealtimeConfig, base: &str) -> Result<String, RealtimeError> {
        if cfg.model.is_empty()
            || !cfg
                .model
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || MODEL_NAME_PUNCTUATION.contains(c))
        {
            return Err(RealtimeError::Config(
                "the realtime model name has characters that are not allowed".into(),
            ));
        }
        let base = if base.is_empty() { "/" } else { base };
        Ok(format!("{base}?model={}", cfg.model))
    }

    fn headers(&self, cfg: &RealtimeConfig) -> Vec<(String, String)> {
        vec![("Authorization".into(), format!("Bearer {}", cfg.api_key))]
    }

    fn session_update(&self, cfg: &RealtimeConfig, tools: &[ToolSpec]) -> Value {
        let mut input = json!({
            "format": {"type": "audio/pcm", "rate": SAMPLE_RATE},
            "turn_detection": {"type": "server_vad"},
        });
        if !cfg.transcription_model.is_empty() {
            input["transcription"] = json!({"model": cfg.transcription_model});
        }
        json!({
            "type": "session.update",
            "session": {
                "type": "realtime",
                "model": cfg.model,
                "output_modalities": ["audio"],
                "instructions": cfg.instructions,
                "audio": {
                    "input": input,
                    "output": {"format": {"type": "audio/pcm", "rate": SAMPLE_RATE}, "voice": cfg.voice},
                },
                "tools": tools.iter().map(|t| json!({
                    "type": "function",
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters,
                })).collect::<Vec<_>>(),
                "tool_choice": "auto",
            }
        })
    }

    fn tools_update(&self, tools: &[ToolSpec]) -> Value {
        json!({
            "type": "session.update",
            "session": {
                "type": "realtime",
                "tools": tools.iter().map(|t| json!({
                    "type": "function",
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters,
                })).collect::<Vec<_>>(),
                "tool_choice": "auto",
            }
        })
    }

    fn audio_append(&self, pcm: &[u8]) -> Value {
        json!({"type": "input_audio_buffer.append", "audio": b64::encode(pcm)})
    }

    fn function_output(&self, call_id: &str, output: &str) -> Value {
        json!({
            "type": "conversation.item.create",
            "item": {"type": "function_call_output", "call_id": call_id, "output": output},
        })
    }

    fn response_create(&self, instructions: Option<&str>) -> Value {
        match instructions {
            Some(i) => json!({"type": "response.create", "response": {"instructions": i}}),
            None => json!({"type": "response.create"}),
        }
    }

    /// The one place a provider event's `type` is matched as text; everything after this is an [`Event`].
    fn parse(&self, text: &str) -> Event {
        let Ok(v) = serde_json::from_str::<Value>(text) else {
            return Event::Ignored;
        };
        let s = |p: &Value| p.as_str().unwrap_or_default().to_owned();
        match v["type"].as_str().unwrap_or_default() {
            "response.output_audio.delta" => v["delta"]
                .as_str()
                .and_then(b64::decode)
                .map_or(Event::Ignored, Event::Audio),
            "input_audio_buffer.speech_started" => Event::UserSpeechStarted,
            "conversation.item.input_audio_transcription.completed" => {
                Event::UserTranscript(s(&v["transcript"]))
            }
            "response.created" => Event::ResponseStarted,
            "response.done" => Event::ResponseDone(
                v["response"]["output"]
                    .as_array()
                    .map(|items| {
                        items
                            .iter()
                            .filter(|i| i["type"] == "function_call")
                            .map(|i| FnCall {
                                id: s(&i["call_id"]),
                                name: s(&i["name"]),
                                arguments: s(&i["arguments"]),
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            ),
            "error" => Event::Error {
                code: s(&v["error"]["code"]),
                message: s(&v["error"]["message"]),
            },
            _ => Event::Ignored,
        }
    }

    fn error_kind(&self, code: &str) -> ErrorKind {
        if HARMLESS.contains(&code) {
            return ErrorKind::Harmless;
        }
        match code {
            "invalid_api_key" | "authentication_error" | "invalid_authentication" => {
                ErrorKind::KeyRejected
            }
            "insufficient_quota" | "billing_hard_limit_reached" => ErrorKind::Quota,
            "rate_limit_exceeded" => ErrorKind::RateLimited,
            _ => ErrorKind::Other,
        }
    }
}
