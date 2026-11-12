//! Turns that run beside the main loop (ADR-0021, REQ-AST-027, finding C1 of the architecture review).
//!
//! The main loop owns the state: the timers and the built-in tools, the portal's settings, tools and skills,
//! the usage files. A model turn is slow (seconds, up to the turn deadline), so it runs as a task of its own
//! and the loop goes on ringing timers, answering the portal and listening to other speakers meanwhile.
//! Nothing is shared behind a lock. The loop and a turn talk by messages:
//!
//! ```text
//!   loop ── Request ──► Hub::submit ── spawns ──► turn task ◄── a snapshot of the prompt, the tools,
//!    ▲                                              │  │         the policy and the config, taken now
//!    │◄──── ToolReq (a built-in tool call) ─────────┘  │
//!    │───── ToolAnswer (result and effects) ──────────►│
//!    │◄──── Finished (the conversation, the words to say, the effects)
//! ```
//!
//! * A turn task **owns** its speaker's [`Conversation`] (so its held confirmation) for the turn and gives it
//!   back in [`Finished`]; the set of conversations lives in the loop ([`crate::speakers`]).
//! * HTTP and MCP tools are called by the task itself, through clones of their clients. A built-in tool
//!   (timers, alarms, the stopwatch, music, the inbox) changes state the loop owns, so the task sends the
//!   call to the loop as a [`ToolReq`]; the loop runs it and answers with the result and the effects it
//!   produced, which the task collects for its speaker.
//! * What a turn decided at its start (persona, model, limits, which tools are off, the tiers the
//!   administrator chose) is a snapshot: a portal change made during a turn reaches the next turn. The gate
//!   (tier, hold, audit, limits, cancel) runs unchanged in the task.
//! * The daily request and spend counters are the one shared thing, a tiny counter cell, so the cap sees every
//!   turn at once ([`UsageTracker`]).

use crate::assistant::{
    Assistant, Effect, builtin_describe_action, builtin_returns_untrusted_text, builtin_risk,
};
use crate::extensions::{Extensions, WithPolicy};
use crate::httptools::{HttpTools, WithHttp};
use crate::llm::ToolSpec;
use crate::mcp::{McpClient, WithMcp};
use crate::satellite::Answer;
use crate::sched::Ts;
use crate::session::{
    ChatBackend, Conversation, Risk, ToolHost, TurnConfig, TurnCtl, TurnEnd, TurnInputs,
    run_turn_with,
};
use crate::speakers::{Checkout, SpeakerId, Speakers};
use crate::usage::UsageTracker;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};
use tokio::task::{Id, JoinSet};

/// What to say to a speaker, with the effects its satellite should carry out.
#[derive(Debug, Clone, Default)]
pub struct Reply {
    /// The words.
    pub say: String,
    /// What the satellite should do besides speaking.
    pub effects: Vec<Effect>,
    /// The reply ends with a question, so the follow-up window should be longer (REQ-AST-021).
    pub expects_reply: bool,
}

/// Where a reply goes.
#[derive(Debug)]
pub enum Origin {
    /// A satellite waiting for its answer.
    Ask(oneshot::Sender<Answer>),
    /// The console prints it.
    Console,
}

impl Origin {
    /// Deliver the reply. The console is handled by the caller, which gets the reply back.
    pub fn deliver(self, reply: Reply) -> Option<Reply> {
        match self {
            Self::Ask(tx) => {
                let _ = tx.send(Answer {
                    say: reply.say,
                    effects: reply.effects,
                    expects_reply: reply.expects_reply,
                });
                None
            }
            Self::Console => Some(reply),
        }
    }
}

/// One thing a speaker said that needs the language model.
#[derive(Debug)]
pub struct Request {
    /// Who said it.
    pub speaker: SpeakerId,
    /// What they said.
    pub line: String,
    /// Cancel, and where plan and progress phrases go.
    pub ctl: TurnCtl,
    /// Where the answer goes.
    pub origin: Origin,
}

/// A call to a built-in tool, sent to the loop, which owns the state it changes.
#[derive(Debug)]
pub struct ToolReq {
    name: String,
    args: Value,
    reply: oneshot::Sender<ToolAnswer>,
}

