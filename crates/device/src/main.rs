//! `clawdot-device`: run the device daemon, or check a board file.

use clawdot_device::board::{self, EMBEDDED};
use clawdot_device::caps::Declared;
use clawdot_device::daemon::{DaemonConfig, DaemonDevice, real_sleep, run_forever};
use clawdot_device::quiet::{QuietHours, QuietPolicy};
use clawdot_satellite::{Config, DEAD_AFTER};
use std::net::TcpStream;
use std::process::ExitCode;

const HELP: &str = "\
clawdot-device: the ClawDot device daemon

USAGE:
    clawdot-device --board NAME|FILE.json --core HOST:PORT [options]
    clawdot-device --board NAME|FILE.json --check

A board is `none`, one of the embedded boards (desktop, echo-dot2, raspberry-pi), or the path of a board file
(see schemas/board/). A board file with an unknown key or an impossible value stops startup and names the key.

OPTIONS:
    --check                   load and validate the board, print what it declares, and exit
    --core HOST:PORT          the ClawDot core's satellite address
    --token-env VAR           environment variable holding the shared token
    --id NAME                 device id (default: the board name)
    --quiet HH:MM-HH:MM       quiet hours: cap the volume and dim the LEDs in this daily window
        --quiet-volume N      volume ceiling in quiet hours, percent (default 20)
        --utc-offset-min N    local time as minutes from UTC, for --quiet (default 0)
    --turns N                 leave after N replies (development)
";

#[derive(Default)]
struct Opts {
    board: Option<String>,
    core: Option<String>,
    token_env: Option<String>,
    id: Option<String>,
    check: bool,
    quiet: Option<String>,
    quiet_volume: Option<u8>,
    utc_offset_min: i32,
    turns: Option<usize>,
}

fn parse(args: impl Iterator<Item = String>) -> Result<Option<Opts>, String> {
    let mut o = Opts::default();
    let mut args = args;
    while let Some(a) = args.next() {
        let mut val = |n: &str| args.next().ok_or_else(|| format!("{n} needs a value"));
        match a.as_str() {
            "-h" | "--help" => return Ok(None),
            "--board" => o.board = Some(val("--board")?),
            "--core" => o.core = Some(val("--core")?),
            "--token-env" => o.token_env = Some(val("--token-env")?),
            "--id" => o.id = Some(val("--id")?),
            "--check" => o.check = true,
            "--quiet" => o.quiet = Some(val("--quiet")?),
            "--quiet-volume" => {
                o.quiet_volume =
                    Some(val("--quiet-volume")?.parse().map_err(|_| {
                        "--quiet-volume needs a whole number from 0 to 100".to_owned()
                    })?);
            }
            "--utc-offset-min" => {
                o.utc_offset_min = val("--utc-offset-min")?
                    .parse()
                    .map_err(|_| "--utc-offset-min needs a whole number of minutes".to_owned())?;
            }
            "--turns" => {
                o.turns = Some(
                    val("--turns")?
                        .parse()
                        .map_err(|_| "--turns needs a whole number".to_owned())?,
                );
            }
            other => return Err(format!("unknown option `{other}`")),
        }
    }
    Ok(Some(o))
}

fn go(o: &Opts) -> Result<(), String> {
    let name = o.board.as_deref().ok_or("--board is needed")?;
    let board = board::load(name)?;
    let declared: Declared = board.declared();
    if o.check {
        println!("board: {}", board.name());
        print!("{}", declared.report());
        return Ok(());
    }
    let core = o.core.as_deref().ok_or("--core is needed")?;
    let token = o
        .token_env
        .as_deref()
        .map(|v| std::env::var(v).map_err(|_| format!("{v} is not set")))
        .transpose()?;
    let quiet = match &o.quiet {
        Some(w) => Some(
            QuietHours::parse(w, o.quiet_volume.unwrap_or(20), 10)
                .map_err(|e| format!("--quiet: {e}"))?,
        ),
        None => None,
    };
    let device_id = o.id.clone().unwrap_or_else(|| board.name().to_owned());
    let parts = board.assemble()?;
    for p in &parts.problems {
        eprintln!("clawdot-device: {p}");
    }
    let mut device = DaemonDevice::new(
        parts,
        DaemonConfig {
            quiet: QuietPolicy::new(quiet),
            utc_offset_min: o.utc_offset_min,
            max_turns: o.turns,
            verbose: true,
        },
    );
    let cfg = Config {
        device_id,
        token,
        wake_on_connect: false,
        dead_after: DEAD_AFTER,
    };
    let core = core.to_owned();
    run_forever(
        || TcpStream::connect(&core).map_err(|e| format!("cannot connect to {core}: {e}")),
        &mut device,
        &cfg,
        None,
        real_sleep,
    )
    .map(|_| ())
}

fn main() -> ExitCode {
    let o = match parse(std::env::args().skip(1)) {
        Ok(Some(o)) => o,
        Ok(None) => {
            print!("{HELP}");
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("clawdot-device: {e}\n\n{HELP}");
            return ExitCode::from(2);
        }
    };
    if o.board.is_none() {
        eprintln!(
            "clawdot-device: --board is needed (none, {})\n\n{HELP}",
            EMBEDDED
                .iter()
                .map(|(n, _)| *n)
                .collect::<Vec<_>>()
                .join(", ")
        );
        return ExitCode::from(2);
    }
    match go(&o) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("clawdot-device: {e}");
            ExitCode::from(1)
        }
    }
}
