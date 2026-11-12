//! Start-up of the `clawdot` binary as one explicit sequence, [`start`]: the effective settings, the clock,
//! the core and its stores, the management commands, the [`Brain`] and its conversation hub, a recorded
//! utterance if asked for, the listening socket, the satellite servers, hive discovery and the update check,
//! ending in a running [`App`]. Each phase is a function below and fails with the message to print and the
//! exit code. The order is part of the contract: nothing that reveals a token is printed before the socket
//! is bound (REQ-CFG-015).

use crate::app::{Adapters, Core, Limits, Stores};
use crate::app_loop::{App, Inbox, Speaker};
use crate::assistant::Assistant;
use crate::audit::{AuditConfig, AuditLog};
use crate::cli::Args;
use crate::commands::{announce_admin, bind_error, run_endpoint_commands, spawn_console};
use crate::connects::Connects;
use crate::env::Env;
use crate::extensions::Extensions;
use crate::http::{ServeOptions, serve};
use crate::httptools::HttpTools;
use crate::mcp::McpClient;
use crate::notes::NoteStore;
use crate::rtvoice::{Realtime, RtSettings};
use crate::runtime::{Brain, build_llm, mcp_connected, settings_extra, zone_for};
use crate::satellite::{self, Ask};
use crate::sched::{Clock, Scheduler, SystemClock, Zone};
use crate::session::{Conversation, ToolHost};
use crate::settings::{SettingsState, public_url_from_listen};
use crate::speaker::{Say, Voice, speak};
use crate::speechio::{build_registered_speech, voice_once};
use crate::store::JsonStore;
use crate::turns::Hub;
use crate::usage::{UsageLimits, UsageTracker};
use std::net::SocketAddr;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tokio::net::TcpListener;
use tokio::sync::mpsc;

/// Why start-up stopped: the message to print (after `clawdot: `) and the process exit code.
#[derive(Debug)]
pub struct Failure {
    pub code: u8,
    pub message: String,
}

fn fail(code: u8, message: String) -> Failure {
    Failure { code, message }
}

/// How start-up ended when it did not fail.
#[derive(Debug)]
pub enum Started {
    /// A command that did its work and leaves (`--create`, `--rotate`, `--voice-in`).
    Done(ExitCode),
    /// The server is up; run it.
    Run(Box<App>),
}

/// The brain with its hub and the channels that report to it, between building and the first event.
struct Running {
    brain: Brain,
    hub: Hub,
    core: Arc<Core>,
    applied: serde_json::Map<String, serde_json::Value>,
    mcp_rx: mpsc::Receiver<crate::connects::Done>,
    realtime_rx: mpsc::Receiver<crate::rtvoice::Job>,
}

/// Run the start-up sequence for `args`: settings, clock, core, management commands, brain, a recorded
/// utterance if asked for, the bound socket, satellites, hive, the admin announcement and the server.
pub async fn start(mut args: Args, env: Env) -> Result<Started, Failure> {
    let (settings, warnings) =
        crate::seed::resolve_settings(&mut args, &env).map_err(|e| fail(2, e))?;
    for w in warnings {
        eprintln!("clawdot: {w}");
    }
    let zone = zone_for(args.zone.as_deref()).map_err(|e| fail(2, e))?;
    let clock = check_clock();
    migrate_admin_token(&args)?;
    let core = open_core(&args, &clock, env)?;

    let public = announce_public_address(&args);
    let failed = run_endpoint_commands(&core, &args.create, &args.rotate, &public);
    if args.manage {
        // A management command: the token is printed; do not start a server (REQ-CFG-015).
        return Ok(Started::Done(ExitCode::from(u8::from(failed))));
    }

    let stations: Vec<String> = args.stations.iter().map(|s| s.name.clone()).collect();
    let running = build_running(&mut args, settings, zone, &clock, core).await?;
    if let Some(input) = args.voice_in.clone() {
        return one_utterance(running, &args, &input)
            .await
            .map(Started::Done);
    }

    // Bind before anything is created or printed, so a server that cannot start never reveals a token
    // (REQ-CFG-015).
    let listener = TcpListener::bind(args.listen)
        .await
        .map_err(|e| fail(1, bind_error(args.listen, &e)))?;
    // Satellites (speakers) connect here; their questions come back to the loop through `asks`.
    let (ask_tx, asks) = mpsc::channel::<Ask>(8);
    let announcer = satellite::Announcer::default();
    start_satellites(
        &args,
        &running.core,
        running.brain.realtime(),
        &announcer,
        &ask_tx,
    )
    .await
    .map_err(|e| fail(1, e))?;
    #[cfg(feature = "hive")]
    start_hive(&args, &running.core);

    running.core.set_listen_ip(args.listen.ip());
    announce_admin(&running.core, &public);
    println!(
        "clawdot: listening on http://{}  (endpoints: {}; language model: {})",
        args.listen,
        running.core.endpoints().len(),
        if running.brain.has_model() {
            "connected"
        } else {
            "not configured"
        }
    );
    spawn_server(&running.core, listener);

    let (line_tx, lines) = mpsc::channel::<String>(8);
    if args.console {
        spawn_console(line_tx);
        println!("clawdot: console ready. Type what you would say; \"quit\" or Ctrl-D to leave.");
    }
    let inbox = Inbox {
        portal: running.core.attach_portal(),
        mcp: running.mcp_rx,
        realtime: running.realtime_rx,
        asks,
        lines,
        console: args.console,
    };
    let speaker = spawn_speaker(
        &running.brain,
        &running.core,
        announcer,
        args.console || args.echo_speech,
    );
    let app = App::new(
        running.brain,
        running.hub,
        running.core,
        running.applied,
        inbox,
        speaker,
    )
    .with_stations(stations);
    Ok(Started::Run(Box::new(app)))
}

