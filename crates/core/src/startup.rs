//! Start-up phases of the `clawdot` binary, in the order `main` runs them: build the [`Brain`] and its
//! conversation hub from the effective settings, then start the satellite servers, hive discovery and
//! the update check. Each returns what `main` needs to print and exit with when it fails.

use crate::app::Core;
use crate::assistant::Assistant;
use crate::audit::{AuditConfig, AuditLog};
use crate::cli::Args;
use crate::connects::Connects;
use crate::extensions::Extensions;
use crate::httptools::HttpTools;
use crate::mcp::McpClient;
use crate::notes::NoteStore;
use crate::rtvoice::{Realtime, RtSettings};
use crate::runtime::{Brain, build_llm};
use crate::satellite::{self, Ask};
use crate::sched::{Clock, Scheduler, Zone};
use crate::session::{Conversation, ToolHost};
use crate::speechio::build_registered_speech;
use crate::store::JsonStore;
use crate::turns::Hub;
use crate::usage::{UsageLimits, UsageTracker};
use std::net::SocketAddr;
use std::sync::Arc;
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
