//! `clawdot`: the core server with a text console standing in for a satellite.
//!
//! Until a real audio satellite exists (milestone M1), this runs the complete inbox loop end to end:
//! agents post webhooks, the console "speaks" announcements, and what you type is handled exactly as a
//! transcribed utterance would be (fast path first).

use clawdot_core::app::{Adapters, Core, Limits, Stores};
use clawdot_core::assistant::{Assistant, describe_fired};
use clawdot_core::http::{ServeOptions, serve};
use clawdot_core::inbox::summary::announcement;
use clawdot_core::inbox::summary::fallback_summary;
use clawdot_core::sched::{Clock, FixedOffset, Scheduler, SystemClock, Zone};
use clawdot_core::store::JsonStore;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::mpsc;

const HELP: &str = "\
clawdot: ClawDot core with a text console (pre-alpha; no audio yet)

USAGE:
    clawdot [options]

OPTIONS:
    --data-dir DIR           Where state is kept (default: ./clawdot-data)
    --listen ADDR            Address for the HTTP server (default: 127.0.0.1:8080)
    --public-url URL         How agents reach this device, used in printed snippets
    --zone NAME              IANA time zone, for example America/New_York (default: UTC)
    --create-endpoint SPEC   NAME or NAME:ADAPTER (adapters: claude-code, generic); prints the token once
    --rotate-endpoint NAME   Issue a new token for an endpoint
    --no-console             Do not read the console (run as a server only)
    -h, --help               Show this help

Type a command at the console (for example: set a timer for ten minutes, or: what are my agents up to).
";

struct Args {
    data_dir: PathBuf,
    listen: SocketAddr,
    public_url: Option<String>,
    zone: Option<String>,
    create: Vec<String>,
    rotate: Vec<String>,
    console: bool,
}

fn parse() -> Result<Args, String> {
    let mut a = Args {
        data_dir: PathBuf::from("clawdot-data"),
        listen: "127.0.0.1:8080"
            .parse()
            .map_err(|_| "internal: bad default address".to_owned())?,
        public_url: None,
        zone: None,
        create: Vec::new(),
        rotate: Vec::new(),
        console: true,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = |n: &str| it.next().ok_or_else(|| format!("{n} needs a value"));
        match flag.as_str() {
            "-h" | "--help" => return Err(String::new()),
            "--data-dir" => a.data_dir = PathBuf::from(value("--data-dir")?),
            "--listen" => {
                a.listen = value("--listen")?
                    .parse()
                    .map_err(|e| format!("bad --listen: {e}"))?
            }
            "--public-url" => a.public_url = Some(value("--public-url")?),
            "--zone" => a.zone = Some(value("--zone")?),
            "--create-endpoint" => a.create.push(value("--create-endpoint")?),
            "--rotate-endpoint" => a.rotate.push(value("--rotate-endpoint")?),
            "--no-console" => a.console = false,
            other => return Err(format!("unknown option `{other}`")),
        }
    }
    Ok(a)
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
            "Claude Code hooks: add to ~/.claude/settings.json (replace nothing else in your file):"
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

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let args = match parse() {
        Ok(a) => a,
        Err(m) if m.is_empty() => {
            print!("{HELP}");
            return ExitCode::SUCCESS;
        }
        Err(m) => {
            eprintln!("clawdot: {m}\n\n{HELP}");
            return ExitCode::from(2);
        }
    };
    let zone = match zone_for(args.zone.as_deref()) {
        Ok(z) => z,
        Err(e) => {
            eprintln!("clawdot: {e}");
            return ExitCode::from(2);
        }
    };

    let clock: Arc<dyn Clock + Send + Sync> = Arc::new(SystemClock);
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

    let public = args
        .public_url
        .clone()
        .unwrap_or_else(|| format!("http://{}", args.listen));
    for spec in &args.create {
        let (name, adapter) = spec.split_once(':').unwrap_or((spec, spec));
        let adapter = if matches!(adapter, "claude-code" | "generic") {
            adapter
        } else {
            "generic"
        };
        match core.create_endpoint(name, adapter) {
            Ok((_, token)) => print_snippet(&public, name, adapter, &token),
            Err(e) => eprintln!("clawdot: endpoint `{name}`: {e}"),
        }
    }
    for name in &args.rotate {
        match core.rotate_endpoint(name) {
            Ok(token) => println!("New token for `{name}` (shown once): {token}"),
            Err(e) => eprintln!("clawdot: endpoint `{name}`: {e}"),
        }
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
    let mut assistant = Assistant::new(
        clock,
        zone,
        sched,
        Some(sched_store),
        Some(Arc::clone(&core)),
    );

    let listener = match TcpListener::bind(args.listen).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("clawdot: cannot listen on {}: {e}", args.listen);
            return ExitCode::from(1);
        }
    };
    println!(
        "clawdot: listening on http://{}  (endpoints: {})",
        args.listen,
        core.endpoints().len()
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
        println!("clawdot: console ready. Type what you would say; Ctrl-D to quit.");
    }

    let mut tick = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            line = line_rx.recv(), if args.console => {
                let Some(line) = line else { return ExitCode::SUCCESS };
                if line.is_empty() { continue; }
                match assistant.handle(&line) {
                    Some(out) if out.say.is_empty() => println!("speaker> (done: {:?})", out.effects),
                    Some(out) => println!("speaker> {}", out.say),
                    None => println!("speaker> (that needs the language model, which is not connected yet)"),
                }
            }
            _ = tick.tick() => {
                for fired in assistant.tick() {
                    println!("speaker> {}", describe_fired(&fired));
                }
                if let Some(next) = core.next_announcement(true, assistant.minute_of_day()) {
                    // Until an LLM is wired in, say only the headline and outcome, never raw agent text.
                    let spoken = fallback_summary(&next.event);
                    println!("speaker> {}", announcement(&next.event, &spoken, next.more_waiting));
                    core.mark_announced(&[next.event.id]);
                }
                core.maintain();
            }
        }
    }
}