/// Warn when the wall clock is not set; timers cannot fire until it is (REQ-TMR-006).
fn check_clock() -> Arc<dyn Clock + Send + Sync> {
    let clock: Arc<dyn Clock + Send + Sync> = Arc::new(SystemClock);
    if !clock.wall_clock_valid() {
        eprintln!(
            "clawdot: warning: the system clock is not set (no network time yet). Timers, alarms, and reminders will not fire, and the time cannot be told, until it is set (REQ-TMR-006)."
        );
    }
    clock
}

/// Convert an administrator token from before passwords, if one is on disk.
fn migrate_admin_token(args: &Args) -> Result<(), Failure> {
    match crate::admin::migrate_legacy(&args.data_dir) {
        Ok(true) => println!(
            "clawdot: the old administrator token still works: sign in with it, then set a password in the web UI"
        ),
        Ok(false) => {}
        Err(e) => {
            return Err(fail(
                1,
                format!("cannot convert the old administrator token: {e}"),
            ));
        }
    }
    Ok(())
}

/// Open the core over the data directory, keeping the process environment it was started with.
fn open_core(
    args: &Args,
    clock: &Arc<dyn Clock + Send + Sync>,
    env: Env,
) -> Result<Arc<Core>, Failure> {
    Core::open(
        clock.clone(),
        Adapters::builtin(),
        Limits::default(),
        Stores::in_dir(&args.data_dir),
    )
    .map(|c| Arc::new(c.with_env(env)))
    .map_err(|e| fail(1, format!("cannot open {}: {e}", args.data_dir.display())))
}

/// The address snippets name for this server, with a note when it is only a guess. Returns it.
fn announce_public_address(args: &Args) -> String {
    let public = args.public_url.clone().unwrap_or_default();
    let (derived, assumed) = public_url_from_listen(args.listen);
    if assumed && public == derived {
        eprintln!(
            "clawdot: {} is not reachable as written, so snippets use {derived}; set the public address to change that",
            args.listen.ip()
        );
    }
    public
}

/// Build the brain and hub, claim the channels that report to them, and publish what the web UI shows.
async fn build_running(
    args: &mut Args,
    mut settings: SettingsState,
    zone: Box<dyn Zone + Send + Sync>,
    clock: &Arc<dyn Clock + Send + Sync>,
    core: Arc<Core>,
) -> Result<Running, Failure> {
    let (mut brain, hub) = build_brain(args, &settings.live_values(), zone, clock, &core).await?;
    let realtime_rx = brain.attach_realtime();
    let mcp_rx = brain.attach_connects().ok_or_else(|| {
        fail(
            1,
            "cannot start: the connection results were already claimed".to_owned(),
        )
    })?;

    core.set_ui_tools(brain.listing());
    let applied = settings.live_values();
    if let Some(url) = &args.speech.url {
        core.set_speech_origin(crate::speech::SpeechOrigin::new(
            url,
            args.speech.key_env.as_deref(),
            args.speech.dialect(),
        ));
    }
    #[cfg(feature = "tls")]
    start_updates(args, &applied, &core);
    settings.set_extra(settings_extra(args));
    core.set_settings(settings);
    Ok(Running {
        brain,
        hub,
        core,
        applied,
        mcp_rx,
        realtime_rx,
    })
}

