//! `clawdot`: the core server with a text console standing in for a satellite.
//!
//! Until a real audio satellite exists (milestone M1), this runs the complete loop end to end: agents
//! post webhooks, the console "speaks" announcements, and what you type is handled exactly as a
//! transcribed utterance would be: the local fast path first, then the language model if one is
//! configured.

use clawdot_core::app::Op;
use clawdot_core::app::{Adapters, Core, Limits, Stores};
use clawdot_core::assistant::{Assistant, describe_fired};
use clawdot_core::audit::{AuditConfig, AuditLog};
use clawdot_core::cli::{self, Args};
use clawdot_core::config::{AgentConfig, SpeechConfig};
#[cfg(feature = "esphome-satellite")]
use clawdot_core::esphome_cfg::EsphomeConfig;
use clawdot_core::extensions::{ExtOp, Extensions, WithPolicy};
use clawdot_core::http::{ServeOptions, serve};
use clawdot_core::httptools::{HttpTools, WithHttp};
use clawdot_core::inbox::event::Event;
use clawdot_core::inbox::summary::{
    SummaryConfig, announcement, clamp_for_speech, fallback_summary, short_announcement,
    summary_prompt,
};
use clawdot_core::llm::{ChatRequest, Message};
use clawdot_core::mcp::{McpClient, WithMcp};
use clawdot_core::notes::NoteStore;
use clawdot_core::rtvoice::{self, Realtime, RtSettings};
use clawdot_core::satellite::{self, Ask};
use clawdot_core::sched::{Clock, FixedOffset, Scheduler, SystemClock, Zone};
use clawdot_core::seed;
use clawdot_core::seed::{agent_from, policy_from, quiet_from};
use clawdot_core::session::{ChatBackend, Conversation, ToolHost, TurnConfig, TurnCtl};
use clawdot_core::settings::{self, Group, public_url_from_listen};
use clawdot_core::speech::{HttpSynthesizer, HttpTranscriber, parse_wav, wav};
use clawdot_core::store::JsonStore;
use clawdot_core::transport::HttpBackend;
use clawdot_core::turns::{
    self, Event as HubEvent, Hub, Origin, Reply, Request, Submitted, TurnKit,
};
use clawdot_core::usage::{UsageLimits, UsageTracker};
use clawdot_core::voice::voice_turn;
use std::net::SocketAddr;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::mpsc;

