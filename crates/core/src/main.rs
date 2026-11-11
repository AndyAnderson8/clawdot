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
use clawdot_core::cli;
use clawdot_core::commands::{
    announce_admin, bind_error, is_quit, print_snippet, reset_admin, say_command,
};
use clawdot_core::extensions::Extensions;
use clawdot_core::http::{ServeOptions, serve};
use clawdot_core::httptools::HttpTools;
use clawdot_core::mcp::McpClient;
use clawdot_core::notes::NoteStore;
use clawdot_core::rtvoice::{Realtime, RtSettings};
use clawdot_core::runtime::{
    Brain, SpeakerView, apply_live, build_llm, deliver, dispatch, drain, on_hub_event,
    settings_extra, zone_for,
};
use clawdot_core::satellite::{self, Ask};
use clawdot_core::sched::{Clock, Scheduler, SystemClock};
use clawdot_core::seed;
use clawdot_core::session::{Conversation, ToolHost, TurnCtl};
use clawdot_core::settings::public_url_from_listen;
use clawdot_core::speaker::{Say, Voice, speak};
use clawdot_core::speechio::build_registered_speech;
use clawdot_core::speechio::voice_once;
use clawdot_core::store::JsonStore;
use clawdot_core::turns::{Hub, Origin, Request};
use clawdot_core::usage::{UsageLimits, UsageTracker};
use std::net::SocketAddr;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::mpsc;

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
