//! `clawdot-satellite`: a ClawDot satellite for a desktop.
//!
//! Two uses. With `--in` and no `--ptt`/`--live` it says a WAV file to a core and saves the spoken reply (a
//! development satellite and conformance driver). With `--ptt` or `--live` it runs real turns: audio from the
//! microphone (or a WAV file) and the reply to the speaker (or a WAV file) through the audio interface. Other
//! devices implement `clawdot_satellite::Device` instead.

use clawdot_satellite::audio::{AudioIn, AudioOut, WavIn, WavOut, encode_wav, parse_wav};
use clawdot_satellite::command::{
    CommandIn, CommandOut, DEFAULT_STALL, custom, default_capture, default_playback,
};
use clawdot_satellite::front::{AudioDevice, DeviceConfig, Immediate, LineTrigger, Trigger};
use clawdot_satellite::{Config, FileDevice, run};
use std::net::TcpStream;
use std::process::ExitCode;
use std::time::Duration;

const HELP: &str = "\
clawdot-satellite: a ClawDot satellite for a desktop

USAGE:
    clawdot-satellite --core HOST:PORT --in question.wav [--out answer.wav] [--token-env VAR] [--id NAME]
    clawdot-satellite --core HOST:PORT --ptt  [audio options]     push-to-talk: press Enter to talk, q to quit
    clawdot-satellite --core HOST:PORT --live [audio options]     one spoken turn, starting now

Recordings must be mono 16-bit PCM at 16000 Hz. The core must have a `satellite` section configured.

AUDIO OPTIONS (with --ptt or --live; --in and --out then replace the microphone and the speaker):
    --mic-device NAME         microphone name for the discovered program (required by ffmpeg on Windows)
    --speaker-device NAME     speaker name for the discovered program
    --mic-cmd PROGRAM         use this capture program instead; it must write raw 16 kHz mono s16le to stdout
        --mic-arg ARG         an argument for it (repeat for several; no shell is involved)
    --speaker-cmd PROGRAM     use this playback program; it must read raw 16 kHz mono s16le from stdin
        --speaker-arg ARG     an argument for it (repeat)
        --speaker-wav         send a WAV header before the samples (for players such as ffplay)
    --turns N                 leave after N replies (default: --live 1, --ptt unlimited)
    --stall-secs N            treat a silent program as stalled after N seconds (default 5)
";

#[derive(Default)]
struct Opts {
    core: Option<String>,
    input: Option<String>,
    output: Option<String>,
    token_env: Option<String>,
    id: Option<String>,
    ptt: bool,
    live: bool,
    mic_device: Option<String>,
    speaker_device: Option<String>,
    mic_cmd: Option<String>,
    mic_args: Vec<String>,
    speaker_cmd: Option<String>,
    speaker_args: Vec<String>,
    speaker_wav: bool,
    turns: Option<usize>,
    stall: Option<u64>,
}

fn parse(args: impl Iterator<Item = String>) -> Result<Option<Opts>, String> {
    let mut o = Opts::default();
    let mut args = args;
    while let Some(a) = args.next() {
        let mut val = |n: &str| args.next().ok_or_else(|| format!("{n} needs a value"));
        match a.as_str() {
            "-h" | "--help" => return Ok(None),
            "--core" => o.core = Some(val("--core")?),
            "--in" => o.input = Some(val("--in")?),
            "--out" => o.output = Some(val("--out")?),
            "--token-env" => o.token_env = Some(val("--token-env")?),
            "--id" => o.id = Some(val("--id")?),
            "--ptt" => o.ptt = true,
            "--live" => o.live = true,
            "--mic-device" => o.mic_device = Some(val("--mic-device")?),
            "--speaker-device" => o.speaker_device = Some(val("--speaker-device")?),
            "--mic-cmd" => o.mic_cmd = Some(val("--mic-cmd")?),
            "--mic-arg" => o.mic_args.push(val("--mic-arg")?),
            "--speaker-cmd" => o.speaker_cmd = Some(val("--speaker-cmd")?),
            "--speaker-arg" => o.speaker_args.push(val("--speaker-arg")?),
            "--speaker-wav" => o.speaker_wav = true,
            "--turns" => {
                let n: usize = val("--turns")?
                    .parse()
                    .map_err(|_| "--turns needs a whole number".to_owned())?;
                o.turns = Some(n);
            }
            "--stall-secs" => {
                let n: u64 = val("--stall-secs")?
                    .parse()
                    .map_err(|_| "--stall-secs needs a whole number".to_owned())?;
                o.stall = Some(n.max(1));
            }
            other => return Err(format!("unknown option `{other}`")),
        }
    }
    if o.ptt && o.live {
        return Err("choose either --ptt or --live".into());
    }
    if (o.mic_cmd.is_none() && !o.mic_args.is_empty())
        || (o.speaker_cmd.is_none() && !o.speaker_args.is_empty())
    {
        return Err("--mic-arg and --speaker-arg need --mic-cmd and --speaker-cmd".into());
    }
    Ok(Some(o))
}

