//! Start-up phases of the `clawdot` binary, in the order `main` runs them: build the [`Brain`] and its
//! conversation hub from the effective settings, then start the satellite servers, hive discovery and
//! the update check. Each returns what `main` needs to print and exit with when it fails.

use crate::app::Core;
use crate::assistant::Assistant;
use crate::audit::{AuditConfig, AuditLog};
use crate::cli::Args;
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
pub async fn build_brain(
    args: &mut Args,
    live: &serde_json::Map<String, serde_json::Value>,
    zone: Box<dyn Zone + Send + Sync>,
    clock: &Arc<dyn Clock + Send + Sync>,
    core: &Arc<Core>,
) -> Result<(Brain, Hub), Failure> {
    let limits = UsageLimits {
        daily_spend_cap_usd: args.daily_spend,
        ..UsageLimits::default()
    };
    let usage_store = JsonStore::new(args.data_dir.join("usage.json"), crate::usage::SCHEMA);
    let usage = UsageTracker::load(&usage_store, limits)
        .map_err(|e| fail(1, format!("cannot load usage counters: {e}")))?;

    let llm = build_llm(
        args.llm_url.as_deref(),
        args.llm_model.as_deref(),
        &args.key_env,
        limits.max_output_tokens_per_turn,
        &args.agent,
    )
    .map_err(|e| fail(2, e))?;
    let persona = args.persona.clone();
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

    let sched_store = JsonStore::new(args.data_dir.join("timers.json"), crate::sched::SCHEMA);
    let sched = Scheduler::load(&sched_store, 64)
        .map_err(|e| fail(1, format!("cannot load timers: {e}")))?;
    let assistant = Assistant::new(
        clock.clone(),
        zone,
        sched,
        Some(sched_store),
        Some(Arc::clone(core)),
    )
    .with_stations(std::mem::take(&mut args.stations));

    let builtin: Vec<String> = ToolHost::tool_specs(&assistant)
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
    tool_defs.extend(ext.startup_tools(&taken, |v| std::env::var(v).ok()));
    server_defs.extend(ext.startup_servers(|v| std::env::var(v).ok()));
    let http_tools =
        HttpTools::new(tool_defs, &builtin, |v| std::env::var(v).ok()).map_err(|e| fail(1, e))?;

    let mut reserved = builtin.clone();
    reserved.extend(http_tools.specs().into_iter().map(|t| t.name));
    let mcp = match McpClient::connect(server_defs, &reserved, |v| std::env::var(v).ok()).await {
        Ok((client, warnings)) => {
            for w in warnings {
                eprintln!("clawdot: {w}");
            }
            client
        }
        Err(e) => return Err(fail(1, e)),
    };

    // One conversation for each speaker, made on first use (REQ-AST-026): its own history, its own held
    // confirmation, the same audit log and notes.
    let hub = Hub::new({
        let persona = persona.clone();
        move || {
            let mut c = Conversation::new(persona.clone(), 20, 24_000, 600);
            c.set_audit(Arc::clone(&audit_for_conv));
            c.set_notes(Arc::clone(&notes_for_conv));
            c
        }
    });
    let ext_view = Arc::new(ext.snapshot());
    let brain = Brain {
        assistant,
        llm,
        usage,
        usage_store,
        http_tools: Arc::new(http_tools),
        mcp: Arc::new(mcp),
        clock: clock.clone(),
        ext,
        ext_view,
        builtin,
        persona,
        agent: args.agent.clone(),
        voice_configured: args.speech.url.is_some() && args.speech.tts_model.is_some(),
        rt: Realtime::new(RtSettings::from_live(live)),
    };
    if brain.rt.settings().enabled && !RtSettings::built() {
        brain.rt.set_error(Some(
            "Realtime voice is turned on, but this build of ClawDot does not include it.".into(),
        ));
    }
    Ok((brain, hub))
}

/// Start the optional update check (`tls` builds only); a failure is reported and the speaker carries on.
#[cfg(feature = "tls")]
pub fn start_updates(args: &Args, live: &serde_json::Map<String, serde_json::Value>) {
    match crate::update::UpdateConfig::new(
        args.updates.url.as_deref(),
        args.updates.public_key.as_deref(),
    ) {
        Ok(cfg) => {
            let on = live
                .get("update_check")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true);
            if let Err(e) = crate::update::start(cfg, on, Some(args.data_dir.clone())) {
                crate::diag::error(&format!("updates are not available: {e}"));
            }
        }
        Err(e) => crate::diag::error(&format!("updates are not available: {e}")),
    }
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
        let (stt, tts) = build_registered_speech(&args.speech)?;
        let mut settings = satellite::Settings {
            announcer: announcer.clone(),
            realtime: Some(rt.clone()),
            ..satellite::Settings::default()
        };
        if let Some(var) = &args.satellite.token_env {
            let token = std::env::var(var).unwrap_or_default();
            if token.is_empty() {
                return Err(format!(
                    "environment variable {var} (satellite.token_env) is not set"
                ));
            }
            settings.token_hashes = vec![crate::inbox::endpoint::hash_token(&token)];
        }
        if let Some(s) = args.satellite.follow_up_secs {
            settings.dialog.follow_up_secs = s;
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
    }

    // ESPHome voice satellites (optional feature; spec 0004) share the satellite pipeline and announcer.
    #[cfg(feature = "esphome-satellite")]
    if !args.esphome.is_empty() {
        let (stt, tts) = build_registered_speech(&args.speech)?;
        let mut settings = satellite::Settings {
            announcer: announcer.clone(),
            realtime: Some(rt.clone()),
            ..satellite::Settings::default()
        };
        if let Some(s) = args.satellite.follow_up_secs {
            settings.dialog.follow_up_secs = s;
        }
        crate::esphome::start(
            &args.esphome,
            Arc::new(stt),
            Arc::new(tts),
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
        if let Err(e) = crate::hive::start(&args.data_dir, &opts) {
            eprintln!("clawdot: hive: {e}");
        }
    }
}
