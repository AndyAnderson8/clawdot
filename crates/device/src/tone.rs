//! Built-in feedback tones (REQ-DEV-014).
//!
//! Short sine beeps generated in code, so feedback never needs the network, a file or the core. Output is 16 kHz
//! mono PCM like all daemon audio.

/// A built-in tone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// Wake word or button acknowledged: a short rising pair.
    Wake,
    /// Something failed: a low double beep.
    Error,
    /// A timer or alarm: three beeps.
    Timer,
    /// Something waits in the inbox: a soft high beep.
    Inbox,
    /// The microphone was muted or unmuted: one low beep.
    Mute,
}

const RATE: usize = 16_000;

/// (frequency Hz, milliseconds) segments; 0 Hz is silence.
fn segments(t: Tone) -> &'static [(u32, usize)] {
    match t {
        Tone::Wake => &[(660, 70), (880, 90)],
        Tone::Error => &[(220, 120), (0, 60), (220, 160)],
        Tone::Timer => &[(1000, 100), (0, 60), (1000, 100), (0, 60), (1000, 100)],
        Tone::Inbox => &[(1320, 120)],
        Tone::Mute => &[(440, 100)],
    }
}

/// The samples of `tone` at 16 kHz, peak about 30 % of full scale, with 5 ms fades so it does not click.
#[must_use]
pub fn samples(tone: Tone) -> Vec<i16> {
    let mut out = Vec::new();
    for &(hz, ms) in segments(tone) {
        let n = RATE * ms / 1000;
        let fade = (RATE * 5 / 1000).max(1);
        for i in 0..n {
            if hz == 0 {
                out.push(0);
                continue;
            }
            let phase = 2.0 * std::f32::consts::PI * hz as f32 * i as f32 / RATE as f32;
            let edge = i.min(n - 1 - i).min(fade) as f32 / fade as f32;
            out.push((phase.sin() * edge * 0.3 * f32::from(i16::MAX)) as i16);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn req_dev_014_tones_are_short_and_nonempty() {
        for t in [
            Tone::Wake,
            Tone::Error,
            Tone::Timer,
            Tone::Inbox,
            Tone::Mute,
        ] {
            let s = samples(t);
            assert!(s.len() > RATE / 20, "{t:?} too short");
            assert!(s.len() < RATE, "{t:?} longer than a second");
            assert!(s.iter().any(|&x| x != 0));
            let peak = s.iter().map(|x| i32::from(*x).abs()).max().unwrap();
            assert!(peak < i32::from(i16::MAX) / 2, "{t:?} too loud");
        }
    }

    #[test]
    fn req_dev_014_tones_start_and_end_silent() {
        let s = samples(Tone::Wake);
        assert_eq!(s[0], 0);
        assert!(i32::from(s[s.len() - 1]).abs() < 2000);
    }
}