/// Answer one recorded utterance and leave. There is no loop to serve meanwhile, so wait for the MCP sources
/// to answer first, as the single turn should have their tools.
async fn one_utterance(
    mut r: Running,
    args: &Args,
    input: &std::path::Path,
) -> Result<ExitCode, Failure> {
    while r.brain.is_connecting() {
        let Some(done) = r.mcp_rx.recv().await else {
            break;
        };
        mcp_connected(&mut r.brain, &r.core, done);
    }
    voice_once(
        &mut r.brain,
        &mut r.hub,
        &args.speech,
        &r.core,
        input,
        args.voice_out.as_deref(),
    )
    .await
    .map(|()| ExitCode::SUCCESS)
    .map_err(|e| fail(1, e))
}

/// Serve the web UI and the webhooks on `listener` in a task.
fn spawn_server(core: &Arc<Core>, listener: TcpListener) {
    let core = Arc::clone(core);
    tokio::spawn(async move {
        if let Err(e) = serve(
            listener,
            core,
            ServeOptions::default(),
            std::future::pending(),
        )
        .await
        {
            eprintln!("clawdot: server stopped: {e}");
        }
    });
}

/// Start the speaker task and return the loop's side of it.
fn spawn_speaker(
    brain: &Brain,
    core: &Arc<Core>,
    announcer: satellite::Announcer,
    console: bool,
) -> Speaker {
    let (usage, usage_store) = brain.usage_handles();
    let voice = Arc::new(Voice {
        announcer: announcer.clone(),
        core: Arc::clone(core),
        usage,
        usage_store,
        echo: console,
        available: AtomicBool::new(false),
        polling: AtomicBool::new(false),
    });
    let (tx, rx) = mpsc::unbounded_channel::<Say>();
    tokio::spawn(speak(Arc::clone(&voice), rx));
    Speaker {
        tx,
        voice,
        announcer,
        console,
        lost: false,
    }
}

/// Open the stores and build the [`Brain`] and the per-speaker conversation [`Hub`] (REQ-AST-026) from the
/// effective settings in `args` and the live values. Moves the tools, MCP servers and stations out of `args`.
/// Each step fails with its own message and exit code.
pub async fn build_brain(
    args: &mut Args,
    live: &serde_json::Map<String, serde_json::Value>,
    zone: Box<dyn Zone + Send + Sync>,
    clock: &Arc<dyn Clock + Send + Sync>,
    core: &Arc<Core>,
) -> Result<(Brain, Hub), Failure> {
    let (llm_usd_per_mtok_in, llm_usd_per_mtok_out) = crate::runtime::model_prices(live);
    let limits = UsageLimits {
        daily_spend_cap_usd: args.daily_spend,
        llm_usd_per_mtok_in,
        llm_usd_per_mtok_out,
        ..UsageLimits::default()
    };
    let usage_store = JsonStore::new(args.data_dir.join("usage.json"), crate::usage::SCHEMA);
    let usage = UsageTracker::load(&usage_store, limits)
        .map_err(|e| fail(1, format!("cannot load usage counters: {e}")))?;

    let llm = build_llm(
        args.llm_url.as_deref(),
        args.llm_model.as_deref(),
        core.secrets(),
        core.env(),
        &args.key_env,
        limits.max_output_tokens_per_turn,
        &args.agent,
    )
    .map_err(|e| fail(2, e))?;
    let persona = args.persona.clone();
    let (audit, notes) = open_records(args, core)?;
    let assistant = open_assistant(args, zone, clock, core)?;
    let tools = declare_tools(args, core, &assistant)?;
    let hub = make_hub(&persona, audit, notes);

    let ext_view = Arc::new(tools.ext.snapshot());
    let brain = Brain {
        assistant,
        llm,
        usage,
        usage_store,
        writer: core.writer(),
        http_tools: Arc::new(tools.http_tools),
        mcp: Arc::new(tools.mcp),
        clock: clock.clone(),
        ext: tools.ext,
        ext_view,
        builtin: tools.builtin,
        persona,
        agent: args.agent.clone(),
        hardware: crate::hardware::detected()
            .clone()
            .with_device(&args.device),
        voice_configured: args.speech.url.is_some() && args.speech.tts_model.is_some(),
        rt: Realtime::with_env_and_secrets(
            RtSettings::from_live(live),
            core.env().lookup(),
            core.secrets().clone(),
        ),
        connects: tools.connects,
    };
    if brain.rt.settings().enabled && !RtSettings::built() {
        brain.rt.set_error(Some(
            "Realtime voice is turned on, but this build of ClawDot does not include it.".into(),
        ));
    }
    Ok((brain, hub))
}

