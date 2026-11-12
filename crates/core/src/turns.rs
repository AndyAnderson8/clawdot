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
    ChatBackend, Conversation, Risk, ToolHost, TurnConfig, TurnCtl, TurnEnd, run_turn_ctl,
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
        let _ = self.reply.send(ToolAnswer { result, effects });
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
    /// Seconds on the assistant's clock at the start.
    pub now: Ts,
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
        run_turn_ctl(
            &ctl, &*backend, &mut host, &mut conv, &cfg, &mut usage, day, now, &line,
        )
        .await
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
    let now = move || (day, clock.now());
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
    /// builds the snapshot and is called only when a task starts. `now` is seconds on the assistant's clock.
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
mod tests {
    use super::*;
    use crate::llm::{Completion, LlmError, Role, ToolCall};
    use crate::sched::{Clock, FixedOffset, Scheduler};
    use crate::session::Tier;
    use crate::usage::UsageLimits;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    /// A clock that runs ten times faster than the real one, so a "two second" timer rings after 200 ms.
    struct FastClock(Instant);
    impl Clock for FastClock {
        fn now(&self) -> Ts {
            1_800_000_000 + i64::try_from(self.0.elapsed().as_millis() / 100).unwrap()
        }
    }

    /// A model that answers from what it is asked: `call NAME JSON` makes that call, anything with `slow`
    /// takes a while, a tool result is repeated, and anything else is echoed.
    struct Fake {
        delay: Duration,
        systems: Mutex<Vec<String>>,
        requests: Mutex<u32>,
    }

    impl Fake {
        fn new(delay_ms: u64) -> Arc<Self> {
            Arc::new(Self {
                delay: Duration::from_millis(delay_ms),
                systems: Mutex::new(Vec::new()),
                requests: Mutex::new(0),
            })
        }
    }

    impl ChatBackend for Fake {
        async fn complete(&self, req: &crate::llm::ChatRequest) -> Result<Completion, LlmError> {
            *self.requests.lock().unwrap() += 1;
            self.systems
                .lock()
                .unwrap()
                .push(req.messages[0].content.clone());
            let last = req.messages.last().unwrap();
            let text = |t: String| {
                Ok(Completion {
                    text: t,
                    ..Completion::default()
                })
            };
            if last.role == Role::Tool {
                return text(format!("result: {}", last.content));
            }
            if last.content.contains("slow") {
                tokio::time::sleep(self.delay).await;
            }
            if let Some(rest) = last.content.strip_prefix("call ") {
                let (name, args) = rest.split_once(' ').unwrap_or((rest, "{}"));
                return Ok(Completion {
                    tool_calls: vec![ToolCall {
                        id: "c1".into(),
                        name: name.into(),
                        arguments: args.into(),
                    }],
                    ..Completion::default()
                });
            }
            text(format!("heard {}", last.content))
        }
    }

    /// The loop's side, as `main` has it, in miniature.
    struct Lab {
        assistant: Assistant,
        hub: Hub,
        backend: Arc<Fake>,
        usage: UsageTracker,
        http: Arc<HttpTools>,
        mcp: Arc<McpClient>,
        ext: Extensions,
        ext_view: Arc<Extensions>,
        persona: String,
        builtin: Vec<String>,
        started: Instant,
        finished: Vec<(Duration, SpeakerId, String)>,
        fired: Vec<Duration>,
        answers: Vec<oneshot::Receiver<Answer>>,
        others: usize,
    }

    fn env(_: &str) -> Option<String> {
        None
    }

    impl Lab {
        async fn new(delay_ms: u64) -> Self {
            let clock: Arc<dyn Clock + Send + Sync> = Arc::new(FastClock(Instant::now()));
            let assistant = Assistant::new(
                clock,
                Box::new(FixedOffset(0)),
                Scheduler::new(16),
                None,
                None,
            );
            let builtin: Vec<String> = ToolHost::tool_specs(&assistant)
                .into_iter()
                .map(|t| t.name)
                .collect();
            let http = HttpTools::new(Vec::new(), &builtin, env).unwrap();
            let (mcp, _) = McpClient::connect(Vec::new(), &builtin, env).await.unwrap();
            let ext = Extensions::in_memory();
            let ext_view = Arc::new(ext.snapshot());
            Self {
                assistant,
                hub: Hub::new(|| Conversation::standard("persona")),
                backend: Fake::new(delay_ms),
                usage: UsageTracker::new(UsageLimits::default()),
                http: Arc::new(http),
                mcp: Arc::new(mcp),
                ext,
                ext_view,
                persona: "persona".into(),
                builtin,
                started: Instant::now(),
                finished: Vec::new(),
                fired: Vec::new(),
                answers: Vec::new(),
                others: 0,
            }
        }

