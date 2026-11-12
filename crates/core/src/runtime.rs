//! What the loop owns about the assistant itself (ADR-0021): the [`Brain`] (assistant, language model, tools and
//! usage counters), how the language model client is built, live settings (`apply_live`) and the status snapshot
//! for the web UI. What the loop does with events is on [`crate::app_loop::App`].

use crate::app::Core;
use crate::assistant::Assistant;
use crate::cli::Args;
use crate::config::AgentConfig;
use crate::connects::{Connects, Done};
use crate::extensions::{ExtOp, Extensions, ToolSets, WithPolicy};
use crate::httptools::{HttpTools, WithHttp};
use crate::mcp::{McpClient, WithMcp};
use crate::rtvoice::{self, Realtime};
use crate::sched::{Clock, Fired, FixedOffset, Ts, Zone};
#[cfg(feature = "realtime")]
use crate::session::Conversation;
use crate::session::{ToolHost, TurnConfig};
use crate::store::JsonStore;
use crate::transport::HttpBackend;
use crate::turns::{Hub, Reply, TurnKit};
use crate::usage::UsageTracker;
use std::sync::Arc;
use std::time::Duration;

mod live;
pub use live::apply_live;

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

/// The prices of a million model tokens, in and out, from the live settings. A missing key is `None` (unset,
/// priced by the built-in price); 0 stays `Some(0.0)` (free).
pub fn model_prices(
    live: &serde_json::Map<String, serde_json::Value>,
) -> (Option<f64>, Option<f64>) {
    let price = |k: &str| live.get(k).and_then(serde_json::Value::as_f64);
    (price("llm_usd_per_mtok_in"), price("llm_usd_per_mtok_out"))
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
    /// What this device is: what the system shows, with the configured name and parts (the `device` section).
    pub(crate) hardware: crate::hardware::Hardware,
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

    /// Monotonic seconds, for measuring how long something has lasted (speaker idle time, windows).
    #[must_use]
    pub fn mono(&self) -> i64 {
        self.clock.monotonic_secs()
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
                ToolSets::new(
                    Arc::make_mut(&mut self.http_tools),
                    Arc::make_mut(&mut self.mcp),
                    &self.builtin,
                    &|n: &str| env.get(n),
                ),
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
        let limits = self.usage.limits();
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
                "daily_spend_cap_usd": limits.daily_spend_cap_usd,
                "requests_today": self.usage.requests_today(),
                "spend_today": self.usage.spend_today(),
                // What spend_today rests on: "reported" (the provider's own costs), "priced" (its token
                // counts times the person's prices), "estimated" (built-in prices and/or sizes), or "none"
                // while nothing today has cost anything.
                "spend_basis": self.usage.basis().map_or("none", crate::usage::Basis::as_str),
                "llm_usd_per_mtok_in": limits.llm_usd_per_mtok_in,
                "llm_usd_per_mtok_out": limits.llm_usd_per_mtok_out,
            },
            "hardware": &self.hardware,
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
            mono: self.clock.monotonic_secs(),
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
mod tests;
