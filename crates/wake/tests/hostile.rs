//! A model file is untrusted input (spec 0007 REQ-WAK-002): damaged and forged files must give an error or a
//! working interpreter, never a panic, a hang or a large allocation.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use clawdot_wake::{Engine, Interpreter, Model, WordConfig};
use support::{Rng, model};

const MODELS: [&str; 2] = ["okay_nabu", "hey_mycroft"];

/// Load, and if that works run two inferences and a reset.
fn exercise(bytes: Vec<u8>) -> bool {
    match Interpreter::new(bytes) {
        Ok(mut i) => {
            let input = vec![-37i8; i.input_len()];
            let _ = i.invoke(&input);
            let _ = i.invoke(&input);
            i.reset();
            let _ = i.output_value();
            true
        }
        Err(_) => false,
    }
}

#[test]
fn req_wak_002_the_unmodified_models_load() {
    for name in MODELS {
        assert!(exercise(model(name)), "{name} must load");
    }
}

#[test]
fn req_wak_002_every_truncation_is_refused_or_runs() {
    for name in MODELS {
        let full = model(name);
        for cut in (0..full.len()).step_by(5) {
            let _ = exercise(full[..cut].to_vec());
        }
        for cut in full.len().saturating_sub(64)..full.len() {
            let _ = exercise(full[..cut].to_vec());
        }
    }
}

#[test]
fn req_wak_002_random_byte_damage_never_panics() {
    let mut rng = Rng(0x00c0_ffee_1234_5678);
    for name in MODELS {
        let full = model(name);
        let (mut loaded, mut refused) = (0, 0);
        for round in 0..4000 {
            let mut b = full.clone();
            let hits = 1 + rng.below(4);
            for _ in 0..hits {
                // Half the time damage the structure (the first 4 KiB holds the tables); half anywhere.
                let at = if round % 2 == 0 {
                    rng.below(b.len().min(4096))
                } else {
                    rng.below(b.len())
                };
                b[at] = match rng.below(6) {
                    0 => 0,
                    1 => 0xff,
                    2 => 0x7f,
                    3 => 0x80,
                    4 => b[at].wrapping_add(1),
                    _ => (rng.next() & 0xff) as u8,
                };
            }
            if exercise(b) {
                loaded += 1;
            } else {
                refused += 1;
            }
        }
        println!("{name}: {loaded} damaged files still loaded, {refused} refused");
        assert!(refused > 0, "damage should be noticed at least sometimes");
    }
}

#[test]
fn req_wak_002_damage_to_whole_words_never_panics() {
    // Overwrite aligned 32-bit words with extreme values: offsets, lengths, dimensions and indices.
    let mut rng = Rng(0x0bad_cafe_dead_beef);
    let full = model("okay_nabu");
    for _ in 0..4000 {
        let mut b = full.clone();
        let at = rng.below(b.len().min(8192) / 4) * 4;
        let v: u32 = match rng.below(7) {
            0 => 0,
            1 => u32::MAX,
            2 => 0x8000_0000,
            3 => 0x7fff_ffff,
            4 => 1 << 20,
            5 => rng.below(300) as u32,
            _ => (rng.next() >> 32) as u32,
        };
        b[at..at + 4].copy_from_slice(&v.to_le_bytes());
        let _ = exercise(b);
    }
}

#[test]
fn req_wak_002_garbage_and_oversize_input_is_refused() {
    assert!(Interpreter::new(Vec::new()).is_err());
    assert!(Interpreter::new(vec![0; 3]).is_err());
    assert!(Interpreter::new(vec![0xff; 1000]).is_err());
    let mut rng = Rng(7);
    for len in [8usize, 64, 1000, 5000] {
        let b: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
        assert!(Interpreter::new(b).is_err());
    }
    let mut big = model("okay_nabu");
    big.resize(1 << 20 | 1, 0);
    assert!(matches!(
        Model::parse(big),
        Err(clawdot_wake::Error::Limit(_))
    ));
    assert!(Engine::new(vec![WordConfig::new(vec![1, 2, 3], 0.9, 5)]).is_err());
}

#[test]
fn req_wak_001_an_operator_outside_the_supported_set_is_named() {
    // Damage the tables near the start of the file until the operator-code table points at an operator outside the
    // supported set; the refusal must carry the reason.
    let mut b = model("hey_mycroft");
    let tflite = b.clone();
    let mut rng = Rng(99);
    let mut named = false;
    for _ in 0..4000 {
        b.copy_from_slice(&tflite);
        let at = rng.below(b.len().min(2048));
        b[at] = b[at].wrapping_add(1 + rng.below(40) as u8);
        if let Err(clawdot_wake::Error::Unsupported(m)) = Interpreter::new(b.clone()) {
            assert!(!m.is_empty());
            named = true;
            break;
        }
    }
    assert!(
        named,
        "no damaged file produced an 'unsupported' refusal in 4000 tries"
    );
}
