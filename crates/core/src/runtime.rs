//! The state the main loop owns and what it does with it (ADR-0021): the [`Brain`] (assistant, language model,
//! tools and usage counters), live settings (`apply_live`), the status snapshot for the web UI, and turn
//! dispatch (`dispatch`, `finish_turn`, `on_hub_event`). The binary's `main` wires these to its channels.

use crate::app::{Core, Job, Op};
use crate::assistant::Assistant;
use crate::cli::Args;
use crate::config::AgentConfig;
use crate::connects::{Connects, Done, Source};
use crate::extensions::{ExtOp, Extensions, WithPolicy};
use crate::httptools::{HttpTools, WithHttp};
use crate::mcp::{McpClient, WithMcp};
use crate::rtvoice::{self, Realtime, RtSettings};
use crate::sched::{Clock, Fired, FixedOffset, Ts, Zone};
use crate::seed::{agent_from, policy_from, quiet_from};
#[cfg(feature = "realtime")]
use crate::session::Conversation;
use crate::session::{ToolHost, TurnConfig, TurnCtl};
use crate::settings::{self, Group};
use crate::store::JsonStore;
use crate::transport::HttpBackend;
use crate::turns::{self, Event as HubEvent, Hub, Origin, Reply, Request, Submitted, TurnKit};
use crate::usage::UsageTracker;
use std::sync::Arc;
use std::time::Duration;

pub fn zone_for(name: Option<&str>) -> Result<Box<dyn Zone + Send + Sync>, String> {
    match name {
        None => Ok(Box::new(FixedOffset(0))),
        #[cfg(feature = "tz")]
        Some(n) => {
            crate::sched::IanaZone::new(n).map(|z| Box::new(z) as Box<dyn Zone + Send + Sync>)
        }
        #[cfg(not(feature = "tz"))]
        Some(_) => Err("this build has no time-zone database (feature `tz`)".to_owned()),
    }
}

/// Facts shown next to the settings but not edited there: the speech provider's models and where speakers
/// connect. Names of environment variables are shown; their values never are.
pub fn settings_extra(args: &Args) -> serde_json::Value {
    serde_json::json!({
        "voice_info": {
            "configured": args.speech.url.is_some(),
            "language": args.speech.language,
            "provider": args
                .speech
                .url
                .as_deref()
                .and_then(|u| crate::transport::Endpoint::parse(u).ok())
                .map(|e| e.host),
            "stt_model": args.speech.stt_model,
            "tts_model": args.speech.tts_model,
            "dialect": args.speech.dialect().name(),
        },
        "speech_key_env": args.speech.key_env,
        "speakers_listen": args.satellite.listen,
    })
}

/// The language model and everything needed to use it.
#[derive(Clone)]
pub struct Llm {
    pub(crate) backend: Arc<HttpBackend>,
    pub(crate) cfg: TurnConfig,
}

/// The turn settings: the defaults of REQ-AST-019 lowered or switched by the `agent` section.
pub fn agent_turn_config(
    model: &str,
    fallback: Option<&str>,
    summary: Option<&str>,
    max_tokens: u32,
    agent: &AgentConfig,
) -> TurnConfig {
    with_agent(
        TurnConfig {
            model: model.to_owned(),
            fallback_model: fallback.map(str::to_owned),
            max_tokens,
            summary_model: Some(summary.unwrap_or(model).to_owned()),
            ..TurnConfig::default()
        },
        agent,
    )
}

/// `cfg` with the limits and speech switches the `agent` settings give (the defaults where they say nothing).
pub fn with_agent(cfg: TurnConfig, agent: &AgentConfig) -> TurnConfig {
    let d = TurnConfig::default();
    TurnConfig {
        max_tool_loops: agent.max_rounds.unwrap_or(d.max_tool_loops),
        max_tool_calls: agent.max_tool_calls.unwrap_or(d.max_tool_calls),
        deadline_secs: agent.deadline_secs.unwrap_or(d.deadline_secs),
        tool_timeout: agent
            .tool_timeout_secs
            .map_or(d.tool_timeout, Duration::from_secs),
        plan_speech: agent.plan_speech.unwrap_or(d.plan_speech),
        progress_speech: agent.progress_speech.unwrap_or(d.progress_speech),
        summarize_over: agent.summarize_over_chars.unwrap_or(d.summarize_over),
        ..cfg
    }
}

/// Build the language model client from the effective settings, or `None` when none is set.
pub fn build_llm(
    url: Option<&str>,
    model: Option<&str>,
    secrets: &crate::secrets::Secrets,
    env: &crate::env::Env,
    key_env: &str,
    max_tokens: u32,
    agent: &AgentConfig,
) -> Result<Option<Llm>, String> {
    let (Some(url), Some(model)) = (url, model) else {
        return Ok(None);
    };
    // A key pasted into the portal wins; otherwise the environment variable that was named.
    let key = secrets
        .get(crate::secrets::LLM_KEY)
        .unwrap_or_else(|| env.get(key_env).unwrap_or_default());
    let backend = Arc::new(HttpBackend::new(url, &key, Duration::from_secs(30))?);
    Ok(Some(Llm {
        backend,
        cfg: agent_turn_config(model, None, None, max_tokens, agent),
    }))
}

/// Switch to the new values of the settings that apply without a restart (REQ-CFG-039). `applied` holds
/// what is already in effect, so only real changes do any work.
pub fn apply_live(
    brain: &mut Brain,
    core: &Core,
    live: &serde_json::Map<String, serde_json::Value>,
    applied: &mut serde_json::Map<String, serde_json::Value>,
) {
    let changed = |k: &str| live.get(k) != applied.get(k);
    let text = |k: &str| live.get(k).and_then(|v| v.as_str());
    if changed("zone") {
        let z = text("zone").filter(|z| *z != "UTC");
        match zone_for(z) {
            Ok(zone) => brain.assistant.set_zone(zone),
            Err(e) => crate::diag::error(&format!("cannot switch time zone: {e}")),
        }
    }
    if changed("persona")
        && let Some(p) = text("persona")
    {
        p.clone_into(&mut brain.persona);
    }
    if changed("daily_spend_usd") {
        let mut l = brain.usage.limits();
        l.daily_spend_cap_usd = live
            .get("daily_spend_usd")
            .and_then(serde_json::Value::as_f64);
        brain.usage.set_limits(l);
    }
    if settings::group_changed(Group::Agent, live, applied) {
        brain.agent = agent_from(live, brain.agent.summarize_over_chars);
        if let Some(llm) = &mut brain.llm {
            llm.cfg = with_agent(llm.cfg.clone(), &brain.agent);
        }
    }
    if settings::group_changed(Group::Speech, live, applied) {
        core.speech_handles().apply(live);
    }
    if settings::group_changed(Group::Realtime, live, applied) {
        brain.rt.set_settings(RtSettings::from_live(live));
    }
    #[cfg(feature = "tls")]
    if changed("update_check")
        && let Some(u) = core.updater()
    {
        u.set_enabled(
            live.get("update_check")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true),
        );
    }
    if changed("notes")
        && let Some(n) = core.notes()
    {
        n.set_enabled(
            live.get("notes")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
        );
    }
    if settings::group_changed(Group::Policy, live, applied)
        && let Some(a) = core.audit()
    {
        a.set_policy(policy_from(live));
    }
    if settings::group_changed(Group::Quiet, live, applied) {
        core.set_quiet_hours(quiet_from(live));
    }
    if settings::group_changed(Group::Llm, live, applied) {
        let max_tokens = brain.usage.limits().max_output_tokens_per_turn;
        match build_llm(
            text("llm_url"),
            text("llm_model"),
            core.secrets(),
            core.env(),
            text("llm_key_env").unwrap_or("OPENROUTER_API_KEY"),
            max_tokens,
            &brain.agent,
        ) {
            Ok(l) => brain.llm = l,
            Err(e) => crate::diag::error(&format!("cannot switch language model: {e}")),
        }
    }
    *applied = live.clone();
}

