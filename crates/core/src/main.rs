//! `clawdot`: the core program. It serves the portal and the HTTP API, accepts satellites, runs the assistant, and
//! keeps a text console that stands in for a satellite: what you type is handled exactly as a transcribed
//! utterance would be (the local fast path first, then the language model if one is configured), and what the
//! speaker would say is printed. This file only chooses what to run; the start-up sequence is `startup::start` and
//! the loop is `app_loop::App`.

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
    let (cli, file) = match cli::parse(&argv) {
        Ok(parsed) => parsed,
        Err(m) if m.is_empty() => {
            print!("{}", cli::HELP);
            return ExitCode::SUCCESS;
        }
        Err(m) => {
            eprintln!("clawdot: {m}\n\n{}", cli::HELP);
            return ExitCode::from(2);
        }
    };
    if cli.reset_admin {
        return reset_admin(&cli.data_dir(&file));
    }
    match start(cli, file, env).await {
        Ok(Started::Run(app)) => app.run(clawdot_core::shutdown::requested()).await,
        Ok(Started::Done(code)) => code,
        Err(f) => {
            eprintln!("clawdot: {}", f.message);
            ExitCode::from(f.code)
        }
    }
}