/// Open the audit log and the notes, hand them to the core, and return the handles every conversation shares.
fn open_records(args: &Args, core: &Core) -> Result<(Arc<AuditLog>, Arc<NoteStore>), Failure> {
    // Every action is recorded in the data directory and held to the daily caps (REQ-SEC-014..016).
    let audit = AuditLog::open(
        &args.data_dir,
        AuditConfig {
            policy: args.actions,
            ..AuditConfig::default()
        },
    )
    .map(Arc::new)
    .map_err(|e| {
        fail(
            1,
            format!(
                "cannot open the audit log in {}: {e}",
                args.data_dir.display()
            ),
        )
    })?;
    let audit_for_conv = Arc::clone(&audit);
    core.set_audit(audit);
    core.set_quiet_hours(args.quiet);
    // Notes the user asks for (REQ-AST-023); off unless the `agent` section turns them on.
    let notes = NoteStore::open(&args.data_dir, args.agent.notes.unwrap_or(false))
        .map(Arc::new)
        .map_err(|e| {
            fail(
                1,
                format!("cannot open the notes in {}: {e}", args.data_dir.display()),
            )
        })?;
    let notes_for_conv = Arc::clone(&notes);
    core.set_notes(notes);
    if args.actions.dry_run {
        eprintln!("clawdot: dry-run mode: actions are described and recorded, not run");
    }
    Ok((audit_for_conv, notes_for_conv))
}

/// Load the timers and build the assistant with its built-in tools. Takes the stations out of `args`.
fn open_assistant(
    args: &mut Args,
    zone: Box<dyn Zone + Send + Sync>,
    clock: &Arc<dyn Clock + Send + Sync>,
    core: &Arc<Core>,
) -> Result<Assistant, Failure> {
    let sched_store = JsonStore::new(args.data_dir.join("timers.json"), crate::sched::SCHEMA);
    let sched = Scheduler::load(&sched_store, 64)
        .map_err(|e| fail(1, format!("cannot load timers: {e}")))?;
    Ok(Assistant::new(
        clock.clone(),
        zone,
        sched,
        Some(sched_store),
        Some(Arc::clone(core)),
    )
    .with_stations(std::mem::take(&mut args.stations)))
}

/// The tools beyond the built-in ones, and the MCP connections to make.
struct Tools {
    builtin: Vec<String>,
    ext: Extensions,
    http_tools: HttpTools,
    mcp: McpClient,
    connects: Connects,
}

/// Load the portal's saved tools and skills, define the HTTP tools, and declare the MCP sources. Takes the
/// tool and server definitions out of `args`.
fn declare_tools(args: &mut Args, core: &Core, assistant: &Assistant) -> Result<Tools, Failure> {
    let builtin: Vec<String> = ToolHost::tool_specs(assistant)
        .into_iter()
        .map(|t| t.name)
        .collect();
    // What the portal saved (tools, connections, skills) loads after what the configuration file defines.
    let mut ext = Extensions::open(&args.data_dir).map_err(|e| {
        fail(
            1,
            format!("cannot load the portal's saved tools and skills: {e}"),
        )
    })?;
    let mut tool_defs = std::mem::take(&mut args.tools);
    let mut server_defs = std::mem::take(&mut args.mcp);
    ext.set_config(
        tool_defs.iter().map(|t| t.name.clone()),
        server_defs.iter().map(|s| s.name.clone()),
    );
    let mut taken = builtin.clone();
    taken.extend(tool_defs.iter().map(|t| t.name.clone()));
    tool_defs.extend(ext.startup_tools(&taken, |v| core.env().get(v)));
    server_defs.extend(ext.startup_servers(|v| core.env().get(v)));
    let http_tools =
        HttpTools::new(tool_defs, &builtin, |v| core.env().get(v)).map_err(|e| fail(1, e))?;

    // Saved and configured MCP sources are declared here and connected in tasks once the loop runs
    // (ADR-0021): a dead server delays only itself, never the start of the speaker. Bad definitions still
    // stop start-up.
    let (mcp, pending) =
        McpClient::declare(server_defs, |v| core.env().get(v)).map_err(|e| fail(1, e))?;
    let mut connects = Connects::new();
    for p in pending {
        connects.start_saved(p);
    }
    Ok(Tools {
        builtin,
        ext,
        http_tools,
        mcp,
        connects,
    })
}

/// One conversation for each speaker, made on first use (REQ-AST-026): its own history, its own held
/// confirmation, the same audit log and notes.
fn make_hub(persona: &str, audit: Arc<AuditLog>, notes: Arc<NoteStore>) -> Hub {
    let persona = persona.to_owned();
    Hub::new(move || {
        let mut c = Conversation::standard(persona.clone());
        c.set_audit(Arc::clone(&audit));
        c.set_notes(Arc::clone(&notes));
        c
    })
}