fn connect(o: &Opts) -> Result<(TcpStream, Config), String> {
    let core = o.core.as_deref().ok_or("--core is needed")?;
    let token = o
        .token_env
        .as_deref()
        .map(|v| std::env::var(v).map_err(|_| format!("{v} is not set")))
        .transpose()?;
    let sock = TcpStream::connect(core).map_err(|e| format!("cannot connect to {core}: {e}"))?;
    let cfg = Config {
        device_id: o.id.clone().unwrap_or_else(|| "desktop".to_owned()),
        token,
        wake_on_connect: !(o.ptt || o.live),
        dead_after: clawdot_satellite::DEAD_AFTER,
    };
    Ok((sock, cfg))
}

fn file_mode(o: &Opts) -> Result<(), String> {
    let input = o.input.as_deref().ok_or("--in is needed")?;
    let pcm = parse_wav(&std::fs::read(input).map_err(|e| format!("cannot read {input}: {e}"))?)
        .map_err(|e| e.to_string())?;
    let (sock, cfg) = connect(o)?;
    let mut dev = FileDevice::new(pcm);
    let report = run(sock, &mut dev, &cfg)?;
    println!("states: {:?}", report.indicators);
    println!("reply: {} samples", dev.reply.len());
    if let Some(out) = &o.output {
        std::fs::write(out, encode_wav(&dev.reply))
            .map_err(|e| format!("cannot write {out}: {e}"))?;
        println!("saved to {out}");
    }
    Ok(())
}

fn audio_mode(o: &Opts) -> Result<(), String> {
    let stall = o.stall.map_or(DEFAULT_STALL, Duration::from_secs);
    let input: Box<dyn AudioIn> = if let Some(path) = &o.input {
        Box::new(WavIn::open(path.as_ref(), 1_500).map_err(|e| e.to_string())?)
    } else {
        let spec = match &o.mic_cmd {
            Some(p) => custom(p, &o.mic_args, false),
            None => default_capture(o.mic_device.as_deref()).map_err(|e| e.to_string())?,
        };
        eprintln!("microphone: {}", spec.program());
        Box::new(CommandIn::new(spec, stall))
    };
    let output: Box<dyn AudioOut> = if let Some(path) = &o.output {
        Box::new(WavOut::to_file(path))
    } else {
        let spec = match &o.speaker_cmd {
            Some(p) => custom(p, &o.speaker_args, o.speaker_wav),
            None => default_playback(o.speaker_device.as_deref()).map_err(|e| e.to_string())?,
        };
        eprintln!("speaker: {}", spec.program());
        Box::new(CommandOut::new(spec, stall))
    };
    let trigger: Box<dyn Trigger> = if o.ptt {
        Box::new(LineTrigger::stdin())
    } else {
        Box::<Immediate>::default()
    };
    let cfg = DeviceConfig {
        max_turns: o.turns.or(if o.live { Some(1) } else { None }),
        verbose: true,
        ..DeviceConfig::default()
    };
    let (sock, ccfg) = connect(o)?;
    let mut dev = AudioDevice::new(input, output, trigger, cfg);
    if o.ptt {
        eprintln!("push-to-talk: press Enter to talk, then speak; q then Enter to quit");
    }
    run(sock, &mut dev, &ccfg)?;
    Ok(())
}

fn main() -> ExitCode {
    let o = match parse(std::env::args().skip(1)) {
        Ok(Some(o)) => o,
        Ok(None) => {
            print!("{HELP}");
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("clawdot-satellite: {e}\n\n{HELP}");
            return ExitCode::from(2);
        }
    };
    if o.core.is_none() || (!o.ptt && !o.live && o.input.is_none()) {
        eprint!("{HELP}");
        return ExitCode::from(2);
    }
    let r = if o.ptt || o.live {
        audio_mode(&o)
    } else {
        file_mode(&o)
    };
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("clawdot-satellite: {e}");
            ExitCode::from(1)
        }
    }
}