        fn kit(&self) -> TurnKit<Fake> {
            TurnKit {
                backend: Arc::clone(&self.backend),
                cfg: TurnConfig {
                    model: "m".into(),
                    ..TurnConfig::default()
                },
                usage: self.usage.clone(),
                http: Arc::clone(&self.http),
                mcp: Arc::clone(&self.mcp),
                ext: Arc::clone(&self.ext_view),
                builtin: ToolHost::tool_specs(&self.assistant),
                system: self.persona.clone(),
                day: 1,
                now: 1_800_000_000,
            }
        }

        fn say(&mut self, speaker: &str, line: &str) {
            let (tx, rx) = oneshot::channel();
            let req = Request {
                speaker: speaker.into(),
                line: line.into(),
                ctl: TurnCtl::default(),
                origin: Origin::Ask(tx),
            };
            let kit = self.kit();
            assert!(matches!(
                self.hub.submit(req, 0, || kit),
                Submitted::Started | Submitted::Queued
            ));
            self.answers.push(rx);
        }

        async fn tool_off(&mut self, op: crate::extensions::ExtOp) {
            self.ext
                .apply(
                    op,
                    Arc::make_mut(&mut self.http),
                    Arc::make_mut(&mut self.mcp),
                    &self.builtin,
                    env,
                )
                .await
                .unwrap();
            self.ext_view = Arc::new(self.ext.snapshot());
        }

        /// Run the loop until `n` turns have finished, ringing timers every 20 ms as the real one does.
        async fn run_until(&mut self, n: usize) {
            self.pump(|lab| lab.finished.len() >= n).await;
        }

        async fn pump(&mut self, done: impl Fn(&Self) -> bool) {
            let mut tick = tokio::time::interval(Duration::from_millis(20));
            while !done(self) {
                tokio::select! {
                    ev = self.hub.next() => match ev {
                        Event::Tool(req) => req.serve(&mut self.assistant).await,
                        Event::Turn(f) => {
                            let speaker = f.speaker.clone();
                            let (origin, reply, next) = self.hub.finish(*f, 0);
                            self.finished.push((self.started.elapsed(), speaker, reply.say.clone()));
                            origin.deliver(reply);
                            if let Some(next) = next {
                                let kit = self.kit();
                                let _ = self.hub.submit(next, 0, || kit);
                            }
                        }
                        Event::Other(_) => self.others += 1,
                        Event::Lost(_) => {}
                    },
                    _ = tick.tick() => {
                        if !self.assistant.tick().is_empty() {
                            self.fired.push(self.started.elapsed());
                        }
                    }
                }
            }
        }

        fn say_of(&self, speaker: &str) -> Vec<String> {
            self.finished
                .iter()
                .filter(|(_, s, _)| s == speaker)
                .map(|(_, _, t)| t.clone())
                .collect()
        }
    }

    // REQ-AST-027
    #[tokio::test(flavor = "current_thread")]
    async fn a_timer_rings_on_time_while_a_slow_model_turn_is_in_flight() {
        let mut lab = Lab::new(900).await;
        lab.assistant
            .call_tool("set_timer", &serde_json::json!({"seconds": 2}))
            .await
            .unwrap();
        lab.say("console", "that is slow");
        lab.run_until(1).await;
        let (done_at, _, _) = &lab.finished[0];
        assert!(
            done_at >= &Duration::from_millis(850),
            "the turn took its time"
        );
        assert_eq!(lab.fired.len(), 1, "the timer rang");
        assert!(
            lab.fired[0] < Duration::from_millis(500),
            "it rang at {:?}, long before the model answered at {done_at:?}",
            lab.fired[0]
        );
    }

