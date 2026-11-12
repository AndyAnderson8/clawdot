//! `clawdot`: the core server with a text console standing in for a satellite.
//!
//! Until a real audio satellite exists (milestone M1), this runs the complete loop end to end: agents
//! post webhooks, the console "speaks" announcements, and what you type is handled exactly as a
//! transcribed utterance would be: the local fast path first, then the language model if one is
//! configured. This file only chooses what to run; the start-up sequence is `startup::start` and the loop
//! is `app_loop::App`.

use clawdot_core::cli;
use clawdot_core::commands::{reset_admin, say_command};
use clawdot_core::env::Env;
use clawdot_core::startup::{Started, start};
use std::process::ExitCode;

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    // The one read of the process environment (see clawdot_core::env).
    let env = Env::from_process();
    let argv: Vec<String> = std::env::args().skip(1).collect();
    match argv.first().map(String::as_str) {
        // Used by an update to check that a new program starts before it replaces the running one.
        Some("--version" | "-V") => {
            println!("clawdot {}", clawdot_core::diag::build_id());
            return ExitCode::SUCCESS;
        }
        Some("say") => return say_command(&argv[1..], &env).await,
        _ => {}
    }
    let args = match cli::parse(&argv) {
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
    match start(args, env).await {
        Ok(Started::Run(app)) => app.run().await,
        Ok(Started::Done(code)) => code,
        Err(f) => {
            eprintln!("clawdot: {}", f.message);
            ExitCode::from(f.code)
        }
    }
}