/// Start the optional update check (`tls` builds only); a failure is reported and the speaker carries on.
#[cfg(feature = "tls")]
pub fn start_updates(args: &Args, live: &serde_json::Map<String, serde_json::Value>, core: &Core) {
    match crate::update::UpdateConfig::new(
        args.updates.url.as_deref(),
        args.updates.public_key.as_deref(),
        core.env(),
    ) {
        Ok(cfg) => {
            let on = live
                .get("update_check")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true);
            // After an install the updater asks for an orderly stop: flush the store writer, then exit (not `exit`
            // alone, which would drop writes still queued on the writer thread).
            let writer = core.writer();
            let stop: crate::update::StopFn = std::sync::Arc::new(move |code| {
                if let Some(w) = &writer {
                    w.flush_blocking();
                }
                std::process::exit(code);
            });
            match crate::update::start(cfg, on, Some(args.data_dir.clone()), stop) {
                Ok(u) => core.set_updater(u),
                Err(e) => crate::diag::error(&format!("updates are not available: {e}")),
            }
        }
        Err(e) => crate::diag::error(&format!("updates are not available: {e}")),
    }
}

/// What every kind of satellite shares: the speech to text and text to speech clients (registered for the
/// portal's status), and the settings that carry the announcer, the realtime voice and the follow-up window.
fn satellite_parts(
    args: &Args,
    core: &Core,
    rt: &Realtime,
    announcer: &satellite::Announcer,
) -> Result<
    (
        Arc<crate::speech::HttpTranscriber>,
        Arc<crate::speech::HttpSynthesizer>,
        satellite::Settings,
    ),
    String,
> {
    let (stt, tts) = build_registered_speech(
        &args.speech,
        core.secrets(),
        core.env(),
        core.speech_handles(),
    )?;
    let mut settings = satellite::Settings {
        announcer: announcer.clone(),
        realtime: Some(rt.clone()),
        ..satellite::Settings::default()
    };
    if let Some(s) = args.satellite.follow_up_secs {
        settings.dialog.follow_up_secs = s;
    }
    Ok((Arc::new(stt), Arc::new(tts), settings))
}

/// Start the satellite listener (`satellite.listen`) and the ESPHome satellites, when configured. Their
/// questions come back through `ask_tx`. The error is the message to print.
pub async fn start_satellites(
    args: &Args,
    core: &Arc<Core>,
    rt: &Realtime,
    announcer: &satellite::Announcer,
    ask_tx: &mpsc::Sender<Ask>,
) -> Result<(), String> {
    if let Some(listen) = &args.satellite.listen {
        let addr: SocketAddr = listen
            .parse()
            .map_err(|e| format!("satellite.listen: {e}"))?;
        let (stt, tts, mut settings) = satellite_parts(args, core, rt, announcer)?;
        if let Some(var) = &args.satellite.token_env {
            let token = core.env().get(var).unwrap_or_default();
            if token.is_empty() {
                return Err(format!(
                    "environment variable {var} (satellite.token_env) is not set"
                ));
            }
            settings.token_hashes = vec![crate::inbox::endpoint::hash_token(&token)];
        }
        settings.admin_reset = Some(satellite::AdminResetHook::new({
            let core = Arc::clone(core);
            move || {
                use crate::admin::AdminError;
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
            if let Err(e) =
                satellite::serve(l, stt, tts, tx, settings, std::future::pending()).await
            {
                eprintln!("clawdot: satellite server stopped: {e}");
            }
        });
    }

    // ESPHome voice satellites (optional feature; spec 0004) share the satellite pipeline and announcer.
    #[cfg(feature = "esphome-satellite")]
    if !args.esphome.is_empty() {
        let (stt, tts, settings) = satellite_parts(args, core, rt, announcer)?;
        crate::esphome::start(
            &args.esphome,
            core.env(),
            stt,
            tts,
            ask_tx.clone(),
            settings,
        )
        .await?;
    }
    Ok(())
}

/// Hive milestone H0: identity and discovery. A failure is reported and the speaker carries on without it.
#[cfg(feature = "hive")]
pub fn start_hive(args: &Args, core: &Core) {
    if let Some(mode) = args.hive.clone() {
        let opts = crate::hive::Options {
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
        let started = crate::hive::start(&args.data_dir, &opts);
        if let Some(e) = started.problem {
            eprintln!("clawdot: hive: {e}");
        }
        if let Some(h) = started.hive {
            core.set_hive(h);
        }
    }
}
