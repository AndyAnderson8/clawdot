//! End-of-utterance detection from raw audio (REQ-VOI-002).
//!
//! A small energy-based endpointer: no model, no dependencies, a few kilobytes of state. It follows the
//! background noise level, decides when speech starts, and ends the utterance after a stretch of silence.
//! Audio is 16 kHz, mono, signed 16-bit (the protocol's default format); chunks of any size may be fed in.
//!
//! It is deliberately simple. Energy detection cannot tell speech from a loud television, so a model-based
//! detector can replace it behind the same [`Endpointer::feed`] interface when measurements call for it.

use std::collections::VecDeque;

/// Samples per second.
pub const SAMPLE_RATE: usize = 16_000;
/// Samples in one analysis frame (20 ms).
const FRAME: usize = SAMPLE_RATE / 50;

/// Tuning, with defaults suited to a quiet room and a close microphone.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Config {
    /// Quietest level (RMS, 0 to 32767) that can count as speech, whatever the background.
    pub min_level: f32,
    /// Speech must be this many times louder than the background noise.
    pub noise_ratio: f32,
    /// Consecutive loud frames needed to start an utterance.
    pub start_frames: usize,
    /// Silence that ends an utterance, in milliseconds.
    pub silence_ms: usize,
    /// Utterances shorter than this are treated as noise, in milliseconds.
    pub min_speech_ms: usize,
    /// Utterances are cut at this length, in milliseconds.
    pub max_utterance_ms: usize,
    /// Give up if nobody speaks within this time, in milliseconds.
    pub no_speech_ms: usize,
    /// Audio kept from before speech was detected, in milliseconds, so the first word is not clipped.
    pub preroll_ms: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            min_level: 400.0,
            noise_ratio: 3.0,
            start_frames: 3,
            silence_ms: 700,
            min_speech_ms: 250,
            max_utterance_ms: 15_000,
            no_speech_ms: 6_000,
            preroll_ms: 300,
        }
    }
}

/// What happened while feeding audio.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Nothing to report yet.
    Nothing,
    /// Speech was detected; the utterance is being collected.
    SpeechStarted,
    /// The utterance is complete.
    Ended {
        /// The audio, including a little before the first word.
        pcm: Vec<i16>,
        /// Whether it was cut at the maximum length rather than ending in silence.
        truncated: bool,
    },
    /// Nobody spoke within the allowed time.
    NoSpeech,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Waiting,
    Speaking,
}

/// Finds utterances in a stream of audio.
#[derive(Debug, Clone)]
pub struct Endpointer {
    cfg: Config,
    state: State,
    partial: Vec<i16>,
    preroll: VecDeque<i16>,
    utterance: Vec<i16>,
    noise: f32,
    loud_run: usize,
    quiet_run: usize,
    waited_frames: usize,
    speech_frames: usize,
}

fn rms(frame: &[i16]) -> f32 {
    if frame.is_empty() {
        return 0.0;
    }
    let sum: f64 = frame.iter().map(|&s| f64::from(s) * f64::from(s)).sum();
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    let r = (sum / frame.len() as f64).sqrt() as f32;
    r
}

fn frames_for(ms: usize) -> usize {
    (ms * SAMPLE_RATE / 1000).div_ceil(FRAME).max(1)
}

impl Endpointer {
    /// A fresh endpointer.
    #[must_use]
    pub fn new(cfg: Config) -> Self {
        Self {
            cfg,
            state: State::Waiting,
            partial: Vec::with_capacity(FRAME),
            preroll: VecDeque::new(),
            utterance: Vec::new(),
            noise: cfg.min_level / cfg.noise_ratio,
            loud_run: 0,
            quiet_run: 0,
            waited_frames: 0,
            speech_frames: 0,
        }
    }

    /// Forget everything and wait for speech again.
    pub fn reset(&mut self) {
        *self = Self::new(self.cfg);
    }

