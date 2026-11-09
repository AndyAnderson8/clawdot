//! `clawdot-device`: run the device daemon, or check a board file.

use clawdot_device::board::{self, EMBEDDED};
use clawdot_device::caps::{Build, Declared};
use clawdot_device::daemon::{DaemonConfig, DaemonDevice, real_sleep, run_forever};
use clawdot_device::quiet::{QuietHours, QuietPolicy};
use clawdot_satellite::front::WakeSink;
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

PC VOICE LOOP (spec 0006 REQ-DEV-025..030):
    --mic-device NAME         microphone name for the board's capture program (ffmpeg on Windows needs one)
    --speaker-device NAME     speaker name for the board's playback program
    --always-on               keep the microphone open between turns (implied by --wake-model)
    --wake-model FILE.tflite  listen for this wake word (repeat for several); FILE.json must sit beside it.
                              Needs a build with `--features mww`.
    --wake-licence TEXT       recorded licence of the models (default Apache-2.0: only the official
                              microWakeWord models; give the real licence for any other model)
    --wake-sensitivity [WORD=]0..1   0 strict, 0.5 as published, 1 loose; WORD is the model's file name without
                              extension; without WORD it applies to every word (repeat)
    --wake-refractory-ms N    ignore a word for N ms after it fired (default 2000)
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
    mic_device: Option<String>,
    speaker_device: Option<String>,
    always_on: bool,
    wake_models: Vec<String>,
    wake_licence: Option<String>,
    wake_sensitivity: Vec<String>,
    wake_refractory_ms: Option<u64>,
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
            "--mic-device" => o.mic_device = Some(val("--mic-device")?),
            "--speaker-device" => o.speaker_device = Some(val("--speaker-device")?),
            "--always-on" => o.always_on = true,
            "--wake-model" => o.wake_models.push(val("--wake-model")?),
            "--wake-licence" => o.wake_licence = Some(val("--wake-licence")?),
            "--wake-sensitivity" => o.wake_sensitivity.push(val("--wake-sensitivity")?),
            "--wake-refractory-ms" => {
                o.wake_refractory_ms = Some(
                    val("--wake-refractory-ms")?
                        .parse()
                        .map_err(|_| "--wake-refractory-ms needs a whole number".to_owned())?,
                );
            }
            other => return Err(format!("unknown option `{other}`")),
        }
    }
    Ok(Some(o))
}

#[cfg(feature = "mww")]
fn wake_sink(o: &Opts) -> Result<Option<Box<dyn WakeSink>>, String> {
    use clawdot_device::wake::{WakeEngine, WordId};
    use clawdot_device::wake_mww::{MwwEngine, MwwSink, load_word};
    use std::path::Path;

    if o.wake_models.is_empty() {
        return Ok(None);
    }
    let licence = o.wake_licence.as_deref().unwrap_or("Apache-2.0");
    let mut words = Vec::new();
    let mut stems = Vec::new();
    for (i, m) in o.wake_models.iter().enumerate() {
        let path = Path::new(m);
        let id = WordId(u16::try_from(i + 1).map_err(|_| "too many wake word models".to_owned())?);
        words.push(load_word(id, path, licence)?);
        stems.push(
            path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_lowercase(),
        );
    }
    let mut engine = MwwEngine::new(words).map_err(|e| format!("wake engine: {e}"))?;
    if let Some(ms) = o.wake_refractory_ms {
        engine = engine.with_refractory(ms.saturating_mul(16));
    }
    for s in &o.wake_sensitivity {
        let (word, value) = s
            .split_once('=')
            .map_or((None, s.as_str()), |(w, v)| (Some(w), v));
        let v: f32 = value
            .parse()
            .ok()
            .filter(|v| (0.0..=1.0).contains(v))
            .ok_or_else(|| format!("--wake-sensitivity {s}: needs a number from 0 to 1"))?;
        let targets: Vec<usize> = match word {
            None => (0..stems.len()).collect(),
            Some(w) => vec![
                stems
                    .iter()
                    .position(|x| *x == w.to_lowercase())
                    .ok_or_else(|| format!("--wake-sensitivity {s}: no model is named `{w}`"))?,
            ],
        };
        for i in targets {
            engine.set_sensitivity(WordId(u16::try_from(i + 1).unwrap_or(u16::MAX)), v);
        }
    }
    for w in engine.words() {
        let hash: String = w.model_sha256.iter().map(|b| format!("{b:02x}")).collect();
        eprintln!(
            "wake word: \"{}\" (model sha256 {}, licence {})",
            w.phrase,
            &hash[..16],
            w.licence
        );
    }
    eprintln!(
        "wake engine: {} KB of working memory",
        engine.memory_bytes() / 1024
    );
    Ok(Some(Box::new(MwwSink::new(engine))))
}

#[cfg(not(feature = "mww"))]
fn wake_sink(o: &Opts) -> Result<Option<Box<dyn WakeSink>>, String> {
    if o.wake_models.is_empty() {
        Ok(None)
    } else {
        Err("--wake-model: this build has no wake engine; build with `--features mww`".into())
    }
}

fn go(o: &Opts) -> Result<(), String> {
    let name = o.board.as_deref().ok_or("--board is needed")?;
    let wake = if o.check { None } else { wake_sink(o)? };
    let build = Build {
        wake_engine: wake.is_some(),
        ..Build::CURRENT
    };
    let board = board::load_tuned(
        name,
        |f| {
            if let Some(d) = &o.mic_device {
                f.audio.capture.device = Some(d.clone());
            }
            if let Some(d) = &o.speaker_device {
                f.audio.playback.device = Some(d.clone());
            }
        },
        build,
    )?;
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
            always_on: o.always_on,
            wake,
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