#[derive(Debug)]
struct ToolAnswer {
    result: Result<String, String>,
    effects: Vec<Effect>,
}

impl ToolReq {
    /// Run the call on the assistant, which only the loop holds, and answer the turn that asked.
    pub async fn serve(self, assistant: &mut Assistant) {
        let result = assistant.call_tool(&self.name, &self.args).await;
        let effects = assistant.take_effects();
        let saved = assistant.take_save();
        // A model told "timer set" says so at once (a realtime session speaks it as it arrives), so the result
        // is held back until the change is on disk. That wait is in its own task: the loop does not stand
        // still for flash. If the change could not be saved, the result says so, so the model tells the person.
        match saved {
            Some(saved) => match saved.outcome_now() {
                Some(outcome) => self.answer(unsaved_note(result, outcome), effects),
                None => {
                    tokio::spawn(async move {
                        let outcome = saved.wait().await;
                        self.answer(unsaved_note(result, outcome), effects);
                    });
                }
            },
            None => self.answer(result, effects),
        }
    }

    fn answer(self, result: Result<String, String>, effects: Vec<Effect>) {
        // The tool has run; if the turn that asked is gone, its result and effects have nowhere to go. Say so
        // once (the tool name only, never its arguments) so a timer that rang for nobody can be traced.
        if self.reply.send(ToolAnswer { result, effects }).is_err() {
            crate::diag::error(&format!(
                "the turn that asked for `{}` ended before its result came back; the tool had already run",
                self.name
            ));
        }
    }
}

/// A tool result with the warning that the change it reports could not be saved, when that is so. A call that
/// failed reports no change, so its message is left alone.
fn unsaved_note(
    result: Result<String, String>,
    outcome: Result<(), crate::app::persist::SaveError>,
) -> Result<String, String> {
    match (result, outcome) {
        (Ok(text), Err(_)) => Ok(format!("{text} {}", crate::assistant::NOT_SAVED_NOTE)),
        (result, _) => result,
    }
}

/// The built-in tools as a turn task sees them: the list as it was when the turn began, the tier and
/// wording rules (pure functions), and the way to the loop for a call.
#[derive(Debug)]
pub struct BuiltinLink {
    specs: Vec<ToolSpec>,
    tx: mpsc::Sender<ToolReq>,
    effects: Vec<Effect>,
}

impl BuiltinLink {
    /// Effects the calls of this turn produced, in order.
    pub fn take_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.effects)
    }
}

impl ToolHost for BuiltinLink {
    fn tool_specs(&self) -> Vec<ToolSpec> {
        self.specs.clone()
    }

    fn risk(&self, name: &str, args: &Value) -> Risk {
        builtin_risk(name, args)
    }

    fn returns_untrusted_text(&self, name: &str) -> bool {
        builtin_returns_untrusted_text(name)
    }

    fn describe_action(&self, name: &str, args: &Value) -> String {
        builtin_describe_action(name, args)
    }

    async fn call_tool(&mut self, name: &str, args: &Value) -> Result<String, String> {
        let (reply, answer) = oneshot::channel();
        let req = ToolReq {
            name: name.to_owned(),
            args: args.clone(),
            reply,
        };
        if self.tx.send(req).await.is_err() {
            return Err("the assistant is shutting down".into());
        }
        match answer.await {
            Ok(a) => {
                self.effects.extend(a.effects);
                a.result
            }
            Err(_) => Err("the assistant is shutting down".into()),
        }
    }
}

/// Everything a turn needs that the loop owned, copied or shared at the moment the turn started.
pub struct TurnKit<B> {
    /// The model.
    pub backend: Arc<B>,
    /// The turn limits and model names in force now.
    pub cfg: TurnConfig,
    /// The daily counters (a handle on the loop's own).
    pub usage: UsageTracker,
    /// The HTTP tools.
    pub http: Arc<HttpTools>,
    /// The MCP tools.
    pub mcp: Arc<McpClient>,
    /// A read-only copy of what is turned off, the chosen tiers and the skills' tool lists.
    pub ext: Arc<Extensions>,
    /// The built-in tools on offer now.
    pub builtin: Vec<ToolSpec>,
    /// The system prompt: the persona and the enabled skills.
    pub system: String,
    /// Local day number and the clock reading at the start.
    pub day: i64,
    /// Wall-clock seconds on the assistant's clock at the start (for timestamps).
    pub now: Ts,
    /// Monotonic seconds at the start (for confirmation windows and idle time).
    pub mono: i64,
}