    /// Feed audio. Returns the most significant event it produced; after `Ended` or `NoSpeech` the endpointer
    /// is ready for the next utterance and any audio left in `samples` is discarded.
    pub fn feed(&mut self, samples: &[i16]) -> Event {
        let mut result = Event::Nothing;
        for &s in samples {
            self.partial.push(s);
            if self.partial.len() < FRAME {
                continue;
            }
            let frame = std::mem::take(&mut self.partial);
            match self.frame(&frame) {
                Event::Nothing => {}
                e @ (Event::Ended { .. } | Event::NoSpeech) => {
                    self.reset();
                    return e;
                }
                e => result = e,
            }
        }
        result
    }

    fn threshold(&self) -> f32 {
        (self.noise * self.cfg.noise_ratio).max(self.cfg.min_level)
    }

    fn frame(&mut self, frame: &[i16]) -> Event {
        let level = rms(frame);
        let loud = level >= self.threshold();
        match self.state {
            State::Waiting => self.waiting_frame(frame, level, loud),
            State::Speaking => self.speaking_frame(frame, loud),
        }
    }

    /// One frame while waiting for speech to start.
    fn waiting_frame(&mut self, frame: &[i16], level: f32, loud: bool) -> Event {
        if loud {
            self.loud_run += 1;
        } else {
            // Follow the background slowly so a steady hum does not read as speech.
            self.noise = self.noise * 0.95 + level * 0.05;
            self.loud_run = 0;
        }
        let cap = frames_for(self.cfg.preroll_ms) * FRAME;
        for &s in frame {
            self.preroll.push_back(s);
        }
        while self.preroll.len() > cap + FRAME * self.cfg.start_frames {
            self.preroll.pop_front();
        }
        if self.loud_run >= self.cfg.start_frames {
            self.state = State::Speaking;
            self.utterance = self.preroll.drain(..).collect();
            self.speech_frames = self.loud_run;
            self.quiet_run = 0;
            return Event::SpeechStarted;
        }
        self.waited_frames += 1;
        if self.waited_frames >= frames_for(self.cfg.no_speech_ms) {
            return Event::NoSpeech;
        }
        Event::Nothing
    }