/// Everything needed to answer one thing the user said, whether it was typed or spoken.
pub struct Brain {
    pub(crate) assistant: Assistant,
    pub(crate) llm: Option<Llm>,
    /// The daily counters. Turns hold handles on the same ones; only this loop saves them.
    pub(crate) usage: UsageTracker,
    pub(crate) usage_store: JsonStore,
    /// Where the counters are written without the loop waiting; `None` saves inline.
    pub(crate) writer: Option<Arc<crate::app::persist::Writer>>,
    /// The HTTP and MCP clients. A turn takes a clone of the `Arc` when it starts and calls the tools itself.
    pub(crate) http_tools: Arc<HttpTools>,
    pub(crate) mcp: Arc<McpClient>,
    pub(crate) clock: Arc<dyn Clock + Send + Sync>,
    /// Tools, connections, and skills managed from the portal.
    pub(crate) ext: Extensions,
    /// A read-only copy of `ext` for the turns, replaced whenever `ext` changes.
    pub(crate) ext_view: Arc<Extensions>,
    /// Names of the built-in tools.
    pub(crate) builtin: Vec<String>,
    /// The system prompt without skills.
    pub(crate) persona: String,
    /// Turn limits, speech while working and notes.
    pub(crate) agent: AgentConfig,
    /// A speech provider (address and speech model) is configured, so a speaker has a voice (REQ-CFG-070).
    pub(crate) voice_configured: bool,
    /// The optional realtime voice: its settings, status and the way sessions reach this loop.
    pub(crate) rt: Realtime,
    /// MCP connections being made beside the loop, for servers added in the portal.
    pub(crate) connects: Connects,
}

/// What the portal is told about the speaker (REQ-CFG-070).
#[derive(Debug)]
pub struct SpeakerView {
    /// Some satellite could speak right now (idle and unmuted).
    pub available: bool,
    /// Speech is printed to the terminal, which counts as a speaker and needs no voice.
    pub console: bool,
    pub quiet: Option<crate::inbox::endpoint::QuietHours>,
}

impl Brain {
    /// A language model is configured.
    #[must_use]
    pub fn has_model(&self) -> bool {
        self.llm.is_some()
    }

    /// A handle on the language model, for work done beside the loop.
    #[must_use]
    pub fn model(&self) -> Option<Llm> {
        self.llm.clone()
    }

    /// Seconds now, on the assistant's clock.
    #[must_use]
    pub fn now(&self) -> Ts {
        self.clock.now()
    }

    /// The timers, alarms and reminders that fell due since the last call.
    pub fn tick(&mut self) -> Vec<Fired> {
        self.assistant.tick()
    }

    /// The minute of the local day, for the quiet-hours check.
    #[must_use]
    pub fn minute_of_day(&self) -> u16 {
        self.assistant.minute_of_day()
    }

    /// The local day number.
    #[must_use]
    pub fn local_day(&self) -> i64 {
        self.assistant.local_day()
    }

    /// Handles on the daily counters and the file they are saved to, for the speaker task.
    #[must_use]
    pub fn usage_handles(&self) -> (UsageTracker, JsonStore) {
        (self.usage.clone(), self.usage_store.clone())
    }

    /// The realtime voice, for the satellites that start its sessions.
    #[must_use]
    pub fn realtime(&self) -> &Realtime {
        &self.rt
    }

    /// The receiving end of realtime session requests. Call once, at start-up.
    #[must_use]
    pub fn attach_realtime(&self) -> tokio::sync::mpsc::Receiver<rtvoice::Job> {
        self.rt.attach()
    }

    /// The receiving end of finished MCP connections, or `None` if it was already taken.
    #[must_use]
    pub fn attach_connects(&mut self) -> Option<tokio::sync::mpsc::Receiver<Done>> {
        self.connects.attach()
    }

    /// MCP connections are still being made.
    #[must_use]
    pub fn is_connecting(&self) -> bool {
        !self.connects.is_empty()
    }

    /// What the Tools, Connections, and Skills tabs show (REQ-CFG-029, 036). Holds no secrets.
    pub fn listing(&self) -> serde_json::Value {
        let specs = ToolHost::tool_specs(&self.assistant);
        // The declared tier of each built-in tool (a tool whose tier depends on its arguments is asked with none).
        let tiers = specs
            .iter()
            .map(|t| {
                (
                    t.name.clone(),
                    ToolHost::risk(&self.assistant, &t.name, &serde_json::json!({})).tier,
                )
            })
            .collect();
        self.ext
            .listing_with(&specs, &tiers, &self.http_tools, &self.mcp)
    }

    /// Carry out a change from the portal and refresh what the page shows.
    /// Keys named by the change are looked up in `env`, the core's snapshot of the environment.
    pub async fn ext_apply(
        &mut self,
        op: ExtOp,
        env: &crate::env::Env,
    ) -> Result<serde_json::Value, crate::app::Fail> {
        // A turn that is running holds its own copies of the clients; `make_mut` gives this loop a private
        // one to change, and the turn keeps what it began with. The copy is of pointers only: the tool
        // definitions, MCP tools and credentials inside are `Arc`s shared with the turn's copy.
        let applied = self
            .ext
            .apply(
                op,
                Arc::make_mut(&mut self.http_tools),
                Arc::make_mut(&mut self.mcp),
                &self.builtin,
                &env.lookup(),
            )
            .await;
        self.ext_view = Arc::new(self.ext.snapshot());
        applied?;
        Ok(self.listing())
    }

    /// How many tools of each kind are on offer, for the status page.
    pub fn tool_counts(&self) -> serde_json::Value {
        serde_json::json!({
            "builtin": self.builtin.len(),
            "http": self.http_tools.specs().len(),
            "mcp": self.mcp.tools().len(),
        })
    }