impl<B> std::fmt::Debug for TurnKit<B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TurnKit").finish_non_exhaustive()
    }
}

/// What a finished turn hands back to the loop.
#[derive(Debug)]
pub struct Finished {
    /// Whose turn it was.
    pub speaker: SpeakerId,
    /// Their conversation, to put back.
    pub conv: Conversation,
    /// What to say.
    pub reply: Reply,
    /// Where it goes.
    pub origin: Origin,
    /// Every model attempt failed, in one plain line for the log.
    pub failed: Option<String>,
}

/// Run one turn to its end. This is the body of the task.
async fn run_one<B>(
    kit: TurnKit<B>,
    mut conv: Conversation,
    speaker: SpeakerId,
    line: String,
    ctl: TurnCtl,
    origin: Origin,
    tx: mpsc::Sender<ToolReq>,
) -> Finished
where
    B: ChatBackend + Send + Sync + 'static,
{
    let TurnKit {
        backend,
        cfg,
        mut usage,
        http,
        mcp,
        ext,
        builtin,
        system,
        day,
        now,
        mono,
    } = kit;
    conv.set_system(system);
    let mut link = BuiltinLink {
        specs: builtin,
        tx,
        effects: Vec::new(),
    };
    let r = {
        let mut host = WithPolicy {
            inner: &mut WithMcp {
                inner: &mut WithHttp {
                    inner: &mut link,
                    http: &http,
                },
                mcp: &mcp,
            },
            ext: &ext,
            mcp: &mcp,
            scope: None,
        };
        let inputs = TurnInputs {
            host: &mut host,
            conv: &mut conv,
            cfg: &cfg,
            usage: &mut usage,
            day,
            now,
            mono,
        };
        run_turn_with(&ctl, &*backend, inputs, &line).await
    };
    let failed = match &r.end {
        TurnEnd::Failed(e) => Some(format!("{e}")),
        _ => None,
    };
    // Without a speech sink (the console, a recorded utterance) plan and progress phrases come first.
    let say = if ctl.speak.is_some() || r.early.is_empty() {
        r.speech
    } else {
        format!("{} {}", r.early.join(" "), r.speech)
    };
    Finished {
        speaker,
        conv,
        reply: Reply {
            say,
            effects: link.take_effects(),
            expects_reply: r.expects_reply,
        },
        origin,
        failed,
    }
}

/// Why a task ended, for the realtime sessions the loop also runs as tasks.
#[derive(Debug)]
pub struct Ended;

#[derive(Debug)]
enum Outcome {
    Turn(Box<Finished>),
    Other(Ended),
}

#[derive(Debug)]
enum Running {
    Turn(SpeakerId),
    Other,
}

/// What a realtime voice session needs that the loop owned, taken when the session starts (ADR-0020).
#[cfg(feature = "realtime")]
pub struct RtKit {
    /// The settings in force.
    pub settings: crate::rtvoice::RtSettings,
    /// The API key. Never printed.
    pub key: Option<String>,
    /// The persona, added to the engine's instructions.
    pub persona: String,
    /// The turn limits (the gate uses them for the model's function calls).
    pub cfg: TurnConfig,
    /// The daily counters (a handle on the loop's own): the audio cost is added to them as it runs.
    pub usage: UsageTracker,
    /// The HTTP tools.
    pub http: Arc<HttpTools>,
    /// The MCP tools.
    pub mcp: Arc<McpClient>,
    /// A read-only copy of the policy.
    pub ext: Arc<Extensions>,
    /// The built-in tools on offer now.
    pub builtin: Vec<ToolSpec>,
    /// Local day number at the start.
    pub day: i64,
    /// The assistant's clock.
    pub clock: Arc<dyn crate::sched::Clock + Send + Sync>,
    /// A fresh conversation: it holds the transcript and any held action in memory for the session only.
    pub conv: Conversation,
    /// The realtime handle, to record how the session ended.
    pub rt: crate::rtvoice::Realtime,
}