    // REQ-AST-027, REQ-CFG-039: the loop applies a portal change while a turn runs; the running turn keeps
    // the snapshot it began with and the next turn sees the change.
    #[tokio::test(flavor = "current_thread")]
    async fn a_portal_change_is_applied_during_a_turn_and_reaches_the_next_one() {
        let mut lab = Lab::new(600).await;
        lab.say("console", "slow one");
        // Let the turn begin, then change the persona and switch a tool off, as the portal would.
        tokio::time::sleep(Duration::from_millis(100)).await;
        let before = Instant::now();
        lab.persona = "a new persona".into();
        lab.tool_off(crate::extensions::ExtOp::ToolEnable(
            "set_timer".into(),
            false,
        ))
        .await;
        assert!(
            before.elapsed() < Duration::from_millis(200),
            "the change did not wait for the turn"
        );
        assert_eq!(lab.hub.active(), 1, "the turn is still running");
        lab.run_until(1).await;
        lab.say("console", "second");
        lab.run_until(2).await;
        let systems = lab.backend.systems.lock().unwrap().clone();
        assert_eq!(systems.first().map(String::as_str), Some("persona"));
        assert_eq!(
            systems.last().map(String::as_str),
            Some("a new persona"),
            "the next turn has the new persona"
        );
    }