    /// A snapshot for the web UI. Contains no secrets and no agent text.
    pub fn status_json(
        &self,
        satellites: usize,
        speaker: &SpeakerView,
        stations: &[String],
        tools: &serde_json::Value,
    ) -> serde_json::Value {
        let minute = self.assistant.minute_of_day();
        let (quiet_set, quiet_active) = speaker
            .quiet
            .map_or((false, false), |q| (true, q.contains(minute)));
        let mut who = serde_json::json!({
            "connected": satellites > 0 || speaker.console,
            "available": speaker.console || (satellites > 0 && speaker.available),
        });
        if speaker.console {
            who["name"] = "console".into();
        }
        let scheduled: Vec<serde_json::Value> = self
            .assistant
            .scheduled()
            .into_iter()
            .map(|(kind, label, secs)| {
                serde_json::json!({ "kind": kind, "label": label, "seconds_left": secs })
            })
            .collect();
        let (day, history) = self.usage.request_days();
        let history: Vec<serde_json::Value> = history
            .iter()
            .map(|(d, n)| serde_json::json!({ "day": d, "requests": n }))
            .collect();
        serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "llm": {
                "configured": self.llm.is_some(),
                "model": self.llm.as_ref().map(|l| l.cfg.model.clone()),
            },
            "usage": {
                "day": day,
                "history": history,
                "daily_spend_cap_usd": self.usage.limits().daily_spend_cap_usd,
                "requests_today": self.usage.requests_today(),
                "spend_today": self.usage.spend_today(),
            },
            "hardware": crate::hardware::detected(),
            "clock_valid": self.clock.wall_clock_valid(),
            "satellites": satellites,
            "speaker": who,
            "voice": { "configured": self.voice_configured },
            "realtime": self.rt.status(),
            // The satellite does not yet say whether it is muted or busy; both read as "not available".
            "muted": false,
            "quiet_hours": { "set": quiet_set, "active": quiet_active },
            "tools": tools,
            "stations": stations,
            "scheduled": scheduled,
        })
    }

    /// Start a realtime voice session for a speaker (REQ-RTV-009) as a task beside this loop (ADR-0021). The
    /// tools run through the same gate, audit log and notes as any turn; the cap is checked before the
    /// connection opens and audio cost is added while it runs. The session lasts at most `max_session_secs`
    /// and nothing else waits for it.
    #[cfg(feature = "realtime")]
    pub fn start_realtime(&mut self, job: rtvoice::Job, core: &Core, hub: &mut Hub) {
        self.rt.set_error(None);
        core.patch_ui_status("realtime", self.rt.status());
        // A fresh conversation per session: it holds the transcript and any held action in memory only.
        let mut conv = Conversation::standard(self.persona.clone());
        if let Some(a) = core.audit() {
            conv.set_audit(a);
        }
        if let Some(n) = core.notes() {
            conv.set_notes(n);
        }
        let kit = turns::RtKit {
            settings: self.rt.settings(),
            key: self.rt.key(),
            persona: self.persona.clone(),
            cfg: self.llm.as_ref().map(|l| l.cfg.clone()).unwrap_or_default(),
            usage: self.usage.clone(),
            http: Arc::clone(&self.http_tools),
            mcp: Arc::clone(&self.mcp),
            ext: Arc::clone(&self.ext_view),
            builtin: ToolHost::tool_specs(&self.assistant),
            day: self.assistant.local_day(),
            clock: Arc::clone(&self.clock),
            conv,
            rt: self.rt.clone(),
        };
        hub.spawn_realtime(kit, job);
    }

    /// Without the `realtime` feature no session can be configured; a stray job is answered at once.
    #[cfg(not(feature = "realtime"))]
    pub fn start_realtime(&mut self, job: rtvoice::Job, _core: &Core, _hub: &mut Hub) {
        let _ = job.done.send(rtvoice::Done {
            speak: Some("This build has no realtime voice.".into()),
            failed: true,
            effects: Vec::new(),
        });
    }

    /// Answer locally if the fast path can (timers, alarms, music). `None` means the model is needed.
    pub fn fast(&mut self, line: &str) -> Option<Reply> {
        self.assistant.handle(line).map(|out| Reply {
            say: out.say,
            effects: out.effects,
            expects_reply: false,
        })
    }

    /// What a model turn starts from, copied or shared now: the prompt, the tools, the policy and the limits
    /// (ADR-0021), for the language model `l`. A change made in the portal while the turn runs reaches the next
    /// turn. Building it is not free, so a caller builds it only when a turn will start.
    pub fn kit(&mut self, l: &Llm) -> TurnKit<HttpBackend> {
        let host = WithPolicy {
            inner: &mut WithMcp {
                inner: &mut WithHttp {
                    inner: &mut self.assistant,
                    http: &self.http_tools,
                },
                mcp: &self.mcp,
            },
            ext: &self.ext,
            mcp: &self.mcp,
            scope: None,
        };
        // The enabled skills ride along in the system prompt, after the persona (REQ-CFG-036).
        let available: Vec<String> = host.tool_specs().into_iter().map(|t| t.name).collect();
        let system = format!("{}{}", self.persona, self.ext.skills_prompt(&available));
        TurnKit {
            backend: Arc::clone(&l.backend),
            cfg: l.cfg.clone(),
            usage: self.usage.clone(),
            http: Arc::clone(&self.http_tools),
            mcp: Arc::clone(&self.mcp),
            ext: Arc::clone(&self.ext_view),
            builtin: ToolHost::tool_specs(&self.assistant),
            system,
            day: self.assistant.local_day(),
            now: self.clock.now(),
        }
    }

    /// Save the usage counters. The loop is their only writer to disk.
    ///
    /// The write goes to the background writer, so the loop does not wait for flash. Saves coalesce: the
    /// counters on disk lag by at most the write in flight.
    pub fn save_usage(&self) {
        self.usage
            .save_queued(&self.usage_store, self.writer.as_deref());
    }
}

pub const NO_LLM: &str = "(that needs a language model; start with --llm-url and --llm-model)";

pub const TOO_BUSY: &str = "I'm still working on your last requests. Ask again in a moment.";

/// Show a reply: to the terminal for the console, or back to the satellite that asked.
pub fn deliver(origin: Origin, reply: Reply) {
    if let Some(reply) = origin.deliver(reply) {
        if reply.say.is_empty() {
            println!("speaker> (done)");
        } else {
            println!("speaker> {}", reply.say);
        }
        for e in &reply.effects {
            println!("satellite> {e:?}");
        }
    }
}

/// Show a reply once any timer or alarm change it may announce is on disk. A crash right after "Timer set"
/// is spoken must not lose the timer, so the reply is held back for the write (a few milliseconds on a
/// laptop, tens to hundreds on slow flash). The wait happens in its own task, so the loop keeps running.
pub fn deliver_confirmed(brain: &Brain, origin: Origin, mut reply: Reply) {
    match brain.assistant.take_save() {
        None => deliver(origin, reply),
        Some(saved) => match saved.outcome_now() {
            Some(outcome) => {
                tell_if_unsaved(&mut reply, outcome);
                deliver(origin, reply);
            }
            None => {
                tokio::spawn(async move {
                    let outcome = saved.wait().await;
                    tell_if_unsaved(&mut reply, outcome);
                    deliver(origin, reply);
                });
            }
        },
    }
}

/// If the timer, alarm or reminder the reply is about could not be saved, say so in the reply itself: it is set
/// and will ring, but a restart would lose it, and the person should know before they rely on it.
fn tell_if_unsaved(reply: &mut Reply, outcome: Result<(), crate::app::persist::SaveError>) {
    if outcome.is_err() {
        reply.say = crate::assistant::say_not_saved(&reply.say);
    }
}

/// A reply held back for the write of the change it announces (see [`deliver_confirmed`]), for the caller that
/// returns the reply instead of delivering it.
async fn confirmed(brain: &Brain, mut reply: Reply) -> Reply {
    if let Some(saved) = brain.assistant.take_save() {
        tell_if_unsaved(&mut reply, saved.wait().await);
    }
    reply
}