#[cfg(feature = "realtime")]
impl std::fmt::Debug for RtKit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RtKit").finish_non_exhaustive()
    }
}

/// Run one realtime session to its end. The function calls its model makes go through the same gate as any
/// turn; built-in tool calls are served by the loop (REQ-RTV-009, REQ-AST-027).
#[cfg(feature = "realtime")]
async fn run_realtime(kit: RtKit, job: crate::rtvoice::Job, tx: mpsc::Sender<ToolReq>) {
    use crate::realtime::{Env, Io, serve::serve};
    let RtKit {
        settings,
        key,
        persona,
        cfg,
        mut usage,
        http,
        mcp,
        ext,
        builtin,
        day,
        clock,
        mut conv,
        rt,
    } = kit;
    let mut link = BuiltinLink {
        specs: builtin,
        tx,
        effects: Vec::new(),
    };
    let now = move || (day, clock.now(), clock.monotonic_secs());
    let io = Io {
        mic: job.mic,
        speaker: job.speaker,
    };
    let finish = {
        let mut host = WithPolicy {
            inner: &mut WithMcp {
                inner: &mut WithHttp {
                    inner: &mut link,
                    http: &http,
                },
                mcp: &mcp,
            },
            ext: &ext,
            mcp: &mcp,
            scope: None,
        };
        let mut env = Env {
            host: &mut host,
            conv: &mut conv,
            turn_cfg: &cfg,
            usage: &mut usage,
            clock: &now,
        };
        serve(&settings, key.as_deref(), &persona, io, &mut env).await
    };
    if let Some(e) = &finish.error {
        crate::diag::error(&format!("realtime voice: {e}"));
    }
    rt.set_error(finish.error.clone());
    let _ = job.done.send(crate::rtvoice::Done {
        speak: finish.speak,
        failed: finish.failed,
        effects: link.take_effects(),
    });
}

#[cfg(feature = "realtime")]
impl Hub {
    /// Run a realtime session beside the loop. The loop is told when it ends ([`Event::Other`]).
    pub fn spawn_realtime(&mut self, kit: RtKit, job: crate::rtvoice::Job) {
        let tx = self.tool_tx.clone();
        self.spawn_other(run_realtime(kit, job, tx));
    }
}

/// What the loop learns from the hub.
#[derive(Debug)]
pub enum Event {
    /// A turn wants a built-in tool called.
    Tool(ToolReq),
    /// A turn is over.
    Turn(Box<Finished>),
    /// A background task other than a turn is over (a realtime session).
    Other(Ended),
    /// A turn's task died without answering; the speaker starts afresh.
    Lost(SpeakerId),
}

/// What happened to a request.
#[derive(Debug)]
pub enum Submitted {
    /// A turn task is running for it.
    Started,
    /// The speaker's earlier turn is still running; this one waits its turn.
    Queued,
    /// Neither: the speaker already has requests waiting, or every place is taken. Say so.
    Refused(Request),
}

/// The loop's side of the turns: the conversations, the running tasks and the channel for tool calls.
pub struct Hub {
    speakers: Speakers<Request>,
    tool_tx: mpsc::Sender<ToolReq>,
    tool_rx: mpsc::Receiver<ToolReq>,
    tasks: JoinSet<Outcome>,
    running: HashMap<Id, Running>,
}

impl std::fmt::Debug for Hub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hub")
            .field("running", &self.running.len())
            .finish_non_exhaustive()
    }
}

impl Hub {
    /// A hub whose speakers start with the conversation `make` builds.
    pub fn new(make: impl Fn() -> Conversation + Send + 'static) -> Self {
        Self::with_speakers(Speakers::new(make))
    }

    /// A hub over a given set of speakers.
    #[must_use]
    pub fn with_speakers(speakers: Speakers<Request>) -> Self {
        let (tool_tx, tool_rx) = mpsc::channel(16);
        Self {
            speakers,
            tool_tx,
            tool_rx,
            tasks: JoinSet::new(),
            running: HashMap::new(),
        }
    }

