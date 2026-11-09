//! Where a scenario's model replies come from: a script (replay mode) or a real endpoint (live mode).

use crate::scenario::{CallSpec, Reply, Turn};
use clawdot_core::llm::{ChatRequest, Completion, LlmError, ToolCall, Usage};
use clawdot_core::session::ChatBackend;
use std::collections::VecDeque;
use std::future::Future;
use std::sync::Mutex;

/// A model the runner can drive turn by turn.
pub trait Model: ChatBackend {
    /// Called before each turn with the turn's number (from 0) and its definition.
    fn start_turn(&self, index: usize, turn: &Turn);
    /// Scripted replies a finished turn left unused (always 0 in live mode).
    fn leftover(&self) -> usize;
    /// Whether the loop asked for more replies than the script held (never in live mode).
    fn exhausted(&self) -> bool;
    /// The last request the model was sent.
    fn last_request(&self) -> Option<ChatRequest>;
    /// What the model replied, turn by turn (recorded in live mode, empty in replay).
    fn recording(&self) -> Vec<Vec<Reply>>;
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn to_completion(r: &Reply, next_id: &mut usize) -> Result<Completion, LlmError> {
    if let Some(e) = &r.error {
        return Err(LlmError::Provider(e.clone()));
    }
    let tool_calls = r
        .calls
        .iter()
        .map(|c: &CallSpec| {
            *next_id += 1;
            ToolCall {
                id: c.id.clone().unwrap_or_else(|| format!("call_{next_id}")),
                name: c.name.clone(),
                arguments: c.args_raw.clone().unwrap_or_else(|| {
                    c.args
                        .as_ref()
                        .map_or_else(|| "{}".to_owned(), serde_json::Value::to_string)
                }),
            }
        })
        .collect();
    Ok(Completion {
        text: r.text.clone().unwrap_or_default(),
        tool_calls,
        finish_reason: None,
        usage: r.cost_usd.map(|c| Usage {
            prompt_tokens: 0,
            completion_tokens: 0,
            cost_usd: Some(c),
        }),
    })
}

/// The scripted fake model: it returns the scenario's replies in order and nothing else.
#[derive(Debug, Default)]
pub struct Scripted {
    queue: Mutex<VecDeque<Reply>>,
    seen: Mutex<Vec<ChatRequest>>,
    exhausted: Mutex<bool>,
    ids: Mutex<usize>,
}

impl Scripted {
    /// An empty script.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl ChatBackend for Scripted {
    fn complete(
        &self,
        req: &ChatRequest,
    ) -> impl Future<Output = Result<Completion, LlmError>> + Send {
        lock(&self.seen).push(req.clone());
        let next = lock(&self.queue).pop_front();
        let out = if let Some(r) = next {
            to_completion(&r, &mut lock(&self.ids))
        } else {
            *lock(&self.exhausted) = true;
            Err(LlmError::Transport(
                "the scenario's script has no more replies".into(),
            ))
        };
        async move { out }
    }
}

impl Model for Scripted {
    fn start_turn(&self, _index: usize, turn: &Turn) {
        let mut q = lock(&self.queue);
        q.clear();
        q.extend(turn.model.iter().flatten().cloned());
        lock(&self.seen).clear();
    }
    fn leftover(&self) -> usize {
        lock(&self.queue).len()
    }
    fn exhausted(&self) -> bool {
        *lock(&self.exhausted)
    }
    fn last_request(&self) -> Option<ChatRequest> {
        lock(&self.seen).last().cloned()
    }
    fn recording(&self) -> Vec<Vec<Reply>> {
        Vec::new()
    }
}

/// A real endpoint, recording what it said so a run can be saved as a transcript.
#[derive(Debug)]
pub struct Live<B> {
    backend: B,
    seen: Mutex<Option<ChatRequest>>,
    turns: Mutex<Vec<Vec<Reply>>>,
}

impl<B> Live<B> {
    /// Wrap a backend.
    #[must_use]
    pub fn new(backend: B) -> Self {
        Self {
            backend,
            seen: Mutex::new(None),
            turns: Mutex::new(Vec::new()),
        }
    }
}

impl<B: ChatBackend + Sync> ChatBackend for Live<B> {
    fn complete(
        &self,
        req: &ChatRequest,
    ) -> impl Future<Output = Result<Completion, LlmError>> + Send {
        *lock(&self.seen) = Some(req.clone());
        let req = req.clone();
        async move {
            let out = self.backend.complete(&req).await;
            if let Ok(c) = &out {
                let reply = Reply {
                    text: (!c.text.is_empty()).then(|| c.text.clone()),
                    calls: c
                        .tool_calls
                        .iter()
                        .map(|t| CallSpec {
                            id: Some(t.id.clone()),
                            name: t.name.clone(),
                            args: serde_json::from_str(&t.arguments).ok(),
                            args_raw: serde_json::from_str::<serde_json::Value>(&t.arguments)
                                .is_err()
                                .then(|| t.arguments.clone()),
                        })
                        .collect(),
                    error: None,
                    cost_usd: c.usage.and_then(|u| u.cost_usd),
                };
                if let Some(t) = lock(&self.turns).last_mut() {
                    t.push(reply);
                }
            }
            out
        }
    }
}

impl<B: ChatBackend + Sync> Model for Live<B> {
    fn start_turn(&self, _index: usize, _turn: &Turn) {
        lock(&self.turns).push(Vec::new());
    }
    fn leftover(&self) -> usize {
        0
    }
    fn exhausted(&self) -> bool {
        false
    }
    fn last_request(&self) -> Option<ChatRequest> {
        lock(&self.seen).clone()
    }
    fn recording(&self) -> Vec<Vec<Reply>> {
        lock(&self.turns).clone()
    }
}