/// Handle something a speaker said: the fast path answers at once; anything else becomes a turn beside the
/// loop (or waits for the same speaker's running turn). Returns the reply when there is one already.
pub fn dispatch(brain: &mut Brain, hub: &mut Hub, req: Request) -> Option<(Origin, Reply)> {
    if let Some(reply) = brain.fast(&req.line) {
        return Some((req.origin, reply));
    }
    // Only the handle on the model is copied here; the kit (tool specs, system prompt) is built by the hub
    // when a task starts, not for a request that is queued or refused.
    let Some(llm) = brain.llm.clone() else {
        return Some((
            req.origin,
            Reply {
                say: NO_LLM.into(),
                ..Reply::default()
            },
        ));
    };
    let now = brain.clock.now();
    match hub.submit(req, now, || brain.kit(&llm)) {
        Submitted::Started | Submitted::Queued => None,
        Submitted::Refused(req) => Some((
            req.origin,
            Reply {
                say: TOO_BUSY.into(),
                ..Reply::default()
            },
        )),
    }
}

/// A turn is over: put the speaker's conversation back, save the counters, start what was waiting for the
/// same speaker, and return the reply to deliver.
pub fn finish_turn(brain: &mut Brain, hub: &mut Hub, f: turns::Finished) -> (Origin, Reply) {
    brain.save_usage();
    if let Some(e) = &f.failed {
        crate::diag::error(&format!("language model error: {e}"));
    }
    let now = brain.clock.now();
    let (origin, reply, next) = hub.finish(f, now);
    if let Some(next) = next
        && let Some((o, r)) = dispatch(brain, hub, next)
    {
        deliver_confirmed(brain, o, r);
    }
    (origin, reply)
}

/// Act on something a turn task reported.
pub async fn on_hub_event(brain: &mut Brain, hub: &mut Hub, core: &Core, ev: HubEvent) {
    match ev {
        HubEvent::Tool(req) => req.serve(&mut brain.assistant).await,
        HubEvent::Turn(f) => {
            let (origin, reply) = finish_turn(brain, hub, *f);
            deliver_confirmed(brain, origin, reply);
        }
        HubEvent::Other(_) => {
            // A realtime session ended: its audio cost is in the counters; show how it went.
            brain.save_usage();
            core.patch_ui_status("realtime", brain.rt.status());
        }
        HubEvent::Lost(speaker) => {
            // The satellite waiting for this answer sees its channel close and says it cannot answer.
            crate::diag::error("a turn ended without an answer");
            let now = brain.clock.now();
            if let Some(next) = hub.lost(&speaker, now)
                && let Some((origin, reply)) = dispatch(brain, hub, next)
            {
                deliver_confirmed(brain, origin, reply);
            }
        }
    }
}

/// Let the turns that are running finish and show their replies (the console was closed).
pub async fn drain(brain: &mut Brain, hub: &mut Hub, core: &Core) {
    while hub.active() > 0 {
        let ev = hub.next().await;
        on_hub_event(brain, hub, core, ev).await;
    }
    // Replies held back for a write are delivered by their own tasks; let them run before the caller leaves.
    if let Some(w) = core.writer() {
        let _ = w.flushed_since(0, &|_| true).await;
    }
    tokio::task::yield_now().await;
}

/// Answer one thing and wait for it, serving the turn's tool calls meanwhile. For one-shot use before the
/// main loop runs (a recorded utterance).
pub async fn respond_inline(brain: &mut Brain, hub: &mut Hub, line: &str) -> Reply {
    let req = Request {
        speaker: "console".into(),
        line: line.to_owned(),
        ctl: TurnCtl::default(),
        origin: Origin::Console,
    };
    if let Some((_, reply)) = dispatch(brain, hub, req) {
        return confirmed(brain, reply).await;
    }
    loop {
        match hub.next().await {
            HubEvent::Tool(req) => req.serve(&mut brain.assistant).await,
            HubEvent::Turn(f) => {
                let reply = finish_turn(brain, hub, *f).1;
                return confirmed(brain, reply).await;
            }
            HubEvent::Lost(_) => {
                return Reply {
                    say: "Sorry, I can't answer right now.".into(),
                    ..Reply::default()
                };
            }
            HubEvent::Other(_) => {}
        }
    }
}

/// Carry out one change the portal asked for and answer it. Everything here is quick and local except adding
/// an MCP connection: the loop checks the request and starts a task for the network half, and the answer
/// is sent when that task's result comes back through [`mcp_connected`] (ADR-0021).
pub async fn portal_op(
    brain: &mut Brain,
    core: &Core,
    applied: &mut serde_json::Map<String, serde_json::Value>,
    job: Job,
) {
    let Job { op, reply } = job;
    let answer = match op {
        Op::Ext(ExtOp::McpAdd(v)) => {
            match brain
                .ext
                .mcp_prepare(&v, &brain.mcp, brain.connects.portal_len(), |v| {
                    core.env().get(v)
                }) {
                Ok(pending) => {
                    brain.connects.start(pending, reply);
                    return;
                }
                Err(f) => Err(f),
            }
        }
        // Removing a connection that is still connecting stops it; that is the whole removal.
        Op::Ext(ExtOp::McpDelete(name)) if brain.connects.cancel(&name) => Ok(brain.listing()),
        // A saved source that is still connecting is removed like any other; its connection stops with it.
        Op::Ext(ExtOp::McpDelete(name)) => {
            let removed = sync_op(
                brain,
                core,
                applied,
                Op::Ext(ExtOp::McpDelete(name.clone())),
            )
            .await;
            if removed.is_ok() {
                brain.connects.forget_saved(&name);
            }
            removed
        }
        // A change to the timers is answered once it is on disk, and answered as a failure if it could not be
        // written: it is made and will ring, but the page must not say it is saved when it is not.
        op @ Op::Sched(_) => {
            let answer = sync_op(brain, core, applied, op).await;
            match brain.assistant.take_save() {
                Some(saved) if answer.is_ok() => {
                    tokio::spawn(async move {
                        let _ = reply.send(match saved.wait().await {
                            Ok(()) => answer,
                            Err(e) => Err(crate::app::Fail::NotSaved(
                                "The change was made, but it could not be saved, so it will be lost if the assistant restarts.".into(),
                                e.disk_full,
                            )),
                        });
                    });
                    return;
                }
                _ => answer,
            }
        }
        op => sync_op(brain, core, applied, op).await,
    };
    let _ = reply.send(answer);
}

/// A connection task reported. For a portal request: add the server if it is still wanted, and answer the
/// request. For a source saved earlier (connected at start-up): record how it went; it stays in the list either
/// way, and a failure is printed and shown as the source's error. A result whose connection was cancelled or
/// replaced meanwhile is dropped (see `connects`).
pub fn mcp_connected(brain: &mut Brain, core: &Core, done: Done) {
    let Some(source) = brain.connects.finish(&done) else {
        return;
    };
    let reply = match source {
        Source::Portal(reply) => reply,
        Source::Saved => {
            let mut reserved = brain.builtin.clone();
            reserved.extend(brain.http_tools.specs().into_iter().map(|t| t.name));
            let name = done.pending.name().to_owned();
            let listed = done.tools.as_ref().ok().map(Vec::len);
            let warnings =
                Arc::make_mut(&mut brain.mcp).settle(done.pending.name(), done.tools, &reserved);
            for w in warnings {
                eprintln!("clawdot: {w}");
            }
            if let Some(n) = listed {
                println!("clawdot: mcp server `{name}` connected ({n} tools listed)");
            }
            core.set_ui_tools(brain.listing());
            return;
        }
    };
    let answer = match done.tools {
        Err(why) => Err(crate::app::Fail::Invalid(why)),
        Ok(tools) => {
            let applied = brain.ext.mcp_commit(
                &done.pending,
                tools,
                &brain.http_tools,
                Arc::make_mut(&mut brain.mcp),
                &brain.builtin,
            );
            brain.ext_view = Arc::new(brain.ext.snapshot());
            applied.map(|()| {
                let listing = brain.listing();
                core.set_ui_tools(listing.clone());
                listing
            })
        }
    };
    let _ = reply.send(answer);
}

