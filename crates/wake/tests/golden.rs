//! Frontend agreement with the reference implementation (spec 0007 REQ-WAK-004, REQ-WAK-011).
//!
//! The golden files were produced by `spikes/wakeword/make_golden.py` from pymicro-features (the TensorFlow Lite
//! Micro audio frontend) on audio synthesised by that script (see tests/golden/PROVENANCE.txt). Differences are
//! counted in the units the model sees: one step of the int8 model input is about 2.6 frontend units.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use clawdot_wake::{Frontend, NUM_CHANNELS};
use support::{numbers, pcm, u16s};

const CLIPS: [&str; 4] = ["speech", "tones", "phrase_a", "phrase_b"];

/// Frontend features for a whole clip.
fn features(clip: &[i16]) -> Vec<[u16; NUM_CHANNELS]> {
    let mut fe = Frontend::new();
    let mut rows = Vec::new();
    fe.process(clip, |f, _| rows.push(*f));
    rows
}

/// Quantise the way the engine does (input scale 0.10196, zero point -128).
fn quantised(raw: u16) -> i32 {
    let q =
        (f64::from(raw) * (10.0 / 256.0) / f64::from(0.101_960_79_f32) - 128.0).round_ties_even();
    q.clamp(-128.0, 127.0) as i32
}

struct Agreement {
    values: usize,
    exact: usize,
    within_one_q: usize,
    worst_q: i32,
    worst_raw: i32,
}

impl Agreement {
    fn show(&self, what: &str) {
        println!(
            "{what}: {} values, {:.3}% identical, {:.3}% within 1 input step, worst {} input steps ({} frontend units)",
            self.values,
            100.0 * self.exact as f64 / self.values as f64,
            100.0 * self.within_one_q as f64 / self.values as f64,
            self.worst_q,
            self.worst_raw
        );
    }

    fn identical(&self) -> f64 {
        self.exact as f64 / self.values as f64
    }

    fn within_one(&self) -> f64 {
        self.within_one_q as f64 / self.values as f64
    }
}

fn compare(ours: &[[u16; NUM_CHANNELS]], reference: &[u16]) -> Agreement {
    assert_eq!(
        ours.len() * NUM_CHANNELS,
        reference.len(),
        "frame counts differ"
    );
    let mut a = Agreement {
        values: 0,
        exact: 0,
        within_one_q: 0,
        worst_q: 0,
        worst_raw: 0,
    };
    for (f, r) in ours.iter().flatten().zip(reference.iter()) {
        a.values += 1;
        let dq = (quantised(*f) - quantised(*r)).abs();
        let draw = (i32::from(*f) - i32::from(*r)).abs();
        a.exact += usize::from(draw == 0);
        a.within_one_q += usize::from(dq <= 1);
        a.worst_q = a.worst_q.max(dq);
        a.worst_raw = a.worst_raw.max(draw);
    }
    a
}

#[test]
fn req_wak_011_frontend_agrees_with_the_reference_on_streamed_clips() {
    for clip in CLIPS {
        let a = compare(
            &features(&pcm(&format!("{clip}.s16"))),
            &u16s(&format!("{clip}.features.u16")),
        );
        a.show(clip);
        assert!(a.identical() >= 0.99, "{clip}: identical values below 99 %");
        assert!(
            a.within_one() >= 0.995,
            "{clip}: within one input step below 99.5 %"
        );
    }
}

#[test]
fn req_wak_011_frontend_agrees_with_the_reference_on_independent_frames() {
    // 30 frames of very different level and shape, each from a fresh frontend: no state to carry errors.
    let all = pcm("frames.s16");
    let reference = numbers("frames.features.txt");
    let mut ours = Vec::new();
    let mut theirs = Vec::new();
    for (k, chunk) in all.chunks(480).enumerate() {
        let f = features(chunk);
        assert_eq!(f.len(), 1);
        ours.push(f[0]);
        theirs.extend(reference[k].iter().map(|v| *v as u16));
    }
    let a = compare(&ours, &theirs);
    a.show("frames");
    assert!(a.identical() >= 0.99);
    assert!(a.within_one() >= 0.999);
}

#[test]
fn req_wak_002_the_frontend_takes_any_samples_without_panicking() {
    // Full-scale square waves, all-minimum samples, alternating extremes and silence.
    let mut fe = Frontend::new();
    let patterns: [Vec<i16>; 4] = [
        vec![i16::MIN; 4000],
        vec![i16::MAX; 4000],
        (0..4000)
            .map(|i| if i % 2 == 0 { i16::MAX } else { i16::MIN })
            .collect(),
        vec![0; 4000],
    ];
    for p in &patterns {
        let mut n = 0;
        fe.process(p, |f, _| {
            n += 1;
            assert!(f.iter().all(|&v| v < 2000));
        });
        assert!(n > 0);
    }
}