/// `--reset-admin`: forget the administrator setting so first-run setup shows again (REQ-CFG-022).
fn reset_admin(dir: &std::path::Path) -> ExitCode {
    match clawdot_core::admin::reset(dir) {
        Ok(removed) => {
            println!(
                "{}Start clawdot and open the web UI to set an administrator password again.\n\
                 Until you do, the setup page can be completed by anyone on a private network who reaches it.",
                if removed {
                    "The web UI administrator setting was cleared. Endpoints and other state are unchanged.\n"
                } else {
                    "There was no administrator setting to clear.\n"
                }
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("clawdot: cannot clear the administrator setting: {e}");
            ExitCode::from(1)
        }
    }
}

/// Tell the person at the console where the web UI is and how it is protected. No secret is printed: the
/// device may have no visible console (REQ-CFG-018, REQ-CFG-021).
fn announce_admin(core: &Core, public: &str) {
    use clawdot_core::admin::Mode;
    match core.admin_mode() {
        Mode::Setup => {
            println!(
                "\nWeb UI: {public}/  (open it to finish first-run setup)\n\
                 Until setup is done, the first person on a private network to open it can set the password."
            );
        }
        Mode::Password => println!("\nWeb UI: {public}/"),
        Mode::Open => println!(
            "\nWeb UI: {public}/  is UNPROTECTED (open mode): anyone on your network can use it.\n\
             Set a password in the page, or run `clawdot --reset-admin` and start again."
        ),
    }
}

/// True if a console line asks to leave (REQ-CFG-017).
fn is_quit(line: &str) -> bool {
    let word = line.trim().trim_end_matches(['.', '!']);
    word.eq_ignore_ascii_case("quit") || word.eq_ignore_ascii_case("exit")
}

const DEFAULT_PORT: u16 = 3335;

/// An actionable message for a failed bind (REQ-CFG-014).
fn bind_error(addr: SocketAddr, e: &std::io::Error) -> String {
    let hint = if e.kind() == std::io::ErrorKind::AddrInUse {
        format!(
            "another program is using port {}; choose a free one, for example --listen {}",
            addr.port(),
            SocketAddr::new(addr.ip(), DEFAULT_PORT)
        )
    } else {
        "choose another address with --listen (or `listen` in the configuration file)".to_owned()
    };
    format!("cannot listen on {addr}: {e}. {hint}")
}

fn zone_for(name: Option<&str>) -> Result<Box<dyn Zone + Send + Sync>, String> {
    match name {
        None => Ok(Box::new(FixedOffset(0))),
        #[cfg(feature = "tz")]
        Some(n) => clawdot_core::sched::IanaZone::new(n)
            .map(|z| Box::new(z) as Box<dyn Zone + Send + Sync>),
        #[cfg(not(feature = "tz"))]
        Some(_) => Err("this build has no time-zone database (feature `tz`)".to_owned()),
    }
}

fn print_snippet(public: &str, endpoint: &str, adapter: &str, token: &str) {
    println!(
        "\nEndpoint `{endpoint}` created (adapter: {adapter}). The token is shown only now:\n  {token}\n"
    );
    if adapter == "claude-code" {
        println!(
            "Claude Code hooks: add to ~/.claude/settings.json (merge with what is already there):"
        );
        let hook = |event: &str| {
            format!(
                "    \"{event}\": [{{ \"hooks\": [{{ \"type\": \"http\", \"url\": \"{public}/hook/{endpoint}\", \"headers\": {{ \"Authorization\": \"Bearer {token}\" }}, \"timeout\": 5 }}] }}]"
            )
        };
        println!(
            "{{\n  \"hooks\": {{\n{},\n{}\n  }}\n}}\n",
            hook("Stop"),
            hook("Notification")
        );
        println!(
            "Voice replies reach the agent through the bridge, on the machine where the agent runs:"
        );
        println!("  CLAWDOT_TOKEN=<token> clawdot-bridge --url {public} --endpoint {endpoint}\n");
    } else {
        println!(
            "Post events with:\n  curl -X POST {public}/hook/{endpoint} -H 'Authorization: Bearer <token>' -d '{{\"title\":\"Backup finished\",\"summary\":\"12 GB copied\"}}'\n"
        );
    }
}

/// Facts shown next to the settings but not edited there: the speech provider's models and where speakers
/// connect. Names of environment variables are shown; their values never are.
fn settings_extra(args: &Args) -> serde_json::Value {
    serde_json::json!({
        "voice_info": {
            "configured": args.speech.url.is_some(),
            "language": args.speech.language,
            "provider": args
                .speech
                .url
                .as_deref()
                .and_then(|u| clawdot_core::transport::Endpoint::parse(u).ok())
                .map(|e| e.host),
            "stt_model": args.speech.stt_model,
            "tts_model": args.speech.tts_model,
            "dialect": args.speech.dialect().name(),
        },
        "speech_key_env": args.speech.key_env,
        "speakers_listen": args.satellite.listen,
    })
}

/// Something for the speaker task to say, handled in the order it was sent (ADR-0021).
enum Say {
    /// A phrase from the portal; a voice being tried is used for this phrase only.
    Phrase {
        text: String,
        preview: Option<String>,
    },
    /// A timer, alarm or reminder that rang.
    Line(String),
    /// Look for an agent event worth announcing now, and say it.
    Poll {
        minute: u16,
        day: i64,
        llm: Option<Llm>,
    },
}

/// What the speaker task needs.
struct Voice {
    announcer: satellite::Announcer,
    core: Arc<Core>,
    usage: UsageTracker,
    usage_store: JsonStore,
    /// Spoken text is echoed only to an interactive console: in server mode stdout goes to a journal or
    /// container log that is never size-capped (REQ-SEC-009, PRIVACY.md).
    echo: bool,
    /// Some satellite could speak right now, as of the last poll (read by the loop for the portal).
    available: AtomicBool,
    /// A poll is queued or running; the loop sends another only when this is false.
    polling: AtomicBool,
}

/// Say what the loop hands over, one thing at a time. A satellite that is busy or muted simply does not
/// speak a timer's line (best effort); an agent event stays queued until one can.
async fn speak(voice: Arc<Voice>, mut rx: mpsc::UnboundedReceiver<Say>) {
    let v = &*voice;
    while let Some(job) = rx.recv().await {
        match job {
            Say::Phrase { text, preview } => {
                if v.echo {
                    println!("speaker> {text}");
                }
                if v.announcer.connected() > 0 {
                    let before = preview.as_deref().and_then(swap_live_voice);
                    v.announcer.announce(&text).await;
                    if let Some(old) = before {
                        swap_live_voice(&old);
                    }
                }
            }
            Say::Line(line) => {
                if v.echo {
                    println!("speaker> {line}");
                }
                if v.announcer.connected() > 0 {
                    v.announcer.announce(&line).await;
                }
            }
            Say::Poll { minute, day, llm } => {
                let connected = v.announcer.connected() > 0;
                let free = connected && v.announcer.is_available().await;
                v.available.store(free, Ordering::Relaxed);
                // A satellite that is connected but busy or muted: keep the event queued and try again.
                if (!connected || free)
                    && let Some(next) = v.core.next_announcement(true, minute)
                {
                    let spoken = match &llm {
                        Some(l) => {
                            let mut usage = v.usage.clone();
                            let s = summarise(l, &mut usage, day, &next.event).await;
                            if let Err(e) = usage.save(&v.usage_store) {
                                clawdot_core::diag::error(&format!(
                                    "could not save usage counters: {e}"
                                ));
                            }
                            s
                        }
                        // No model: say only the headline and outcome, never raw agent text.
                        None => short_announcement(&next.event)
                            .unwrap_or_else(|| fallback_summary(&next.event)),
                    };
                    let line = announcement(&next.event, &spoken, next.more_waiting);
                    if v.echo {
                        println!("speaker> {line}");
                    }
                    let delivered = !connected || v.announcer.announce(&line).await;
                    if delivered {
                        v.core.mark_announced(&[next.event.id]);
                    }
                }
                v.polling.store(false, Ordering::Relaxed);
            }
        }
    }
}

/// The language model and everything needed to use it.
#[derive(Clone)]
struct Llm {
    backend: Arc<HttpBackend>,
    cfg: TurnConfig,
}

/// The turn settings: the defaults of REQ-AST-019 lowered or switched by the `agent` section.
fn agent_turn_config(
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
fn with_agent(cfg: TurnConfig, agent: &AgentConfig) -> TurnConfig {
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
fn build_llm(
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
    let key = clawdot_core::secrets::get(clawdot_core::secrets::LLM_KEY)
        .unwrap_or_else(|| std::env::var(key_env).unwrap_or_default());
    let backend = Arc::new(HttpBackend::new(url, &key, Duration::from_secs(30))?);
    Ok(Some(Llm {
        backend,
        cfg: agent_turn_config(model, None, None, max_tokens, agent),
    }))
}

/// Switch to the new values of the settings that apply without a restart (REQ-CFG-039). `applied` holds
/// what is already in effect, so only real changes do any work.
fn apply_live(
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
            Err(e) => clawdot_core::diag::error(&format!("cannot switch time zone: {e}")),
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
        clawdot_core::update::set_enabled(
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
            Err(e) => clawdot_core::diag::error(&format!("cannot switch language model: {e}")),
        }
    }
    *applied = live.clone();
}

/// A spoken summary: from the summary model if possible, else the headline and outcome only.
async fn summarise(llm: &Llm, usage: &mut UsageTracker, day: i64, event: &Event) -> String {
    let cfg = SummaryConfig::default();
    if let Some(spoken) = short_announcement(event) {
        return spoken;
    }
    if usage.check(day).is_ok() {
        let (system, user) = summary_prompt(event, &cfg);
        let req = ChatRequest {
            model: llm
                .cfg
                .summary_model
                .clone()
                .unwrap_or_else(|| llm.cfg.model.clone()),
            messages: vec![Message::system(system), Message::user(user)],
            tools: Vec::new(),
            max_tokens: 120,
        };
        let outcome = llm.backend.complete(&req).await;
        usage.record(day, outcome.as_ref().ok().and_then(|c| c.usage.as_ref()));
        if let Ok(c) = outcome {
            let spoken = clamp_for_speech(&c.text, &cfg);
            if !spoken.is_empty() {
                return spoken;
            }
        }
    }
    fallback_summary(event)
}

/// Everything needed to answer one thing the user said, whether it was typed or spoken.
struct Brain {
    assistant: Assistant,
    llm: Option<Llm>,
    /// The daily counters. Turns hold handles on the same ones; only this loop saves them.
    usage: UsageTracker,
    usage_store: JsonStore,
    /// The HTTP and MCP clients. A turn takes a clone of the `Arc` when it starts and calls the tools itself.
    http_tools: Arc<HttpTools>,
    mcp: Arc<McpClient>,
    clock: Arc<dyn Clock + Send + Sync>,
    /// Tools, connections, and skills managed from the portal.
    ext: Extensions,
    /// A read-only copy of `ext` for the turns, replaced whenever `ext` changes.
    ext_view: Arc<Extensions>,
    /// Names of the built-in tools.
    builtin: Vec<String>,
    /// The system prompt without skills.
    persona: String,
    /// Turn limits, speech while working and notes.
    agent: AgentConfig,
    /// A speech provider (address and speech model) is configured, so a speaker has a voice (REQ-CFG-070).
    voice_configured: bool,
    /// The optional realtime voice: its settings, status and the way sessions reach this loop.
    rt: Realtime,
}

/// What the portal is told about the speaker (REQ-CFG-070).
struct SpeakerView {
    /// Some satellite could speak right now (idle and unmuted).
    available: bool,
    /// Speech is printed to the terminal, which counts as a speaker and needs no voice.
    console: bool,
    quiet: Option<clawdot_core::inbox::endpoint::QuietHours>,
}

impl Brain {
    /// What the Tools, Connections, and Skills tabs show (REQ-CFG-029, 036). Holds no secrets.
    fn listing(&self) -> serde_json::Value {
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
    async fn ext_apply(&mut self, op: ExtOp) -> Result<serde_json::Value, clawdot_core::app::Fail> {
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
    fn status_json(
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
            "hardware": clawdot_core::hardware::detected(),
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
    fn start_realtime(&mut self, job: rtvoice::Job, core: &Core, hub: &mut Hub) {
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
    fn start_realtime(&mut self, job: rtvoice::Job, _core: &Core, _hub: &mut Hub) {
        let _ = job.done.send(rtvoice::Done {
            speak: Some("This build has no realtime voice.".into()),
            failed: true,
            effects: Vec::new(),
        });
    }

    /// Answer locally if the fast path can (timers, alarms, music). `None` means the model is needed.
    fn fast(&mut self, line: &str) -> Option<Reply> {
        self.assistant.handle(line).map(|out| Reply {
            say: out.say,
            effects: out.effects,
            expects_reply: false,
        })
    }

    /// What a model turn starts from, copied or shared now: the prompt, the tools, the policy and the limits
    /// (ADR-0021). A change made in the portal while the turn runs reaches the next turn. `None` without a
    /// language model.
    fn kit(&mut self) -> Option<TurnKit<HttpBackend>> {
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
    fn save_usage(&self) {
        if let Err(e) = self.usage.save(&self.usage_store) {
            clawdot_core::diag::error(&format!("could not save usage counters: {e}"));
        }
    }
}

const NO_LLM: &str = "(that needs a language model; start with --llm-url and --llm-model)";
const TOO_BUSY: &str = "I'm still working on your last requests. Ask again in a moment.";

/// Show a reply: to the terminal for the console, or back to the satellite that asked.
fn deliver(origin: Origin, reply: Reply) {
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
fn dispatch(brain: &mut Brain, hub: &mut Hub, req: Request) -> Option<(Origin, Reply)> {
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
fn finish_turn(brain: &mut Brain, hub: &mut Hub, f: turns::Finished) -> (Origin, Reply) {
    brain.save_usage();
    if let Some(e) = &f.failed {
        clawdot_core::diag::error(&format!("language model error: {e}"));
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
async fn on_hub_event(brain: &mut Brain, hub: &mut Hub, core: &Core, ev: HubEvent) {
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
            clawdot_core::diag::error("a turn ended without an answer");
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
async fn drain(brain: &mut Brain, hub: &mut Hub, core: &Core) {
    while hub.active() > 0 {
        let ev = hub.next().await;
        on_hub_event(brain, hub, core, ev).await;
    }
}

/// Answer one thing and wait for it, serving the turn's tool calls meanwhile. For one-shot use before the
/// main loop runs (a recorded utterance).
async fn respond_inline(brain: &mut Brain, hub: &mut Hub, line: &str) -> Reply {
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

/// Build the speech providers from the `speech` configuration section.
/// The speech clients that exist, so a change of voice or language in the portal reaches them while running.
#[derive(Default)]
struct SpeechHandles {
    voices: Vec<Arc<std::sync::Mutex<String>>>,
    languages: Vec<Arc<std::sync::Mutex<Option<String>>>>,
}

static SPEECH_HANDLES: std::sync::Mutex<SpeechHandles> = std::sync::Mutex::new(SpeechHandles {
    voices: Vec::new(),
    languages: Vec::new(),
});

/// Build the speech clients and remember how to change their voice and language later.
fn build_registered_speech(
    speech: &SpeechConfig,
) -> Result<(HttpTranscriber, HttpSynthesizer), String> {
    let (stt, tts) = build_speech(speech)?;
    let mut h = SPEECH_HANDLES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    h.voices.push(tts.voice_handle());
    h.languages.push(stt.language_handle());
    Ok((stt, tts))
}

/// Make every speech client use `voice` now, and return the voice they were using (none when there is no client).
fn swap_live_voice(voice: &str) -> Option<String> {
    let h = SPEECH_HANDLES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut old = None;
    for slot in &h.voices {
        let mut v = slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        old.get_or_insert_with(|| v.clone());
        *v = voice.to_owned();
    }
    old
}

/// Apply the voice and language that are now set to every speech client.
fn apply_speech(live: &serde_json::Map<String, serde_json::Value>) {
    let h = SPEECH_HANDLES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(v) = live
        .get("voice")
        .and_then(serde_json::Value::as_str)
        .filter(|v| !v.is_empty())
    {
        for slot in &h.voices {
            *slot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = v.to_owned();
        }
    }
    let language = live
        .get("speech_language")
        .and_then(serde_json::Value::as_str)
        .filter(|l| !l.is_empty())
        .map(str::to_owned);
    for slot in &h.languages {
        *slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = language.clone();
    }
}

fn build_speech(speech: &SpeechConfig) -> Result<(HttpTranscriber, HttpSynthesizer), String> {
    let missing =
        |k: &str| format!("speech.{k} is not configured (see docs/architecture/voice-pipeline.md)");
    let url = speech.url.as_deref().ok_or_else(|| missing("url"))?;
    let key = speech
        .key_env
        .as_deref()
        .map(|v| std::env::var(v).unwrap_or_default())
        .unwrap_or_default();
    let deadline = Duration::from_secs(30);
    let dialect = speech.dialect();
    let stt = HttpTranscriber::new(
        url,
        &key,
        speech
            .stt_model
            .as_deref()
            .ok_or_else(|| missing("stt_model"))?,
        speech.language.as_deref(),
        deadline,
    )?
    .with_dialect(dialect)
    .with_stored_key();
    let tts = HttpSynthesizer::new(
        url,
        &key,
        speech
            .tts_model
            .as_deref()
            .ok_or_else(|| missing("tts_model"))?,
        speech.voice.as_deref().ok_or_else(|| missing("voice"))?,
        speech.tts_sample_rate.unwrap_or(24_000),
        deadline,
    )?
    .with_dialect(dialect)?
    .with_stored_key();
    Ok((stt, tts))
}

/// Answer one recorded utterance: transcribe the WAV, respond, speak the reply into a WAV.
async fn voice_once(
    brain: &mut Brain,
    hub: &mut Hub,
    speech: &SpeechConfig,
    input: &std::path::Path,
    output: Option<&std::path::Path>,
) -> Result<(), String> {
    let (stt, tts) = build_speech(speech)?;
    let bytes =
        std::fs::read(input).map_err(|e| format!("cannot read {}: {e}", input.display()))?;
    let pcm = parse_wav(&bytes).map_err(|e| format!("{}: {e}", input.display()))?;

    let effects = std::cell::RefCell::new(Vec::new());
    let out = voice_turn(&stt, &tts, &pcm, |text| {
        let brain = &mut *brain;
        let hub = &mut *hub;
        let effects = &effects;
        async move {
            let r = respond_inline(brain, hub, &text).await;
            effects.borrow_mut().extend(r.effects);
            r.say
        }
    })
    .await;
    match &out.transcript {
        Some(t) => println!("heard> {t}"),
        None => println!("heard> (nothing understood)"),
    }
    if !out.reply.is_empty() {
        println!("speaker> {}", out.reply);
    }
    for e in effects.into_inner() {
        println!("satellite> {e:?}");
    }
    for p in &out.problems {
        eprintln!("clawdot: {p}");
    }
    if let Some(path) = output {
        let audio: Vec<i16> = out.audio.concat();
        std::fs::write(path, wav(&audio))
            .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        println!(
            "clawdot: wrote {} sample(s) of speech to {}",
            audio.len(),
            path.display()
        );
    }
    Ok(())
}

/// `clawdot say TEXT ...`: speak a message on a running ClawDot (REQ-INB-026).
async fn say_command(rest: &[String]) -> ExitCode {
    let say_args = match clawdot_core::say::parse(rest, |k| std::env::var(k).ok()) {
        Ok(a) => a,
        Err(m) if m.is_empty() => {
            print!("{}", clawdot_core::say::HELP);
            return ExitCode::SUCCESS;
        }
        Err(m) => {
            eprintln!("clawdot say: {m}");
            return ExitCode::from(2);
        }
    };
    let token = std::env::var(&say_args.token_env)
        .ok()
        .filter(|t| !t.is_empty());
    if token.is_none() && say_args.body.get("endpoint").is_none() {
        eprintln!(
            "clawdot say: set the environment variable {} to an endpoint token (or choose another with --token-env)",
            say_args.token_env
        );
        return ExitCode::from(2);
    }
    match clawdot_core::say::run(&say_args, token.as_deref()).await {
        Ok(line) => {
            println!("{line}");
            ExitCode::SUCCESS
        }
        Err(m) => {
            eprintln!("clawdot say: {m}");
            ExitCode::FAILURE
        }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    // Used by an update to check that a new program starts before it replaces the running one.
    if matches!(argv.first().map(String::as_str), Some("--version" | "-V")) {
        println!("clawdot {}", clawdot_core::diag::build_id());
        return ExitCode::SUCCESS;
    }
    if argv.first().map(String::as_str) == Some("say") {
        return say_command(&argv[1..]).await;
    }
    let mut args = match cli::parse(&argv) {
        Ok(a) => a,
        Err(m) if m.is_empty() => {
            print!("{}", cli::HELP);
            return ExitCode::SUCCESS;
        }
        Err(m) => {
            eprintln!("clawdot: {m}\n\n{}", cli::HELP);
            return ExitCode::from(2);
        }
    };
    if args.reset_admin {
        return reset_admin(&args.data_dir);
    }
    let (mut settings_state, warnings) = match seed::resolve_settings(&mut args) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("clawdot: {e}");
            return ExitCode::from(2);
        }
    };
    for w in warnings {
        eprintln!("clawdot: {w}");
    }
    let zone = match zone_for(args.zone.as_deref()) {
        Ok(z) => z,
        Err(e) => {
            eprintln!("clawdot: {e}");
            return ExitCode::from(2);
        }
    };

    let clock: Arc<dyn Clock + Send + Sync> = Arc::new(SystemClock);
    if !clock.wall_clock_valid() {
        eprintln!(
            "clawdot: warning: the system clock is not set (no network time yet). Timers, alarms, and              reminders will not fire, and the time cannot be told, until it is set (REQ-TMR-006)."
        );
    }
    clawdot_core::secrets::init(&args.data_dir);
    match clawdot_core::admin::migrate_legacy(&args.data_dir) {
        Ok(true) => println!(
            "clawdot: the old administrator token still works: sign in with it, then set a password in the web UI"
        ),
        Ok(false) => {}
        Err(e) => {
            eprintln!("clawdot: cannot convert the old administrator token: {e}");
            return ExitCode::from(1);
        }
    }
    let core = match Core::open(
        clock.clone(),
        Adapters::builtin(),
        Limits::default(),
        Stores::in_dir(&args.data_dir),
    ) {
        Ok(c) => Arc::new(c),
        Err(e) => {
            eprintln!("clawdot: cannot open {}: {e}", args.data_dir.display());
            return ExitCode::from(1);
        }
    };

    let public = args.public_url.clone().unwrap_or_default();
    let (derived, assumed) = public_url_from_listen(args.listen);
    if assumed && public == derived {
        eprintln!(
            "clawdot: {} is not reachable as written, so snippets use {derived}; set the public address to change that",
            args.listen.ip()
        );
    }
    let mut failed = false;
    for spec in &args.create {
        let (name, adapter) = spec.split_once(':').unwrap_or((spec, spec));
        let adapter = if matches!(adapter, "claude-code" | "generic") {
            adapter
        } else {
            "generic"
        };
        match core.create_endpoint(name, adapter) {
            Ok((_, token)) => print_snippet(&public, name, adapter, &token),
            Err(e) => {
                eprintln!("clawdot: endpoint `{name}`: {e}");
                failed = true;
            }
        }
    }
    for name in &args.rotate {
        match core.rotate_endpoint(name) {
            Ok(token) => println!("New token for `{name}` (shown once): {token}"),
            Err(e) => {
                eprintln!("clawdot: endpoint `{name}`: {e}");
                failed = true;
            }
        }
    }
    if args.manage {
        // A management command: the token is printed; do not start a server (REQ-CFG-015).
        return ExitCode::from(u8::from(failed));
    }

    let limits = UsageLimits {
        daily_spend_cap_usd: args.daily_spend,
        ..UsageLimits::default()
    };
    let usage_store = JsonStore::new(
        args.data_dir.join("usage.json"),
        clawdot_core::usage::SCHEMA,
    );
    let usage = match UsageTracker::load(&usage_store, limits) {
        Ok(u) => u,
        Err(e) => {
            eprintln!("clawdot: cannot load usage counters: {e}");
            return ExitCode::from(1);
        }
    };

    let llm = match build_llm(
        args.llm_url.as_deref(),
        args.llm_model.as_deref(),
        &args.key_env,
        limits.max_output_tokens_per_turn,
        &args.agent,
    ) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("clawdot: {e}");
            return ExitCode::from(2);
        }
    };
    let persona = args.persona.clone();
    // Every action is recorded in the data directory and held to the daily caps (REQ-SEC-014..016).
    let audit = match AuditLog::open(
        &args.data_dir,
        AuditConfig {
            policy: args.actions,
            ..AuditConfig::default()
        },
    ) {
        Ok(a) => Arc::new(a),
        Err(e) => {
            eprintln!(
                "clawdot: cannot open the audit log in {}: {e}",
                args.data_dir.display()
            );
            return ExitCode::from(1);
        }
    };
    let audit_for_conv = Arc::clone(&audit);
    core.set_audit(audit);
    core.set_quiet_hours(args.quiet);
    // Notes the user asks for (REQ-AST-023); off unless the `agent` section turns them on.
    let notes = match NoteStore::open(&args.data_dir, args.agent.notes.unwrap_or(false)) {
        Ok(n) => Arc::new(n),
        Err(e) => {
            eprintln!(
                "clawdot: cannot open the notes in {}: {e}",
                args.data_dir.display()
            );
            return ExitCode::from(1);
        }
    };
    let notes_for_conv = Arc::clone(&notes);
    core.set_notes(notes);
    if args.actions.dry_run {
        eprintln!("clawdot: dry-run mode: actions are described and recorded, not run");
    }

    let sched_store = JsonStore::new(
        args.data_dir.join("timers.json"),
        clawdot_core::sched::SCHEMA,
    );
    let sched = match Scheduler::load(&sched_store, 64) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("clawdot: cannot load timers: {e}");
            return ExitCode::from(1);
        }
    };
    let station_names: Vec<String> = args.stations.iter().map(|s| s.name.clone()).collect();
    let assistant = Assistant::new(
        clock.clone(),
        zone,
        sched,
        Some(sched_store),
        Some(Arc::clone(&core)),
    )
    .with_stations(std::mem::take(&mut args.stations));

    let builtin: Vec<String> = ToolHost::tool_specs(&assistant)
        .into_iter()
        .map(|t| t.name)
        .collect();
    // What the portal saved (tools, connections, skills) loads after what the configuration file defines.
    let mut ext = match Extensions::open(&args.data_dir) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("clawdot: cannot load the portal's saved tools and skills: {e}");
            return ExitCode::from(1);
        }
    };
    let mut tool_defs = std::mem::take(&mut args.tools);
    let mut server_defs = std::mem::take(&mut args.mcp);
    ext.set_config(
        tool_defs.iter().map(|t| t.name.clone()),
        server_defs.iter().map(|s| s.name.clone()),
    );
    let mut taken = builtin.clone();
    taken.extend(tool_defs.iter().map(|t| t.name.clone()));
    tool_defs.extend(ext.startup_tools(&taken, |v| std::env::var(v).ok()));
    server_defs.extend(ext.startup_servers(|v| std::env::var(v).ok()));
    let http_tools = match HttpTools::new(tool_defs, &builtin, |v| std::env::var(v).ok()) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("clawdot: {e}");
            return ExitCode::from(1);
        }
    };

    let mut reserved = builtin.clone();
    reserved.extend(http_tools.specs().into_iter().map(|t| t.name));
    let mcp = match McpClient::connect(server_defs, &reserved, |v| std::env::var(v).ok()).await {
        Ok((client, warnings)) => {
            for w in warnings {
                eprintln!("clawdot: {w}");
            }
            client
        }
        Err(e) => {
            eprintln!("clawdot: {e}");
            return ExitCode::from(1);
        }
    };

    // One conversation for each speaker, made on first use (REQ-AST-026): its own history, its own held
    // confirmation, the same audit log and notes.
    let mut hub = Hub::new({
        let persona = persona.clone();
        move || {
            let mut c = Conversation::new(persona.clone(), 20, 24_000, 600);
            c.set_audit(Arc::clone(&audit_for_conv));
            c.set_notes(Arc::clone(&notes_for_conv));
            c
        }
    });
    let ext_view = Arc::new(ext.snapshot());
    let mut brain = Brain {
        assistant,
        llm,
        usage,
        usage_store,
        http_tools: Arc::new(http_tools),
        mcp: Arc::new(mcp),
        clock: clock.clone(),
        ext,
        ext_view,
        builtin: builtin.clone(),
        persona,
        agent: args.agent.clone(),
        voice_configured: args.speech.url.is_some() && args.speech.tts_model.is_some(),
        rt: Realtime::new(RtSettings::from_live(&settings_state.live_values())),
    };
    if brain.rt.settings().enabled && !RtSettings::built() {
        brain.rt.set_error(Some(
            "Realtime voice is turned on, but this build of ClawDot does not include it.".into(),
        ));
    }
    let mut rt_rx = brain.rt.attach();

    core.set_ui_tools(brain.listing());
    let mut applied = settings_state.live_values();
    if let Some(url) = &args.speech.url {
        clawdot_core::speech::set_source(
            url,
            args.speech.key_env.as_deref(),
            args.speech.dialect(),
        );
    }
    #[cfg(feature = "tls")]
    match clawdot_core::update::UpdateConfig::new(
        args.updates.url.as_deref(),
        args.updates.public_key.as_deref(),
    ) {
        Ok(cfg) => {
            let on = applied
                .get("update_check")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true);
            if let Err(e) = clawdot_core::update::start(cfg, on, Some(args.data_dir.clone())) {
                clawdot_core::diag::error(&format!("updates are not available: {e}"));
            }
        }
        Err(e) => clawdot_core::diag::error(&format!("updates are not available: {e}")),
    }
    settings_state.set_extra(settings_extra(&args));
    core.set_settings(settings_state);
    if let Some(input) = args.voice_in.clone() {
        return match voice_once(
            &mut brain,
            &mut hub,
            &args.speech,
            &input,
            args.voice_out.as_deref(),
        )
        .await
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("clawdot: {e}");
                ExitCode::from(1)
            }
        };
    }

    // Bind before anything is created or printed, so a server that cannot start never reveals a token
    // (REQ-CFG-015).
    let listener = match TcpListener::bind(args.listen).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("clawdot: {}", bind_error(args.listen, &e));
            return ExitCode::from(1);
        }
    };

    // Satellites (speakers) connect here; their questions come back to this loop through `ask_rx`.
    let (ask_tx, mut ask_rx) = mpsc::channel::<Ask>(8);
    let announcer = satellite::Announcer::default();
    if let Some(listen) = &args.satellite.listen {
        let started = async {
            let addr: SocketAddr = listen
                .parse()
                .map_err(|e| format!("satellite.listen: {e}"))?;
            let (stt, tts) = build_registered_speech(&args.speech)?;
            let mut settings = satellite::Settings {
                announcer: announcer.clone(),
                realtime: Some(brain.rt.clone()),
                ..satellite::Settings::default()
            };
            if let Some(var) = &args.satellite.token_env {
                let token = std::env::var(var).unwrap_or_default();
                if token.is_empty() {
                    return Err(format!(
                        "environment variable {var} (satellite.token_env) is not set"
                    ));
                }
                settings.token_hashes = vec![clawdot_core::inbox::endpoint::hash_token(&token)];
            }
            if let Some(s) = args.satellite.follow_up_secs {
                settings.dialog.follow_up_secs = s;
            }
            settings.admin_reset = Some(satellite::AdminResetHook::new({
                let core = Arc::clone(&core);
                move || {
                    use clawdot_core::admin::AdminError;
                    core.admin_open_reset_window()
                        .map(|s| u32::try_from(s).unwrap_or(0))
                        .map_err(|e| match e {
                            AdminError::NoPassword => {
                                "No password is set, so there is nothing to reset.".to_owned()
                            }
                            AdminError::RateLimited => "Wait a minute and try again.".to_owned(),
                            _ => "The reset window could not be opened.".to_owned(),
                        })
                }
            }));
            let l = TcpListener::bind(addr)
                .await
                .map_err(|e| format!("cannot listen on {addr}: {e}"))?;
            println!("clawdot: satellites connect to {addr}");
            let tx = ask_tx.clone();
            tokio::spawn(async move {
                if let Err(e) = satellite::serve(
                    l,
                    Arc::new(stt),
                    Arc::new(tts),
                    tx,
                    settings,
                    std::future::pending(),
                )
                .await
                {
                    eprintln!("clawdot: satellite server stopped: {e}");
                }
            });
            Ok::<(), String>(())
        };
        if let Err(e) = started.await {
            eprintln!("clawdot: {e}");
            return ExitCode::from(1);
        }
    }

    // ESPHome voice satellites (optional feature; spec 0004) share the satellite pipeline and announcer.
    #[cfg(feature = "esphome-satellite")]
    if !args.esphome.is_empty() {
        let started = async {
            let (stt, tts) = build_registered_speech(&args.speech)?;
            let mut settings = satellite::Settings {
                announcer: announcer.clone(),
                realtime: Some(brain.rt.clone()),
                ..satellite::Settings::default()
            };
            if let Some(s) = args.satellite.follow_up_secs {
                settings.dialog.follow_up_secs = s;
            }
            clawdot_core::esphome::start(
                &args.esphome,
                Arc::new(stt),
                Arc::new(tts),
                ask_tx.clone(),
                settings,
            )
            .await
        };
        if let Err(e) = started.await {
            eprintln!("clawdot: {e}");
            return ExitCode::from(1);
        }
    }

    // Hive milestone H0: identity and discovery. A failure is reported and the speaker carries on without it.
    #[cfg(feature = "hive")]
    if let Some(mode) = args.hive.clone() {
        let opts = clawdot_core::hive::Options {
            mode,
            port: args.listen.port(),
            addr: match args.listen.ip() {
                std::net::IpAddr::V4(a) if !a.is_unspecified() => Some(a),
                _ => None,
            },
            publish_name: args.hive_publish_name,
            name: core
                .setting("device_name")
                .as_str()
                .unwrap_or("ClawDot")
                .to_owned(),
        };
        if let Err(e) = clawdot_core::hive::start(&args.data_dir, &opts) {
            eprintln!("clawdot: hive: {e}");
        }
    }

    core.set_listen_ip(args.listen.ip());
    announce_admin(&core, &public);
    println!(
        "clawdot: listening on http://{}  (endpoints: {}; language model: {})",
        args.listen,
        core.endpoints().len(),
        if brain.llm.is_some() {
            "connected"
        } else {
            "not configured"
        }
    );
    let server_core = Arc::clone(&core);
    tokio::spawn(async move {
        if let Err(e) = serve(
            listener,
            server_core,
            ServeOptions::default(),
            std::future::pending(),
        )
        .await
        {
            eprintln!("clawdot: server stopped: {e}");
        }
    });

    let (line_tx, mut line_rx) = mpsc::channel::<String>(8);
    if args.console {
        std::thread::spawn(move || {
            let mut buf = String::new();
            while std::io::stdin().read_line(&mut buf).is_ok_and(|n| n > 0) {
                if line_tx.blocking_send(buf.trim().to_owned()).is_err() {
                    break;
                }
                buf.clear();
            }
        });
        println!("clawdot: console ready. Type what you would say; \"quit\" or Ctrl-D to leave.");
    }

    let mut portal_rx = core.attach_portal();
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    let voice = Arc::new(Voice {
        announcer: announcer.clone(),
        core: Arc::clone(&core),
        usage: brain.usage.clone(),
        usage_store: brain.usage_store.clone(),
        echo: args.console || args.echo_speech,
        available: AtomicBool::new(false),
        polling: AtomicBool::new(false),
    });
    let (say_tx, say_rx) = mpsc::unbounded_channel::<Say>();
    tokio::spawn(speak(Arc::clone(&voice), say_rx));
    loop {
        tokio::select! {
            Some(job) = portal_rx.recv() => {
                let answer = match job.op {
                    Op::Sched(op) => brain.assistant.admin_sched(op),
                    Op::Settings(live) => {
                        apply_live(&mut brain, &core, &live, &mut applied);
                        Ok(serde_json::json!({ "applied": true }))
                    }
                    Op::Rekey => {
                        // Make the next comparison see a change, so the client is built again with the new key.
                        let live = applied.clone();
                        applied.insert("llm_key_env".into(), serde_json::Value::Null);
                        apply_live(&mut brain, &core, &live, &mut applied);
                        Ok(serde_json::json!({ "applied": true }))
                    }
                    Op::Ext(op) => {
                        let r = brain.ext_apply(op).await;
                        if let Ok(listing) = &r {
                            core.set_ui_tools(listing.clone());
                        }
                        r
                    }
                };
                let _ = job.reply.send(answer);
            }
            line = line_rx.recv(), if args.console => {
                let Some(line) = line else {
                    drain(&mut brain, &mut hub, &core).await;
                    return ExitCode::SUCCESS;
                };
                if line.is_empty() { continue; }
                if is_quit(&line) { return ExitCode::SUCCESS; }
                let req = Request {
                    speaker: "console".into(),
                    line,
                    ctl: TurnCtl::default(),
                    origin: Origin::Console,
                };
                if let Some((origin, reply)) = dispatch(&mut brain, &mut hub, req) {
                    deliver(origin, reply);
                }
            }
            Some(job) = rt_rx.recv() => {
                brain.start_realtime(job, &core, &mut hub);
            }
            Some(ask) = ask_rx.recv() => {
                let ctl = TurnCtl {
                    cancel: ask.cancel.clone(),
                    speak: ask.progress.clone(),
                    clock: None,
                };
                let req = Request {
                    speaker: ask.speaker,
                    line: ask.text,
                    ctl,
                    origin: Origin::Ask(ask.reply),
                };
                if let Some((origin, reply)) = dispatch(&mut brain, &mut hub, req) {
                    deliver(origin, reply);
                }
            }
            ev = hub.next() => {
                on_hub_event(&mut brain, &mut hub, &core, ev).await;
            }
            _ = tick.tick() => {
                hub.sweep(brain.clock.now());
                let tool_counts = serde_json::json!({ "builtin": builtin.len(), "http": brain.http_tools.specs().len(), "mcp": brain.mcp.tools().len() });
                let view = SpeakerView {
                    available: announcer.connected() > 0 && voice.available.load(Ordering::Relaxed),
                    console: args.console || args.echo_speech,
                    quiet: core.quiet_hours(),
                };
                core.set_ui_status(brain.status_json(announcer.connected(), &view, &station_names, &tool_counts));
                // Everything that speaks is handed to the speaker task, which says it in order, so the wait for
                // a voice or a summary never holds this loop (ADR-0021).
                while let Some((text, preview)) = core.take_speech_as() {
                    let _ = say_tx.send(Say::Phrase { text, preview });
                }
                for fired in brain.assistant.tick() {
                    let _ = say_tx.send(Say::Line(describe_fired(&fired)));
                }
                if !voice.polling.swap(true, Ordering::Relaxed) {
                    let _ = say_tx.send(Say::Poll {
                        minute: brain.assistant.minute_of_day(),
                        day: brain.assistant.local_day(),
                        llm: brain.llm.clone(),
                    });
                }
                core.maintain();
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn addr(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    #[test]
    fn quit_and_exit_leave_the_console() {
        for l in ["quit", "exit", "Quit", " EXIT ", "quit."] {
            assert!(is_quit(l), "{l}");
        }
        for l in ["quit smoking", "what time is it", "exit the building", ""] {
            assert!(!is_quit(l), "{l}");
        }
    }

    #[test]
    fn snippet_urls_follow_the_listen_address() {
        assert_eq!(
            public_url_from_listen(addr("192.168.1.20:9000")),
            ("http://192.168.1.20:9000".to_owned(), false)
        );
        assert_eq!(
            public_url_from_listen(addr("[fe80::1]:9000")),
            ("http://[fe80::1]:9000".to_owned(), false)
        );
        for unspecified in ["0.0.0.0:9000", "[::]:9000"] {
            assert_eq!(
                public_url_from_listen(addr(unspecified)),
                ("http://clawdot.local:9000".to_owned(), true)
            );
        }
    }

    #[test]
    fn a_taken_port_gets_an_actionable_message() {
        let e = std::io::Error::from(std::io::ErrorKind::AddrInUse);
        let m = bind_error(addr("0.0.0.0:8080"), &e);
        assert!(m.contains("0.0.0.0:8080"), "{m}");
        assert!(m.contains("--listen 0.0.0.0:3335"), "{m}");
    }
}
