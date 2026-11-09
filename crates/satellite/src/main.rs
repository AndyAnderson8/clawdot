//! `clawdot-satellite`: say a WAV file to a ClawDot core and save the spoken reply.
//!
//! A development satellite and a conformance driver: it connects, wakes the assistant, streams the recording
//! as microphone audio, plays the reply into a file, and exits. Real devices implement
//! `clawdot_satellite::Device` instead.

use clawdot_satellite::{Config, FileDevice, run};
use std::net::TcpStream;
use std::process::ExitCode;

const HELP: &str = "\
clawdot-satellite: say a recording to a ClawDot core and save the reply

USAGE:
    clawdot-satellite --core HOST:PORT --in question.wav [--out answer.wav] [--token-env VAR] [--id NAME]

The recording must be mono 16-bit PCM at 16000 Hz. The core must have a `satellite` section configured.
";

/// Read a WAV file in the protocol's uplink format: mono 16-bit PCM at 16 kHz.
fn read_wav(bytes: &[u8]) -> Result<Vec<i16>, String> {
    if bytes.len() < 44 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a WAV file".into());
    }
    let mut pos = 12;
    let mut ok = false;
    while pos + 8 <= bytes.len() {
        let len = u32::from_le_bytes([
            bytes[pos + 4],
            bytes[pos + 5],
            bytes[pos + 6],
            bytes[pos + 7],
        ]) as usize;
        let body = pos + 8;
        let end = body.saturating_add(len).min(bytes.len());
        match &bytes[pos..pos + 4] {
            b"fmt " if end - body >= 16 => {
                let f = &bytes[body..end];
                let (tag, ch) = (
                    u16::from_le_bytes([f[0], f[1]]),
                    u16::from_le_bytes([f[2], f[3]]),
                );
                let rate = u32::from_le_bytes([f[4], f[5], f[6], f[7]]);
                let bits = u16::from_le_bytes([f[14], f[15]]);
                if (tag, ch, rate, bits) != (1, 1, 16_000, 16) {
                    return Err("the recording must be mono 16-bit PCM at 16000 Hz".into());
                }
                ok = true;
            }
            b"data" if ok => {
                return Ok(bytes[body..end]
                    .chunks_exact(2)
                    .map(|b| i16::from_le_bytes([b[0], b[1]]))
                    .collect());
            }
            _ => {}
        }
        pos = body.saturating_add(len).saturating_add(len & 1);
    }
    Err("no audio data in the WAV file".into())
}

fn write_wav(pcm: &[i16]) -> Vec<u8> {
    let data = u32::try_from(pcm.len() * 2).unwrap_or(0);
    let mut out = Vec::with_capacity(44 + pcm.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&16_000u32.to_le_bytes());
    out.extend_from_slice(&32_000u32.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for s in pcm {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

fn main() -> ExitCode {
    let (mut core, mut input, mut output, mut token_env) = (None, None, None, None);
    let mut id = "desktop".to_owned();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut val = |n: &str| args.next().ok_or_else(|| format!("{n} needs a value"));
        let r = match a.as_str() {
            "-h" | "--help" => {
                print!("{HELP}");
                return ExitCode::SUCCESS;
            }
            "--core" => val("--core").map(|v| core = Some(v)),
            "--in" => val("--in").map(|v| input = Some(v)),
            "--out" => val("--out").map(|v| output = Some(v)),
            "--token-env" => val("--token-env").map(|v| token_env = Some(v)),
            "--id" => val("--id").map(|v| id = v),
            other => Err(format!("unknown option `{other}`")),
        };
        if let Err(e) = r {
            eprintln!("clawdot-satellite: {e}\n\n{HELP}");
            return ExitCode::from(2);
        }
    }
    let (Some(core), Some(input)) = (core, input) else {
        eprint!("{HELP}");
        return ExitCode::from(2);
    };
    let go = || -> Result<(), String> {
        let pcm =
            read_wav(&std::fs::read(&input).map_err(|e| format!("cannot read {input}: {e}"))?)?;
        let token = token_env
            .as_deref()
            .map(|v| std::env::var(v).map_err(|_| format!("{v} is not set")))
            .transpose()?;
        let sock =
            TcpStream::connect(&core).map_err(|e| format!("cannot connect to {core}: {e}"))?;
        let mut dev = FileDevice::new(pcm);
        let cfg = Config {
            device_id: id.clone(),
            token,
            wake_on_connect: true,
            dead_after: clawdot_satellite::DEAD_AFTER,
        };
        let report = run(sock, &mut dev, &cfg)?;
        println!("states: {:?}", report.indicators);
        println!("reply: {} samples", dev.reply.len());
        if let Some(out) = &output {
            std::fs::write(out, write_wav(&dev.reply))
                .map_err(|e| format!("cannot write {out}: {e}"))?;
            println!("saved to {out}");
        }
        Ok(())
    };
    match go() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("clawdot-satellite: {e}");
            ExitCode::from(1)
        }
    }
}
