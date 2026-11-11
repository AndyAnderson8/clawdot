//! `clawdot`: the core server with a text console standing in for a satellite.
//!
//! Until a real audio satellite exists (milestone M1), this runs the complete loop end to end: agents
//! post webhooks, the console "speaks" announcements, and what you type is handled exactly as a
//! transcribed utterance would be: the local fast path first, then the language model if one is
//! configured.

use clawdot_core::app::{Adapters, Core, Limits, Stores};
use clawdot_core::assistant::describe_fired;
use clawdot_core::cli;
use clawdot_core::commands::{
    announce_admin, bind_error, is_quit, reset_admin, run_endpoint_commands, say_command,
    spawn_console,
};
use clawdot_core::http::{ServeOptions, serve};
use clawdot_core::runtime::{
    SpeakerView, deliver, dispatch, drain, on_hub_event, portal_op, settings_extra, zone_for,
};
use clawdot_core::satellite::{self, Ask};
use clawdot_core::sched::{Clock, SystemClock};
use clawdot_core::seed;
use clawdot_core::session::TurnCtl;
use clawdot_core::settings::public_url_from_listen;
use clawdot_core::speaker::{Say, Voice, speak};
use clawdot_core::speechio::voice_once;
#[cfg(feature = "hive")]
use clawdot_core::startup::start_hive;
#[cfg(feature = "tls")]
use clawdot_core::startup::start_updates;
use clawdot_core::startup::{build_brain, start_satellites};
use clawdot_core::turns::{Origin, Request};
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
    let failed = run_endpoint_commands(&core, &args.create, &args.rotate, &public);
    if args.manage {
        // A management command: the token is printed; do not start a server (REQ-CFG-015).
        return ExitCode::from(u8::from(failed));
    }

    let station_names: Vec<String> = args.stations.iter().map(|s| s.name.clone()).collect();
    let (mut brain, mut hub) = match build_brain(
        &mut args,
        &settings_state.live_values(),
        zone,
        &clock,
        &core,
    )
    .await
    {
        Ok(r) => r,
        Err(f) => {
            eprintln!("clawdot: {}", f.message);
            return ExitCode::from(f.code);
        }
    };
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
    start_updates(&args, &applied);
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
    if let Err(e) = start_satellites(&args, &core, &brain.rt, &announcer, &ask_tx).await {
        eprintln!("clawdot: {e}");
        return ExitCode::from(1);
    }
    #[cfg(feature = "hive")]
    start_hive(&args, &core);

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
        spawn_console(line_tx);
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
                let answer = portal_op(&mut brain, &core, &mut applied, job.op).await;
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
                let tool_counts = serde_json::json!({ "builtin": brain.builtin.len(), "http": brain.http_tools.specs().len(), "mcp": brain.mcp.tools().len() });
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
