//! Stream a WAV file (or raw 16 kHz mono signed 16-bit little-endian samples, `.s16`) through the microWakeWord
//! engine in 10 ms frames and print the detections (spec 0007).
//!
//! ```text
//! cargo run -p clawdot-device --features mww --release --example wake_wav -- \
//!     --model okay_nabu.tflite --manifest okay_nabu.json --licence Apache-2.0 clip.wav
//! ```
//!
//! The model and its manifest come from <https://github.com/esphome/micro-wake-word-models> (Apache-2.0, `models/v2`);
//! the repository keeps two of them for its tests in `crates/wake/tests/golden/models`. Nothing is read except the
//! files named, and nothing is written.

use clawdot_device::wake::{Detection, WakeEngine, WordId};
use clawdot_device::wake_mww::{MwwEngine, MwwWord};
use std::time::Instant;

const HELP: &str = "\
wake_wav: run a clip through the microWakeWord engine

USAGE:
    wake_wav --model FILE.tflite --manifest FILE.json --licence TEXT [--sensitivity 0..1] [--repeat N] CLIP

CLIP is a 16 kHz mono 16-bit PCM WAV file, or raw samples in a file ending in .s16.
";

struct Opts {
    model: String,
    manifest: String,
    licence: String,
    sensitivity: Option<f32>,
    repeat: usize,
    clip: String,
}

fn parse() -> Result<Opts, String> {
    let mut args = std::env::args().skip(1);
    let (mut model, mut manifest, mut licence, mut sensitivity, mut repeat, mut clip) =
        (None, None, None, None, 1usize, None);
    while let Some(a) = args.next() {
        let mut value = |what: &str| args.next().ok_or(format!("{what} needs a value"));
        match a.as_str() {
            "--model" => model = Some(value("--model")?),
            "--manifest" => manifest = Some(value("--manifest")?),
            "--licence" => licence = Some(value("--licence")?),
            "--sensitivity" => {
                sensitivity = Some(
                    value("--sensitivity")?
                        .parse::<f32>()
                        .map_err(|e| format!("--sensitivity: {e}"))?,
                );
            }
            "--repeat" => {
                repeat = value("--repeat")?
                    .parse::<usize>()
                    .map_err(|e| format!("--repeat: {e}"))?
                    .max(1);
            }
            "-h" | "--help" => return Err(String::new()),
            s if s.starts_with("--") => return Err(format!("unknown option {s}")),
            _ => clip = Some(a),
        }
    }
    Ok(Opts {
        model: model.ok_or("--model is required")?,
        manifest: manifest.ok_or("--manifest is required")?,
        licence: licence
            .ok_or("--licence is required (REQ-VOI-009: record where the model came from)")?,
        sensitivity,
        repeat,
        clip: clip.ok_or("a clip file is required")?,
    })
}

/// 16-bit mono 16 kHz samples from a RIFF/WAVE file; anything else is refused with a reason.
fn read_wav(bytes: &[u8]) -> Result<Vec<i16>, String> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a RIFF/WAVE file".into());
    }
    let mut pos = 12;
    let mut format_ok = false;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes([
            bytes[pos + 4],
            bytes[pos + 5],
            bytes[pos + 6],
            bytes[pos + 7],
        ]) as usize;
        let body = pos + 8;
        let end = body.saturating_add(size).min(bytes.len());
        if id == b"fmt " && end - body >= 16 {
            let f = &bytes[body..end];
            let tag = u16::from_le_bytes([f[0], f[1]]);
            let channels = u16::from_le_bytes([f[2], f[3]]);
            let rate = u32::from_le_bytes([f[4], f[5], f[6], f[7]]);
            let bits = u16::from_le_bytes([f[14], f[15]]);
            if tag != 1 || channels != 1 || rate != 16_000 || bits != 16 {
                return Err(format!(
                    "need 16 kHz mono 16-bit PCM, found tag {tag}, {channels} channel(s), {rate} Hz, {bits} bits"
                ));
            }
            format_ok = true;
        } else if id == b"data" {
            if !format_ok {
                return Err("data chunk before the fmt chunk".into());
            }
            return Ok(bytes[body..end]
                .chunks_exact(2)
                .map(|c| i16::from_le_bytes([c[0], c[1]]))
                .collect());
        }
        pos = body.saturating_add(size).saturating_add(size & 1);
    }
    Err("no data chunk".into())
}

/// Peak resident set size in kilobytes, where the system reports it.
fn peak_rss_kb() -> Option<u64> {
    let s = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = s.lines().find(|l| l.starts_with("VmHWM:"))?;
    line.split_whitespace().nth(1)?.parse().ok()
}

fn run(o: &Opts) -> Result<(), String> {
    let model = std::fs::read(&o.model).map_err(|e| format!("{}: {e}", o.model))?;
    let manifest =
        std::fs::read_to_string(&o.manifest).map_err(|e| format!("{}: {e}", o.manifest))?;
    let raw = std::fs::read(&o.clip).map_err(|e| format!("{}: {e}", o.clip))?;
    let pcm = if o.clip.ends_with(".s16") {
        raw.chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect()
    } else {
        read_wav(&raw)?
    };

    let word = MwwWord::from_manifest(WordId(1), &manifest, model, &o.licence)?;
    let rss_before = peak_rss_kb();
    let mut engine = MwwEngine::new(vec![word])?;
    if let Some(s) = o.sensitivity {
        engine.set_sensitivity(WordId(1), s);
    }
    let info = engine.words()[0].clone();
    let hash: String = info
        .model_sha256
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    println!(
        "word \"{}\"  licence {}  model sha256 {hash}  cutoff {:.3}  working memory {} bytes",
        info.phrase,
        info.licence,
        engine.cutoff(info.id).unwrap_or(0.0),
        engine.memory_bytes()
    );

    let seconds = pcm.len() as f64 / 16_000.0;
    let mut detections: Vec<Detection> = Vec::new();
    let started = Instant::now();
    for _ in 0..o.repeat {
        engine.reset();
        detections.clear();
        for frame in pcm.chunks(160) {
            engine.process(frame, &mut detections);
        }
    }
    let spent = started.elapsed().as_secs_f64();
    for d in &detections {
        println!(
            "detected \"{}\" score {:.3} at {:.2} s",
            info.phrase,
            d.score,
            d.end_sample as f64 / 16_000.0
        );
    }
    if detections.is_empty() {
        println!("no detection");
    }
    let audio = seconds * o.repeat as f64;
    println!(
        "{audio:.1} s of audio in {spent:.3} s wall: {:.3}% of one core, {:.0}x real time",
        100.0 * spent / audio.max(1e-9),
        audio / spent.max(1e-9)
    );
    if let (Some(before), Some(after)) = (rss_before, peak_rss_kb()) {
        println!(
            "peak resident memory {after} kB (it was {before} kB before the engine was built)"
        );
    }
    Ok(())
}

fn main() {
    match parse() {
        Ok(o) => {
            if let Err(e) = run(&o) {
                eprintln!("wake_wav: {e}");
                std::process::exit(1);
            }
        }
        Err(e) if e.is_empty() => print!("{HELP}"),
        Err(e) => {
            eprintln!("wake_wav: {e}\n\n{HELP}");
            std::process::exit(2);
        }
    }
}