    // REQ-AST-026, REQ-SEC-011
    #[tokio::test(flavor = "current_thread")]
    async fn two_speakers_have_separate_conversations_and_separate_held_confirmations() {
        let mut lab = Lab::new(10).await;
        // Make the timer tool costly, so a call to it is held for a yes.
        lab.tool_off(crate::extensions::ExtOp::ToolTier(
            "set_timer".into(),
            Some(Tier::Costly),
        ))
        .await;

        lab.say("sat:a", r#"call set_timer {"seconds": 600}"#);
        lab.run_until(1).await;
        assert!(
            lab.say_of("sat:a")[0].contains("Yes or no?"),
            "{:?}",
            lab.say_of("sat:a")
        );
        assert!(lab.assistant.scheduled().is_empty(), "nothing has run yet");

        // B says yes. It has nothing held, so the yes is just words for the model, and nothing runs.
        lab.say("sat:b", "yes");
        lab.run_until(2).await;
        assert_eq!(lab.say_of("sat:b"), vec!["heard yes".to_owned()]);
        assert!(
            lab.assistant.scheduled().is_empty(),
            "B's yes released nothing"
        );

        // A says yes: its own held action runs, exactly once.
        lab.say("sat:a", "yes");
        lab.run_until(3).await;
        assert_eq!(lab.assistant.scheduled().len(), 1);
        assert_eq!(lab.hub.speakers().len(), 2);
    }

    // REQ-AST-026: a speaker's second request waits for its first; another speaker does not.
    #[tokio::test(flavor = "current_thread")]
    async fn a_speaker_waits_for_its_own_turn_but_not_for_anyone_elses() {
        let mut lab = Lab::new(500).await;
        lab.say("sat:a", "slow first");
        lab.say("sat:a", "second from a");
        lab.say("sat:b", "quick from b");
        lab.run_until(3).await;
        let order: Vec<&str> = lab.finished.iter().map(|(_, s, _)| s.as_str()).collect();
        assert_eq!(order[0], "sat:b", "b was not held up by a: {order:?}");
        assert_eq!(
            lab.say_of("sat:a"),
            vec![
                "heard slow first".to_owned(),
                "heard second from a".to_owned()
            ],
            "a's requests ran in order"
        );
    }

    // REQ-AST-027: a built-in tool call changes the loop's state and counts as the model rounds it took.
    #[tokio::test(flavor = "current_thread")]
    async fn a_built_in_tool_runs_in_the_loop_and_counts_one_request_per_model_round() {
        let mut lab = Lab::new(10).await;
        lab.say("console", r#"call set_timer {"seconds": 600}"#);
        lab.run_until(1).await;
        assert_eq!(
            lab.assistant.scheduled().len(),
            1,
            "the loop owns the timer"
        );
        assert!(lab.say_of("console")[0].starts_with("result: Timer set"));
        assert_eq!(
            *lab.backend.requests.lock().unwrap(),
            2,
            "a call round and an answer round"
        );
        assert_eq!(lab.usage.requests_today(), 2);
    }

    // REQ-AST-022: cancel still stops a turn that runs in its own task.
    #[tokio::test(flavor = "current_thread")]
    async fn cancel_stops_a_turn_that_runs_beside_the_loop() {
        let mut lab = Lab::new(5_000).await;
        let ctl = TurnCtl::default();
        let cancel = ctl.cancel.clone();
        let (tx, rx) = oneshot::channel();
        let kit = lab.kit();
        let _ = lab.hub.submit(
            Request {
                speaker: "sat:a".into(),
                line: "slow, very slow".into(),
                ctl,
                origin: Origin::Ask(tx),
            },
            0,
            || kit,
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
        cancel.cancel();
        lab.run_until(1).await;
        let said = rx.await.unwrap().say;
        assert!(
            said.to_lowercase().contains("stopped") || said.to_lowercase().contains("cancel"),
            "{said}"
        );
        assert!(
            lab.started.elapsed() < Duration::from_secs(2),
            "it did not wait for the model"
        );
    }

    // REQ-AST-027, REQ-RTV-009: a realtime session is a task of its own. A timer rings in the middle of it,
    // and a built-in tool its model calls is run by the loop.
    #[cfg(feature = "realtime")]
    #[tokio::test(flavor = "current_thread")]
    async fn a_realtime_session_does_not_delay_a_timer_and_its_calls_reach_the_loop() {
        use crate::realtime::ws::accept;
        use crate::rtvoice::{Job, Realtime, RtSettings};
        let mut lab = Lab::new(0).await;
        lab.assistant
            .call_tool("set_timer", &serde_json::json!({"seconds": 2}))
            .await
            .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let (mut ws, _) = accept(tcp).await.unwrap();
            ws.recv().await.unwrap(); // the session setup
            let call = serde_json::json!({"type": "response.done", "response": {
                "status": "completed", "output": [{
                    "type": "function_call", "status": "completed", "call_id": "c1",
                    "name": "set_timer", "arguments": "{\"seconds\": 600}"}]}});
            ws.send_text(&call.to_string()).await.unwrap();
            ws.recv().await.unwrap(); // the function output
            ws.recv().await.unwrap(); // the request to speak
            tokio::time::sleep(Duration::from_millis(900)).await;
            ws.send_close().await.unwrap();
        });
        let settings = RtSettings {
            enabled: true,
            url: Some(format!("ws://127.0.0.1:{port}/v1/realtime")),
            model: Some("m".into()),
            ..RtSettings::default()
        };
        let kit = crate::turns::RtKit {
            rt: Realtime::new(settings.clone()),
            settings,
            key: Some("sk-test".into()),
            persona: "persona".into(),
            cfg: TurnConfig::default(),
            usage: lab.usage.clone(),
            http: Arc::clone(&lab.http),
            mcp: Arc::clone(&lab.mcp),
            ext: Arc::clone(&lab.ext_view),
            builtin: ToolHost::tool_specs(&lab.assistant),
            day: 1,
            clock: Arc::new(FastClock(Instant::now())),
            conv: Conversation::standard("persona"),
        };
        let (_mic_tx, mic) = mpsc::channel(8);
        let (speaker, _spk_rx) = mpsc::channel(8);
        let (done, done_rx) = oneshot::channel();
        lab.hub.spawn_realtime(
            kit,
            Job {
                speaker_id: "sat:a".into(),
                mic,
                speaker,
                done,
            },
        );
        lab.pump(|l| l.others >= 1).await;
        let ended = lab.started.elapsed();
        assert!(
            ended >= Duration::from_millis(850),
            "the session took its time"
        );
        assert_eq!(lab.fired.len(), 1, "the timer rang during the session");
        assert!(
            lab.fired[0] < Duration::from_millis(500),
            "it rang at {:?}, session ended at {ended:?}",
            lab.fired[0]
        );
        assert_eq!(
            lab.assistant.scheduled().len(),
            1,
            "the call the model made set a timer in the loop"
        );
        let done = done_rx.await.unwrap();
        assert!(!done.failed, "{:?}", done.speak);
        assert_eq!(lab.usage.requests_today(), 1, "the session is one request");
    }

    // REQ-AST-027: a task that dies hands the speaker a fresh start and does not wedge the loop.
    #[tokio::test(flavor = "current_thread")]
    async fn a_lost_turn_does_not_wedge_its_speaker() {
        let mut lab = Lab::new(10).await;
        let (tx, _rx) = oneshot::channel();
        let kit = lab.kit();
        let _ = lab.hub.submit(
            Request {
                speaker: "sat:a".into(),
                line: "slow".into(),
                ctl: TurnCtl::default(),
                origin: Origin::Ask(tx),
            },
            0,
            || kit,
        );
        lab.hub.tasks.abort_all();
        match lab.hub.next().await {
            Event::Lost(s) => {
                assert_eq!(s, "sat:a");
                assert!(lab.hub.lost(&s, 0).is_none());
            }
            other => panic!("expected a lost turn, got {other:?}"),
        }
        lab.say("sat:a", "after");
        lab.run_until(1).await;
        assert_eq!(lab.say_of("sat:a"), vec!["heard after".to_owned()]);
    }
}
