//! `clawdot-bridge`: connect an agent to a ClawDot speaker.

use clawdot_bridge::client::Url;
use clawdot_bridge::harness::ClaudeCode;
use clawdot_bridge::state::Handled;
use clawdot_bridge::{Config, DryRunner, ProcessRunner, Runner, stream_once};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

const HELP: &str = "\
clawdot-bridge: let a ClawDot speaker resume your coding agent by voice.

USAGE:
    clawdot-bridge --url http://HOST:PORT --endpoint NAME [options]

The endpoint token is read from the CLAWDOT_TOKEN environment variable (preferred) or --token-file.

OPTIONS:
    --url URL            The speaker, for example http://clawdot.local:3335 (plain http; use a VPN
                         such as Tailscale to reach it from elsewhere)
    --endpoint NAME      The endpoint to listen on (default: claude-code)
    --token-file FILE    Read the token from a file
    --harness NAME       Agent harness (default and only supported: claude-code)
    --claude-bin PATH    The claude executable (default: claude)
    --claude-arg ARG     Extra argument for claude; repeatable, for example
                         --claude-arg --permission-mode --claude-arg acceptEdits
    --prefix TEXT        Text placed before the spoken words
    --state FILE         Where handled reply ids are remembered
    --dry-run            Print the commands instead of running them
    --once               Handle replies on one connection, then exit when it closes
    -h, --help           Show this help

SECURITY: spoken replies steer a powerful agent. Run the agent under its own permission settings;
the speaker is a messenger, not an authorisation layer.
";

struct Args {
    url: Option<String>,
    endpoint: String,
    token_file: Option<PathBuf>,
    claude: ClaudeCode,
    state: Option<PathBuf>,
    dry_run: bool,
    once: bool,
}

fn parse() -> Result<Args, String> {
    let mut a = Args {
        url: None,
        endpoint: "claude-code".into(),
        token_file: None,
        claude: ClaudeCode::default(),
        state: None,
        dry_run: false,
        once: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
        match flag.as_str() {
            "-h" | "--help" => return Err(String::new()),
            "--url" => a.url = Some(value("--url")?),
            "--endpoint" => a.endpoint = value("--endpoint")?,
            "--token-file" => a.token_file = Some(PathBuf::from(value("--token-file")?)),
            "--harness" => {
                let h = value("--harness")?;
                if h != "claude-code" {
                    return Err(format!(
                        "harness `{h}` is not supported yet (only claude-code)"
                    ));
                }
            }
            "--claude-bin" => a.claude.bin = value("--claude-bin")?,
            "--claude-arg" => a.claude.extra_args.push(value("--claude-arg")?),
            "--prefix" => a.claude.prefix = value("--prefix")?,
            "--state" => a.state = Some(PathBuf::from(value("--state")?)),
            "--dry-run" => a.dry_run = true,
            "--once" => a.once = true,
            other => return Err(format!("unknown option `{other}`")),
        }
    }
    Ok(a)
}

fn default_state_path(endpoint: &str) -> Option<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(
        PathBuf::from(home)
            .join(".clawdot-bridge")
            .join(format!("{endpoint}.handled")),
    )
}

/// The bridge's token: from the file when one is named, else from `CLAWDOT_TOKEN`.
fn read_token(token_file: Option<&Path>) -> Result<String, String> {
    match (token_file, std::env::var("CLAWDOT_TOKEN")) {
        (Some(path), _) => std::fs::read_to_string(path)
            .map(|s| s.trim().to_owned())
            .map_err(|e| format!("cannot read {}: {e}", path.display())),
        (None, Ok(t)) if !t.trim().is_empty() => Ok(t.trim().to_owned()),
        _ => Err("set CLAWDOT_TOKEN or pass --token-file".to_owned()),
    }
}

/// The record of handled events: loaded from `state_path` when there is one, else (or when it cannot be
/// read) kept in memory only.
fn open_state(state_path: Option<&Path>) -> Handled {
    match state_path {
        Some(p) => {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            Handled::load(p).unwrap_or_else(|e| {
                eprintln!(
                    "clawdot-bridge: cannot read state {}: {e}; continuing without it",
                    p.display()
                );
                Handled::in_memory()
            })
        }
        None => Handled::in_memory(),
    }
}

/// Follow the event stream, reconnecting with a growing pause, until `once` is satisfied or the token is
/// refused.
fn stream_forever(
    cfg: &Config,
    handled: &mut Handled,
    runner: &mut dyn Runner,
    once: bool,
) -> ExitCode {
    let mut backoff = Duration::from_secs(1);
    loop {
        match stream_once(cfg, handled, runner, None) {
            Ok(_) => {
                backoff = Duration::from_secs(1);
                if once {
                    return ExitCode::SUCCESS;
                }
                eprintln!("clawdot-bridge: the stream closed; reconnecting");
            }
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                eprintln!("clawdot-bridge: {e}. Check the token (it may have been rotated).");
                return ExitCode::from(3);
            }
            Err(e) => {
                if once {
                    eprintln!("clawdot-bridge: {e}");
                    return ExitCode::from(1);
                }
                eprintln!("clawdot-bridge: {e}; retrying in {}s", backoff.as_secs());
            }
        }
        std::thread::sleep(backoff);
        backoff = (backoff * 2).min(Duration::from_secs(30));
    }
}

fn main() -> ExitCode {
    let args = match parse() {
        Ok(a) => a,
        Err(msg) => {
            if msg.is_empty() {
                print!("{HELP}");
                return ExitCode::SUCCESS;
            }
            eprintln!("clawdot-bridge: {msg}\n\n{HELP}");
            return ExitCode::from(2);
        }
    };
    let Some(url_text) = args.url else {
        eprintln!("clawdot-bridge: --url is required\n\n{HELP}");
        return ExitCode::from(2);
    };
    let url = match Url::parse(&url_text) {
        Ok(u) => u,
        Err(e) => {
            eprintln!("clawdot-bridge: {e}");
            return ExitCode::from(2);
        }
    };
    let token = match read_token(args.token_file.as_deref()) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("clawdot-bridge: {e}");
            return ExitCode::from(2);
        }
    };

    let state_path = args.state.or_else(|| default_state_path(&args.endpoint));
    let mut handled = open_state(state_path.as_deref());

    let cfg = Config {
        url,
        endpoint: args.endpoint,
        token,
        read_timeout: Duration::from_secs(45),
        harness: Box::new(args.claude),
    };
    let mut runner: Box<dyn Runner> = if args.dry_run {
        Box::new(DryRunner)
    } else {
        Box::new(ProcessRunner)
    };

    println!(
        "clawdot-bridge: listening to {} as `{}`{}",
        url_text,
        cfg.endpoint,
        if args.dry_run { " (dry run)" } else { "" }
    );
    stream_forever(&cfg, &mut handled, runner.as_mut(), args.once)
}