    /// How many tasks are running.
    #[must_use]
    pub fn active(&self) -> usize {
        self.running.len()
    }

    /// A sender for tasks that need built-in tool calls served by the loop (a realtime session).
    #[must_use]
    pub fn tool_sender(&self) -> mpsc::Sender<ToolReq> {
        self.tool_tx.clone()
    }

    /// Start a turn for `req` now if its speaker is free, or hold it until the speaker's turn ends. `kit`
    /// builds the snapshot and is called only when a task starts. `now` is monotonic seconds ([`crate::sched::Clock::monotonic_secs`]).
    pub fn submit<B>(
        &mut self,
        req: Request,
        now: i64,
        kit: impl FnOnce() -> TurnKit<B>,
    ) -> Submitted
    where
        B: ChatBackend + Send + Sync + 'static,
    {
        match self.speakers.checkout(&req.speaker, now) {
            Checkout::Ready(conv) => {
                self.start(*conv, req, kit());
                Submitted::Started
            }
            Checkout::Busy => {
                let speaker = req.speaker.clone();
                match self.speakers.defer(&speaker, req) {
                    Ok(()) => Submitted::Queued,
                    Err(req) => Submitted::Refused(req),
                }
            }
            Checkout::Full => Submitted::Refused(req),
        }
    }

    fn start<B>(&mut self, conv: Conversation, req: Request, kit: TurnKit<B>)
    where
        B: ChatBackend + Send + Sync + 'static,
    {
        let Request {
            speaker,
            line,
            ctl,
            origin,
        } = req;
        let tx = self.tool_tx.clone();
        let id = speaker.clone();
        let handle = self.tasks.spawn(async move {
            Outcome::Turn(Box::new(
                run_one(kit, conv, speaker, line, ctl, origin, tx).await,
            ))
        });
        self.running.insert(handle.id(), Running::Turn(id));
    }

    /// Run any future beside the loop and report its end as [`Event::Other`].
    pub fn spawn_other(&mut self, fut: impl std::future::Future<Output = ()> + Send + 'static) {
        let handle = self.tasks.spawn(async move {
            fut.await;
            Outcome::Other(Ended)
        });
        self.running.insert(handle.id(), Running::Other);
    }

    /// The next thing that happened. Cancel safe: use it as one arm of the loop's `select!`.
    pub async fn next(&mut self) -> Event {
        loop {
            tokio::select! {
                Some(req) = self.tool_rx.recv() => return Event::Tool(req),
                done = self.tasks.join_next_with_id(), if !self.tasks.is_empty() => {
                    match done {
                        Some(Ok((id, out))) => {
                            self.running.remove(&id);
                            return match out {
                                Outcome::Turn(f) => Event::Turn(f),
                                Outcome::Other(e) => Event::Other(e),
                            };
                        }
                        Some(Err(e)) => {
                            if let Some(Running::Turn(speaker)) = self.running.remove(&e.id()) {
                                return Event::Lost(speaker);
                            }
                            return Event::Other(Ended);
                        }
                        None => {}
                    }
                }
            }
        }
    }

    /// Take a finished turn: put the conversation back and return what to deliver and the speaker's next
    /// waiting request, if there is one.
    pub fn finish(&mut self, f: Finished, now: i64) -> (Origin, Reply, Option<Request>) {
        self.speakers.checkin(&f.speaker, f.conv, now);
        let next = self.speakers.next_deferred(&f.speaker);
        (f.origin, f.reply, next)
    }

    /// A speaker whose task died starts again with a fresh conversation. Returns its next waiting request.
    pub fn lost(&mut self, speaker: &str, now: i64) -> Option<Request> {
        self.speakers.reset(speaker, now);
        self.speakers.next_deferred(speaker)
    }

    /// Drop conversations that have been idle too long.
    pub fn sweep(&mut self, now: i64) {
        self.speakers.sweep(now);
    }

    /// The conversations, for tests and status.
    #[must_use]
    pub fn speakers(&self) -> &Speakers<Request> {
        &self.speakers
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::too_many_lines)]
mod tests;
