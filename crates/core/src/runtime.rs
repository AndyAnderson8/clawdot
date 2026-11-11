//! The state the main loop owns and what it does with it (ADR-0021): the [`Brain`] (assistant, language model,
//! tools and usage counters), live settings (`apply_live`), the status snapshot for the web UI, and turn
//! dispatch (`dispatch`, `finish_turn`, `on_hub_event`). The binary's `main` wires these to its channels.

use crate::app::{Core, Op};
use crate::assistant::Assistant;
use crate::cli::Args;
use crate::config::AgentConfig;
use crate::extensions::{ExtOp, Extensions, WithPolicy};
use crate::httptools::{HttpTools, WithHttp};
use crate::mcp::{McpClient, WithMcp};
use crate::rtvoice::{self, Realtime, RtSettings};
use crate::sched::{Clock, FixedOffset, Zone};
use crate::seed::{agent_from, policy_from, quiet_from};
#[cfg(feature = "realtime")]
use crate::session::Conversation;
use crate::session::{ToolHost, TurnConfig, TurnCtl};
use crate::settings::{self, Group};
use crate::speechio::apply_speech;
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
    pub backend: Arc<HttpBackend>,
    pub cfg: TurnConfig,
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
    key_env: &str,
    max_tokens: u32,
    agent: &AgentConfig,
) -> Result<Option<Llm>, String> {
    let (Some(url), Some(model)) = (url, model) else {
        return Ok(None);
    };
    // A key pasted into the portal wins; otherwise the environment variable that was named.
    let key = crate::secrets::get(crate::secrets::LLM_KEY)
        .unwrap_or_else(|| std::env::var(key_env).unwrap_or_default());
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
        apply_speech(live);
    }
    if settings::group_changed(Group::Realtime, live, applied) {
        brain.rt.set_settings(RtSettings::from_live(live));
    }
    #[cfg(feature = "tls")]
    if changed("update_check") {
        crate::update::set_enabled(
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
    pub assistant: Assistant,
    pub llm: Option<Llm>,
    /// The daily counters. Turns hold handles on the same ones; only this loop saves them.
    pub usage: UsageTracker,
    pub usage_store: JsonStore,
    /// The HTTP and MCP clients. A turn takes a clone of the `Arc` when it starts and calls the tools itself.
    pub http_tools: Arc<HttpTools>,
    pub mcp: Arc<McpClient>,
    pub clock: Arc<dyn Clock + Send + Sync>,
    /// Tools, connections, and skills managed from the portal.
    pub ext: Extensions,
    /// A read-only copy of `ext` for the turns, replaced whenever `ext` changes.
    pub ext_view: Arc<Extensions>,
    /// Names of the built-in tools.
    pub builtin: Vec<String>,
    /// The system prompt without skills.
    pub persona: String,
    /// Turn limits, speech while working and notes.
    pub agent: AgentConfig,
    /// A speech provider (address and speech model) is configured, so a speaker has a voice (REQ-CFG-070).
    pub voice_configured: bool,
    /// The optional realtime voice: its settings, status and the way sessions reach this loop.
    pub rt: Realtime,
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
    pub async fn ext_apply(&mut self, op: ExtOp) -> Result<serde_json::Value, crate::app::Fail> {
        // A turn that is running holds its own copies of the clients; `make_mut` gives this loop a private
        // one to change, and the turn keeps what it began with.
        let applied = self
            .ext
            .apply(
                op,
                Arc::make_mut(&mut self.http_tools),
                Arc::make_mut(&mut self.mcp),
                &self.builtin,
                |v| std::env::var(v).ok(),
            )
            .await;
        self.ext_view = Arc::new(self.ext.snapshot());
        applied?;
        Ok(self.listing())
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
            .map(|(kind, label, secs)| serde_json::json!({ "kind": kind, "label": label, "seconds_left": secs }))
            .collect();
        serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "llm": {
                "configured": self.llm.is_some(),
                "model": self.llm.as_ref().map(|l| l.cfg.model.clone()),
            },
            "usage": {
                "day": self.usage.request_days().0,
                "history": self.usage.request_days().1.iter().map(|(d, n)| serde_json::json!({ "day": d, "requests": n })).collect::<Vec<_>>(),
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
        let mut conv = Conversation::new(self.persona.clone(), 20, 24_000, 600);
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
    /// (ADR-0021). A change made in the portal while the turn runs reaches the next turn. `None` without a
    /// language model.
    pub fn kit(&mut self) -> Option<TurnKit<HttpBackend>> {
        let l = self.llm.as_ref()?;
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
        Some(TurnKit {
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
        })
    }

    /// Save the usage counters. The loop is their only writer to disk.
    pub fn save_usage(&self) {
        if let Err(e) = self.usage.save(&self.usage_store) {
            crate::diag::error(&format!("could not save usage counters: {e}"));
        }
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

/// Handle something a speaker said: the fast path answers at once; anything else becomes a turn beside the
/// loop (or waits for the same speaker's running turn). Returns the reply when there is one already.
pub fn dispatch(brain: &mut Brain, hub: &mut Hub, req: Request) -> Option<(Origin, Reply)> {
    if let Some(reply) = brain.fast(&req.line) {
        return Some((req.origin, reply));
    }
    let Some(kit) = brain.kit() else {
        return Some((
            req.origin,
            Reply {
                say: NO_LLM.into(),
                ..Reply::default()
            },
        ));
    };
    let now = brain.clock.now();
    match hub.submit(req, now, || kit) {
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
        deliver(o, r);
    }
    (origin, reply)
}

/// Act on something a turn task reported.
pub async fn on_hub_event(brain: &mut Brain, hub: &mut Hub, core: &Core, ev: HubEvent) {
    match ev {
        HubEvent::Tool(req) => req.serve(&mut brain.assistant).await,
        HubEvent::Turn(f) => {
            let (origin, reply) = finish_turn(brain, hub, *f);
            deliver(origin, reply);
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
                deliver(origin, reply);
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
        return reply;
    }
    loop {
        match hub.next().await {
            HubEvent::Tool(req) => req.serve(&mut brain.assistant).await,
            HubEvent::Turn(f) => return finish_turn(brain, hub, *f).1,
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

/// Carry out one change the portal asked for and return the answer to send back.
pub async fn portal_op(
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
            let r = brain.ext_apply(op).await;
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
    use crate::app::{Adapters, Limits, Stores};
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
        let (state, _) = seed::resolve_settings(&mut args).unwrap();
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
        let mut hub = Hub::new(|| Conversation::new(String::new(), 20, 24_000, 600));
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
}