/// The changes that need no network: done on the spot.
async fn sync_op(
    brain: &mut Brain,
    core: &Core,
    applied: &mut serde_json::Map<String, serde_json::Value>,
    op: Op,
) -> Result<serde_json::Value, crate::app::Fail> {
    match op {
        Op::Sched(op) => brain.assistant.admin_sched(op),
        Op::Settings(live) => {
            apply_live(brain, core, &live, applied);
            Ok(serde_json::json!({ "applied": true }))
        }
        Op::Rekey => {
            // Make the next comparison see a change, so the client is built again with the new key.
            let live = applied.clone();
            applied.insert("llm_key_env".into(), serde_json::Value::Null);
            apply_live(brain, core, &live, applied);
            Ok(serde_json::json!({ "applied": true }))
        }
        Op::Ext(op) => {
            let r = brain.ext_apply(op, core.env()).await;
            if let Ok(listing) = &r {
                core.set_ui_tools(listing.clone());
            }
            r
        }
    }
}

impl std::fmt::Debug for Llm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Llm")
            .field("model", &self.cfg.model)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for Brain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Brain")
            .field("llm", &self.llm)
            .field("persona_chars", &self.persona.len())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Adapters, Fail, Limits, Stores};
    use crate::sched::SystemClock;
    use crate::session::Conversation;
    use crate::startup::build_brain;
    use crate::store::testutil::TempDir;
    use crate::{cli, seed};

    type Map = serde_json::Map<String, serde_json::Value>;

    /// A real brain over a scratch data directory, and the live values it started with.
    async fn started(dir: &TempDir, extra: &[&str]) -> (Brain, Arc<Core>, Map) {
        let mut argv = vec!["--data-dir".to_owned(), dir.0.display().to_string()];
        argv.extend(extra.iter().map(|s| (*s).to_owned()));
        let mut args = cli::parse(&argv).unwrap();
        let (state, _) = seed::resolve_settings(&mut args, &crate::env::Env::empty()).unwrap();
        let live = state.live_values();
        let clock: Arc<dyn Clock + Send + Sync> = Arc::new(SystemClock);
        let core = Arc::new(
            Core::open(
                clock.clone(),
                Adapters::builtin(),
                Limits::default(),
                Stores::in_dir(&args.data_dir),
            )
            .unwrap(),
        );
        let zone = zone_for(None).unwrap();
        let (brain, _hub) = build_brain(&mut args, &live, zone, &clock, &core)
            .await
            .unwrap();
        (brain, core, live)
    }

    fn with(live: &Map, changes: &[(&str, serde_json::Value)]) -> Map {
        let mut m = live.clone();
        for (k, v) in changes {
            m.insert((*k).to_owned(), v.clone());
        }
        m
    }

    // REQ-CFG-039: the agent group applies as a whole and reaches the language model's turn settings.
    #[tokio::test]
    async fn a_change_to_the_agent_group_reaches_the_brain_and_the_model_settings() {
        let dir = TempDir::new("apply-live-agent");
        let (mut brain, core, live) = started(
            &dir,
            &["--llm-url", "http://127.0.0.1:1/v1", "--llm-model", "m"],
        )
        .await;
        let mut applied = live.clone();
        assert_ne!(brain.llm.as_ref().unwrap().cfg.max_tool_loops, 3);

        let next = with(
            &live,
            &[
                ("agent_max_rounds", 3.into()),
                ("plan_speech", false.into()),
            ],
        );
        apply_live(&mut brain, &core, &next, &mut applied);

        assert_eq!(brain.agent.max_rounds, Some(3));
        assert_eq!(brain.agent.plan_speech, Some(false));
        let cfg = &brain.llm.as_ref().unwrap().cfg;
        assert_eq!(cfg.max_tool_loops, 3);
        assert!(!cfg.plan_speech);
        assert_eq!(applied, next, "what is in effect is remembered");
    }

    #[tokio::test]
    async fn only_what_changed_is_applied() {
        let dir = TempDir::new("apply-live-persona");
        let (mut brain, core, live) = started(&dir, &[]).await;
        let mut applied = live.clone();

        // Nothing changed: a value set behind the loop's back is left alone.
        brain.persona = "set elsewhere".into();
        apply_live(&mut brain, &core, &live, &mut applied);
        assert_eq!(brain.persona, "set elsewhere");

        let next = with(&live, &[("persona", "You are terse.".into())]);
        apply_live(&mut brain, &core, &next, &mut applied);
        assert_eq!(brain.persona, "You are terse.");
        assert!(brain.llm.is_none(), "no model was configured or added");
    }

    #[tokio::test]
    async fn the_daily_spend_cap_and_notes_switch_follow_the_settings() {
        let dir = TempDir::new("apply-live-spend");
        let (mut brain, core, live) = started(&dir, &[]).await;
        let mut applied = live.clone();
        assert_eq!(brain.usage.limits().daily_spend_cap_usd, None);

        let next = with(
            &live,
            &[("daily_spend_usd", 2.5.into()), ("notes", true.into())],
        );
        apply_live(&mut brain, &core, &next, &mut applied);
        assert_eq!(brain.usage.limits().daily_spend_cap_usd, Some(2.5));
        assert!(core.notes().unwrap().enabled());
    }

    fn ask(
        op: Op,
    ) -> (
        Job,
        tokio::sync::oneshot::Receiver<Result<serde_json::Value, Fail>>,
    ) {
        let (reply, answer) = tokio::sync::oneshot::channel();
        (Job { op, reply }, answer)
    }

    fn add_home(port: u16) -> Op {
        Op::Ext(ExtOp::McpAdd(serde_json::json!({
            "name": "home", "url": format!("http://127.0.0.1:{port}/mcp"),
        })))
    }

    // REQ-AST-027, REQ-CFG-033: while the portal's new MCP server is still connecting, the loop rings a timer
    // and applies a settings change; the server is added only when its result comes back. The server holds
    // its reply until the test opens it, so nothing here depends on a sleep being long enough.
    #[tokio::test(flavor = "current_thread")]
    async fn a_timer_rings_and_a_setting_applies_while_an_mcp_connect_is_in_flight() {
        let dir = TempDir::new("mcp-in-flight");
        let (mut brain, core, live) = started(&dir, &[]).await;
        let mut applied = live.clone();
        let mut done_rx = brain.connects.attach().unwrap();
        let (open_tx, open) = tokio::sync::watch::channel(false);
        let port = crate::connects::testutil::gated_server(open).await;
        brain
            .assistant
            .call_tool("set_timer", &serde_json::json!({"seconds": 1}))
            .await
            .unwrap();

        let (job, mut added) = ask(add_home(port));
        portal_op(&mut brain, &core, &mut applied, job).await;
        assert!(brain.connects.is_connecting("home"));
        assert!(
            added.try_recv().is_err(),
            "the request waits for the server"
        );

        // The settings change goes through and is answered at once.
        let next = with(&live, &[("persona", "You are terse.".into())]);
        let (job, settings) = ask(Op::Settings(next));
        portal_op(&mut brain, &core, &mut applied, job).await;
        assert!(settings.await.unwrap().is_ok());
        assert_eq!(brain.persona, "You are terse.");

        // The loop's tick rings the timer although the server has not answered.
        let mut tick = tokio::time::interval(Duration::from_millis(20));
        let rang = loop {
            tokio::select! {
                _ = tick.tick() => {
                    if !brain.assistant.tick().is_empty() { break true; }
                }
                Some(done) = done_rx.recv() => {
                    mcp_connected(&mut brain, &core, done);
                    break false;
                }
            }
        };
        assert!(rang, "the timer rang first");
        assert!(brain.connects.is_connecting("home"), "still connecting");
        assert!(!brain.mcp.has_server("home"), "nothing added yet");
        assert!(added.try_recv().is_err());

        // The server answers; the loop applies the result and answers the request.
        open_tx.send_replace(true);
        let done = done_rx.recv().await.unwrap();
        mcp_connected(&mut brain, &core, done);
        let listing = added.await.unwrap().unwrap();
        assert!(brain.mcp.has_server("home") && brain.mcp.owns("home__ping"));
        assert!(!brain.connects.is_connecting("home"));
        assert!(listing.to_string().contains("home__ping"), "{listing}");
        assert_eq!(core.ui_tools(), listing);
    }

    // Removing a source that is still connecting stops it: the server never appears, not even when the server
    // answers later, and the request that asked for it is told. The name can then be added again.
    #[tokio::test(flavor = "current_thread")]
    async fn removing_a_source_while_it_connects_cancels_it_and_it_can_be_added_again() {
        let dir = TempDir::new("mcp-cancel");
        let (mut brain, core, live) = started(&dir, &[]).await;
        let mut applied = live.clone();
        let mut done_rx = brain.connects.attach().unwrap();
        let (open_tx, open) = tokio::sync::watch::channel(false);
        let port = crate::connects::testutil::gated_server(open).await;

        let (job, first) = ask(add_home(port));
        portal_op(&mut brain, &core, &mut applied, job).await;
        let (job, twin) = ask(add_home(port));
        portal_op(&mut brain, &core, &mut applied, job).await;
        assert!(
            matches!(twin.await.unwrap(), Err(Fail::Invalid(m)) if m.contains("already")),
            "a second connect for the name is refused"
        );

        let (job, removed) = ask(Op::Ext(ExtOp::McpDelete("home".into())));
        portal_op(&mut brain, &core, &mut applied, job).await;
        assert!(removed.await.unwrap().is_ok());
        assert!(matches!(first.await.unwrap(), Err(Fail::Invalid(m)) if m.contains("removed")));
        assert!(!brain.connects.is_connecting("home"));

        // The server answers after the removal: nothing arrives, nothing is added.
        open_tx.send_replace(true);
        let late = tokio::time::timeout(Duration::from_millis(300), done_rx.recv()).await;
        assert!(late.is_err(), "the cancelled task sent nothing");
        assert!(!brain.mcp.has_server("home"));

        // And the name is free again.
        let (job, again) = ask(add_home(port));
        portal_op(&mut brain, &core, &mut applied, job).await;
        let done = done_rx.recv().await.unwrap();
        mcp_connected(&mut brain, &core, done);
        assert!(again.await.unwrap().is_ok());
        assert!(brain.mcp.has_server("home"));
    }

    /// Save two connections in `dir` the way the portal would: `stuck` (answers only when told to, gives up
    /// after 2 s) and `good`.
    fn save_sources(dir: &TempDir, stuck_port: u16, good_port: u16) {
        crate::store::JsonStore::new(dir.0.join("connections.json"), crate::extensions::SCHEMA)
            .save(&serde_json::json!({ "servers": [
                { "name": "stuck", "url": format!("http://127.0.0.1:{stuck_port}/mcp"), "timeout_secs": 2 },
                { "name": "good", "url": format!("http://127.0.0.1:{good_port}/mcp") },
            ]}))
            .unwrap();
    }

    fn source<'a>(listing: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
        listing["mcp"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["name"] == name)
            .unwrap_or_else(|| panic!("no source {name} in {listing}"))
    }

    // ADR-0021, REQ-AST-027: saved sources connect after the loop is running. One that never answers does not
    // hold up start-up: the loop rings a timer and answers a portal request while it is still connecting, the
    // good source's tools appear as soon as it answers, and the stalled one ends as failed after its own
    // timeout, still saved and listed with its reason.
    #[tokio::test(flavor = "current_thread")]
    async fn a_stalled_saved_source_does_not_hold_up_start_and_ends_failed_after_its_timeout() {
        let dir = TempDir::new("mcp-startup");
        let (_hold, closed) = tokio::sync::watch::channel(false);
        let (_open_tx, open) = tokio::sync::watch::channel(true);
        let stuck = crate::connects::testutil::gated_server(closed).await;
        let good = crate::connects::testutil::gated_server(open).await;
        save_sources(&dir, stuck, good);

        // Building the brain returns at once although one source will never answer.
        let began = std::time::Instant::now();
        let (mut brain, core, live) = started(&dir, &[]).await;
        assert!(
            began.elapsed() < Duration::from_millis(1500),
            "start-up did not wait for the sources"
        );
        let mut applied = live.clone();
        let mut done_rx = brain.connects.attach().unwrap();
        assert!(brain.mcp.is_connecting("stuck") && brain.mcp.is_connecting("good"));
        assert_eq!(brain.connects.len(), 2);
        assert_eq!(
            brain.connects.portal_len(),
            0,
            "saved sources are not portal requests"
        );
        let listing = brain.listing();
        assert_eq!(source(&listing, "stuck")["connecting"], true);
        assert_eq!(source(&listing, "stuck")["connected"], false);

        brain
            .assistant
            .call_tool("set_timer", &serde_json::json!({"seconds": 1}))
            .await
            .unwrap();

        // The loop: a portal request is answered at once, the good source is applied when it reports, and the
        // timer rings, all while the stalled source is still connecting.
        let next = with(&live, &[("persona", "You are terse.".into())]);
        let (job, settings) = ask(Op::Settings(next));
        portal_op(&mut brain, &core, &mut applied, job).await;
        assert!(settings.await.unwrap().is_ok());
        assert_eq!(brain.persona, "You are terse.");

        let mut tick = tokio::time::interval(Duration::from_millis(20));
        let (mut rang, mut good_in) = (false, false);
        while !(rang && good_in) {
            tokio::select! {
                _ = tick.tick() => {
                    rang |= !brain.assistant.tick().is_empty();
                }
                Some(done) = done_rx.recv() => {
                    assert_eq!(done.pending.name(), "good", "the stalled source cannot report first");
                    mcp_connected(&mut brain, &core, done);
                    good_in = true;
                }
            }
        }
        assert!(
            brain.mcp.owns("good__ping"),
            "the good source's tools appear"
        );
        let listing = brain.listing();
        assert_eq!(source(&listing, "good")["connected"], true);
        assert_eq!(source(&listing, "good")["connecting"], false);
        assert!(brain.mcp.is_connecting("stuck"), "still connecting");
        assert_eq!(source(&listing, "stuck")["connecting"], true);
        assert_eq!(core.ui_tools(), listing, "the portal's copy is refreshed");

        // Its own timeout ends the stalled one: failed, with the reason, and still saved.
        let done = done_rx.recv().await.unwrap();
        assert_eq!(done.pending.name(), "stuck");
        mcp_connected(&mut brain, &core, done);
        let listing = brain.listing();
        let s = source(&listing, "stuck");
        assert_eq!(s["connected"], false);
        assert_eq!(s["connecting"], false);
        assert!(s["error"].as_str().is_some_and(|e| !e.is_empty()), "{s}");
        assert!(!brain.mcp.owns("stuck__ping") && brain.connects.is_empty());
        assert_eq!(listing["mcp"].as_array().map(Vec::len), Some(2));
        assert_eq!(
            brain
                .ext
                .snapshot()
                .listing(&[], &brain.http_tools, &brain.mcp)["mcp"]
                .as_array()
                .map(Vec::len),
            Some(2),
            "both stay saved"
        );
    }

    // Removing a saved source that is still connecting removes it and stops its connection; its late result
    // is not applied.
    #[tokio::test(flavor = "current_thread")]
    async fn removing_a_saved_source_while_it_connects_stops_it() {
        let dir = TempDir::new("mcp-startup-remove");
        let (open_tx, closed) = tokio::sync::watch::channel(false);
        let (_g, open) = tokio::sync::watch::channel(true);
        let stuck = crate::connects::testutil::gated_server(closed).await;
        let good = crate::connects::testutil::gated_server(open).await;
        save_sources(&dir, stuck, good);
        let (mut brain, core, live) = started(&dir, &[]).await;
        let mut applied = live.clone();
        let mut done_rx = brain.connects.attach().unwrap();
        assert!(brain.connects.is_connecting("stuck"));

        let (job, removed) = ask(Op::Ext(ExtOp::McpDelete("stuck".into())));
        portal_op(&mut brain, &core, &mut applied, job).await;
        assert!(removed.await.unwrap().is_ok());
        assert!(!brain.connects.is_connecting("stuck") && !brain.mcp.has_server("stuck"));

        // Even if the server answers now, nothing arrives for the removed name.
        open_tx.send_replace(true);
        let done = done_rx.recv().await.unwrap();
        assert_eq!(done.pending.name(), "good");
        mcp_connected(&mut brain, &core, done);
        assert!(
            tokio::time::timeout(Duration::from_millis(300), done_rx.recv())
                .await
                .is_err()
        );
        assert!(!brain.mcp.has_server("stuck") && brain.mcp.owns("good__ping"));
    }

    // A connection task that dies is answered like any other failed connection, and the name can be tried
    // again.
    #[tokio::test(flavor = "current_thread")]
    async fn a_connect_task_that_dies_answers_the_request_and_frees_the_name() {
        let dir = TempDir::new("mcp-died");
        let (mut brain, core, live) = started(&dir, &[]).await;
        let mut applied = live.clone();
        let mut done_rx = brain.connects.attach().unwrap();
        brain.connects.make_next_task_die();
        let (job, answer) = ask(add_home(9));
        portal_op(&mut brain, &core, &mut applied, job).await;
        let done = done_rx.recv().await.unwrap();
        mcp_connected(&mut brain, &core, done);
        assert!(matches!(
            answer.await.unwrap(),
            Err(Fail::Invalid(m)) if m == "The connection could not be made."
        ));
        assert!(!brain.mcp.has_server("home") && brain.connects.is_empty());
        let (job, _again) = ask(add_home(9));
        portal_op(&mut brain, &core, &mut applied, job).await;
        assert!(brain.connects.is_connecting("home"));
    }

    // A server that cannot be used is reported to the request and leaves nothing behind.
    #[tokio::test(flavor = "current_thread")]
    async fn a_connect_that_fails_answers_the_request_and_adds_nothing() {
        let dir = TempDir::new("mcp-fail");
        let (mut brain, core, live) = started(&dir, &[]).await;
        let mut applied = live.clone();
        let mut done_rx = brain.connects.attach().unwrap();
        let (job, answer) = ask(add_home(9));
        portal_op(&mut brain, &core, &mut applied, job).await;
        let done = done_rx.recv().await.unwrap();
        mcp_connected(&mut brain, &core, done);
        assert!(matches!(answer.await.unwrap(), Err(Fail::Invalid(_))));
        assert!(!brain.mcp.has_server("home") && brain.connects.is_empty());
        // A bad request is refused on the spot, without a task.
        let (job, bad) = ask(Op::Ext(ExtOp::McpAdd(
            serde_json::json!({"name": "Bad Name", "url": "http://h/mcp"}),
        )));
        portal_op(&mut brain, &core, &mut applied, job).await;
        assert!(matches!(bad.await.unwrap(), Err(Fail::Invalid(_))));
        assert!(brain.connects.is_empty());
    }

    #[tokio::test]
    async fn the_status_snapshot_reports_the_brain_without_secrets() {
        let dir = TempDir::new("status");
        let (brain, _core, _live) = started(&dir, &[]).await;
        let view = SpeakerView {
            available: false,
            console: true,
            quiet: None,
        };
        let s = brain.status_json(0, &view, &["Jazz".to_owned()], &serde_json::json!({}));
        assert_eq!(s["llm"]["configured"], false);
        assert_eq!(s["speaker"]["name"], "console");
        assert_eq!(s["speaker"]["available"], true);
        assert_eq!(s["stations"][0], "Jazz");
        assert_eq!(s["satellites"], 0);
        assert_eq!(s["version"], env!("CARGO_PKG_VERSION"));
    }

    #[tokio::test]
    async fn the_fast_path_answers_without_a_model_and_other_requests_say_one_is_needed() {
        let dir = TempDir::new("dispatch");
        let (mut brain, _core, _live) = started(&dir, &[]).await;
        let mut hub = Hub::new(|| Conversation::standard(String::new()));
        let ask = |line: &str| Request {
            speaker: "console".into(),
            line: line.to_owned(),
            ctl: TurnCtl::default(),
            origin: Origin::Console,
        };
        let (_, reply) = dispatch(&mut brain, &mut hub, ask("set a timer for 5 minutes")).unwrap();
        assert!(
            !reply.say.is_empty() && reply.say != NO_LLM,
            "{}",
            reply.say
        );
        let (_, reply) = dispatch(
            &mut brain,
            &mut hub,
            ask("tell me about owls and their habits"),
        )
        .unwrap();
        assert_eq!(reply.say, NO_LLM);
    }

    fn fast_request(line: &str, origin: Origin) -> Request {
        Request {
            speaker: "console".into(),
            line: line.to_owned(),
            ctl: TurnCtl::default(),
            origin,
        }
    }

    fn timers_on_disk(dir: &TempDir) -> usize {
        let store = JsonStore::new(dir.0.join("timers.json"), crate::sched::SCHEMA);
        crate::sched::Scheduler::load(&store, 64)
            .unwrap()
            .list()
            .len()
    }

    // REQ-TMR-003: a timer the user was told is set survives a crash right after the confirmation. The reply is
    // held back until the write lands, and the loop is not held with it.
    #[tokio::test(flavor = "current_thread")]
    async fn the_confirmation_of_a_timer_is_not_delivered_before_it_is_on_disk() {
        let dir = TempDir::new("timer-durable");
        let (mut brain, core, _live) = started(&dir, &[]).await;
        let mut hub = Hub::new(|| Conversation::standard(String::new()));
        let writer = core.writer().unwrap();
        writer.pause();
        let (tx, mut rx) = tokio::sync::oneshot::channel();
        let (origin, reply) = dispatch(
            &mut brain,
            &mut hub,
            fast_request("set a timer for 5 minutes", Origin::Ask(tx)),
        )
        .unwrap();
        assert!(reply.say.starts_with("Timer set"), "{}", reply.say);
        deliver_confirmed(&brain, origin, reply);
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert!(rx.try_recv().is_err(), "spoken before it was saved");
        assert_eq!(timers_on_disk(&dir), 0);
        writer.resume();
        let said = rx.await.unwrap();
        assert!(said.say.starts_with("Timer set"));
        // The confirmation is out and nothing has been flushed by hand: it is already on disk.
        assert_eq!(timers_on_disk(&dir), 1);
    }

    // REQ-TMR-003: a timer whose save failed is still set (it will ring), and the confirmation says so in one short
    // sentence instead of claiming it is safe. The next change that does save is confirmed plainly again, and a reply
    // that changed nothing never repeats the bad news.
    #[tokio::test(flavor = "current_thread")]
    async fn a_timer_that_could_not_be_saved_is_confirmed_honestly() {
        let dir = TempDir::new("timer-unsaved");
        let (mut brain, _core, _live) = started(&dir, &[]).await;
        let mut hub = Hub::new(|| Conversation::standard(String::new()));
        // A directory where the temporary file would be written makes every save fail.
        let blocker = crate::store::temp_path(&dir.0.join("timers.json"));
        std::fs::create_dir_all(&blocker).unwrap();
        async fn said(brain: &mut Brain, hub: &mut Hub, line: &str) -> String {
            let (tx, rx) = tokio::sync::oneshot::channel();
            let (origin, reply) =
                dispatch(brain, hub, fast_request(line, Origin::Ask(tx))).unwrap();
            deliver_confirmed(brain, origin, reply);
            rx.await.unwrap().say
        }
        let first = said(&mut brain, &mut hub, "set a timer for 10 minutes").await;
        assert_eq!(
            first,
            "Timer set for 10 minutes, but I could not save it, so it will be lost if I restart.",
            "{first}"
        );
        let unrelated = said(&mut brain, &mut hub, "what time is it").await;
        assert!(!unrelated.contains("save"), "{unrelated}");
        assert_eq!(brain.assistant.scheduled().len(), 1, "it is still set");
        std::fs::remove_dir(&blocker).unwrap();
        let later = said(&mut brain, &mut hub, "set a timer for 5 minutes").await;
        assert!(
            later.starts_with("Timer set") && !later.contains("save"),
            "{later}"
        );
        assert_eq!(timers_on_disk(&dir), 2, "the later save carried both");
    }

    // The page is not told a timer it added is saved when the save failed: the answer is an error (500), though the
    // timer is set. Once saving works again the same request succeeds.
    #[tokio::test(flavor = "current_thread")]
    async fn a_portal_timer_that_could_not_be_saved_is_an_error_to_the_page() {
        let dir = TempDir::new("portal-unsaved");
        let (mut brain, core, live) = started(&dir, &[]).await;
        let mut applied = live.clone();
        let blocker = crate::store::temp_path(&dir.0.join("timers.json"));
        std::fs::create_dir_all(&blocker).unwrap();
        let body = serde_json::json!({"kind": "timer", "seconds": 60, "label": "tea"});
        let (job, answer) = ask(Op::Sched(crate::app::SchedOp::Add(body.clone())));
        portal_op(&mut brain, &core, &mut applied, job).await;
        assert!(
            matches!(answer.await.unwrap(), Err(Fail::NotSaved(_, false))),
            "the page was told it was saved"
        );
        assert_eq!(
            brain.assistant.scheduled().len(),
            1,
            "it is set all the same"
        );
        std::fs::remove_dir(&blocker).unwrap();
        let (job, answer) = ask(Op::Sched(crate::app::SchedOp::Add(body)));
        portal_op(&mut brain, &core, &mut applied, job).await;
        assert!(answer.await.unwrap().is_ok());
        assert_eq!(timers_on_disk(&dir), 2);
    }

    // A reply that announces nothing new is not held back by a write that is still queued.
    #[tokio::test(flavor = "current_thread")]
    async fn a_reply_with_no_pending_timer_change_is_delivered_at_once() {
        let dir = TempDir::new("timer-no-wait");
        let (mut brain, core, _live) = started(&dir, &[]).await;
        let mut hub = Hub::new(|| Conversation::standard(String::new()));
        let writer = core.writer().unwrap();
        writer.pause();
        let (tx, mut rx) = tokio::sync::oneshot::channel();
        let (origin, reply) = dispatch(
            &mut brain,
            &mut hub,
            fast_request("what time is it", Origin::Ask(tx)),
        )
        .unwrap();
        deliver_confirmed(&brain, origin, reply);
        assert!(rx.try_recv().is_ok());
        writer.resume();
    }

    // Rapid changes coalesce, and the file never goes back to an older state.
    #[tokio::test(flavor = "current_thread")]
    async fn rapid_timer_changes_leave_the_newest_state_on_disk() {
        let dir = TempDir::new("timer-rapid");
        let (mut brain, core, _live) = started(&dir, &[]).await;
        let mut hub = Hub::new(|| Conversation::standard(String::new()));
        for n in 1..=5 {
            let line = format!("set a timer for {n} minutes");
            dispatch(&mut brain, &mut hub, fast_request(&line, Origin::Console)).unwrap();
        }
        dispatch(
            &mut brain,
            &mut hub,
            fast_request("cancel the timer", Origin::Console),
        )
        .unwrap();
        for n in 6..=7 {
            let line = format!("set a timer for {n} minutes");
            dispatch(&mut brain, &mut hub, fast_request(&line, Origin::Console)).unwrap();
        }
        let expected = brain.assistant.scheduled().len();
        core.flush_blocking();
        assert_eq!(timers_on_disk(&dir), expected);
        assert_eq!(
            expected, 2,
            "the cancel emptied the list before the last two"
        );
    }

    // The counters are handed to the writer, not written by the loop, and a burst of saves leaves the newest.
    #[tokio::test(flavor = "current_thread")]
    async fn usage_saves_go_to_the_writer_and_the_last_one_wins() {
        let dir = TempDir::new("usage-queued");
        let (brain, core, _live) = started(&dir, &[]).await;
        let writer = core.writer().unwrap();
        let path = dir.0.join("usage.json");
        writer.pause();
        for _ in 0..5 {
            brain.usage.record(7, None);
            brain.save_usage();
        }
        assert!(!path.exists(), "the loop must not write the file itself");
        writer.resume();
        core.flush_blocking();
        let back = UsageTracker::load(&brain.usage_store, brain.usage.limits()).unwrap();
        assert_eq!(back.requests_today(), 5);
    }
}
