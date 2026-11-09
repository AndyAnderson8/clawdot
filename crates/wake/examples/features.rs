//! Print the frontend features for a raw 16 kHz mono signed 16-bit little-endian file, one frame per line
//! (40 unsigned integers). Used to compare against the reference during validation (spec 0007 REQ-WAK-011).
//!
//! `cargo run -p clawdot-wake --example features -- clip.s16`

use clawdot_wake::Frontend;

fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: features <file.s16>");
        std::process::exit(2);
    };
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("cannot read {path}: {e}");
            std::process::exit(1);
        }
    };
    let pcm: Vec<i16> = bytes
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect();
    let mut fe = Frontend::new();
    fe.process(&pcm, |f, _| {
        let line: Vec<String> = f.iter().map(u16::to_string).collect();
        println!("{}", line.join(" "));
    });
}