    /// One frame while an utterance is being heard.
    fn speaking_frame(&mut self, frame: &[i16], loud: bool) -> Event {
        self.utterance.extend_from_slice(frame);
        if loud {
            self.speech_frames += 1;
            self.quiet_run = 0;
        } else {
            self.quiet_run += 1;
        }
        let too_long = self.utterance.len() >= self.cfg.max_utterance_ms * SAMPLE_RATE / 1000;
        if self.quiet_run >= frames_for(self.cfg.silence_ms) || too_long {
            if self.speech_frames < frames_for(self.cfg.min_speech_ms) && !too_long {
                // A click or cough: go back to waiting without reporting anything.
                self.state = State::Waiting;
                self.utterance.clear();
                self.loud_run = 0;
                self.speech_frames = 0;
                return Event::Nothing;
            }
            let pcm = std::mem::take(&mut self.utterance);
            return Event::Ended {
                pcm,
                truncated: too_long,
            };
        }
        Event::Nothing
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A tone burst at `amp`, `ms` long.
    pub fn tone(amp: f32, ms: usize) -> Vec<i16> {
        (0..ms * SAMPLE_RATE / 1000)
            .map(|i| {
                #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
                let v = (amp * (i as f32 * 0.35).sin()) as i16;
                v
            })
            .collect()
    }

    pub fn silence(ms: usize) -> Vec<i16> {
        vec![0; ms * SAMPLE_RATE / 1000]
    }

    fn run(e: &mut Endpointer, audio: &[i16]) -> Vec<Event> {
        let mut out = Vec::new();
        for chunk in audio.chunks(160) {
            match e.feed(chunk) {
                Event::Nothing => {}
                ev => out.push(ev),
            }
        }
        out
    }

    #[test]
    fn a_spoken_phrase_is_found_with_a_little_lead_in() {
        let mut e = Endpointer::new(Config::default());
        let mut audio = silence(500);
        audio.extend(tone(3000.0, 900));
        audio.extend(silence(1000));
        let events = run(&mut e, &audio);
        assert_eq!(events[0], Event::SpeechStarted);
        let Event::Ended { pcm, truncated } = &events[1] else {
            panic!("{events:?}")
        };
        assert!(!truncated);
        let ms = pcm.len() * 1000 / SAMPLE_RATE;
        assert!((900..=2000).contains(&ms), "utterance was {ms} ms");
        // The first samples are the quiet lead-in, not clipped speech.
        assert!(pcm[..SAMPLE_RATE / 100].iter().all(|&s| s.abs() < 400));
    }

    #[test]
    fn chunk_size_does_not_change_the_result() {
        let mut audio = silence(300);
        audio.extend(tone(3000.0, 600));
        audio.extend(silence(900));
        let mut results = Vec::new();
        for size in [1, 7, 160, 333, 4000] {
            let mut e = Endpointer::new(Config::default());
            let mut found = None;
            for chunk in audio.chunks(size) {
                if let Event::Ended { pcm, .. } = e.feed(chunk) {
                    found = Some(pcm.len());
                }
            }
            results.push(found.expect("an utterance"));
        }
        assert!(results.windows(2).all(|w| w[0] == w[1]), "{results:?}");
    }

    #[test]
    fn a_pause_inside_a_sentence_does_not_end_it() {
        let mut e = Endpointer::new(Config::default());
        let mut audio = tone(3000.0, 500);
        audio.extend(silence(400));
        audio.extend(tone(3000.0, 500));
        audio.extend(silence(1000));
        let events = run(&mut e, &audio);
        let ended = events
            .iter()
            .filter(|e| matches!(e, Event::Ended { .. }))
            .count();
        assert_eq!(ended, 1, "{events:?}");
    }

    #[test]
    fn a_click_is_ignored() {
        let mut e = Endpointer::new(Config::default());
        let mut audio = silence(300);
        audio.extend(tone(5000.0, 80));
        audio.extend(silence(1500));
        let events = run(&mut e, &audio);
        assert!(
            !events.iter().any(|e| matches!(e, Event::Ended { .. })),
            "{events:?}"
        );
    }

    #[test]
    fn silence_times_out() {
        let mut e = Endpointer::new(Config::default());
        let events = run(&mut e, &silence(7000));
        assert_eq!(events, vec![Event::NoSpeech]);
    }

    #[test]
    fn quiet_noise_never_counts_as_speech() {
        let mut e = Endpointer::new(Config::default());
        let events = run(&mut e, &tone(150.0, 7000));
        assert_eq!(events, vec![Event::NoSpeech]);
    }

    #[test]
    fn endless_speech_is_cut_at_the_maximum() {
        let cfg = Config {
            max_utterance_ms: 2000,
            ..Config::default()
        };
        let mut e = Endpointer::new(cfg);
        let events = run(&mut e, &tone(3000.0, 4000));
        let Some(Event::Ended { pcm, truncated }) =
            events.iter().find(|e| matches!(e, Event::Ended { .. }))
        else {
            panic!("{events:?}")
        };
        assert!(*truncated);
        assert!(pcm.len() <= 2200 * SAMPLE_RATE / 1000, "{}", pcm.len());
    }

    #[test]
    fn it_is_ready_for_the_next_utterance() {
        let mut e = Endpointer::new(Config::default());
        let mut audio = Vec::new();
        for _ in 0..2 {
            audio.extend(silence(400));
            audio.extend(tone(3000.0, 600));
            audio.extend(silence(900));
        }
        let events = run(&mut e, &audio);
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, Event::Ended { .. }))
                .count(),
            2
        );
    }
}
