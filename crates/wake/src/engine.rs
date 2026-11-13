//! The detection rule and the multi-word engine (spec 0007 REQ-WAK-005 to REQ-WAK-007).
//!
//! One frontend feeds every word. A word is an interpreter plus the reference decision rule: the model runs once
//! per `stride` features, its outputs go through a sliding window, and a detection is a window mean strictly
//! above the cutoff. A decision exists only once the window is full.

use crate::error::{Error, Result};
use crate::frontend::{Frontend, NUM_CHANNELS};
use crate::interp::Interpreter;
use crate::kernels::Rounding;
use crate::sha256::sha256;

/// Features are the frontend's unsigned values times this, as the reference feeds them to the model
/// (`value * 10 / 256`: the reference quantises 1/25.6 steps).
const FEATURE_SCALE: f64 = 10.0 / 256.0;

/// What the caller supplies for one word.
#[derive(Debug, Clone)]
pub struct WordConfig {
    /// The `.tflite` model file.
    pub model: Vec<u8>,
    /// Probability the window mean must exceed (the manifest's `probability_cutoff`).
    pub cutoff: f32,
    /// Number of model outputs averaged (the manifest's `sliding_window_size`).
    pub window: usize,
    /// Accumulator rounding of convolution layers (see [`Rounding`]); the default follows the TFLite reference.
    pub rounding: Rounding,
}

impl WordConfig {
    /// A word with the reference rounding.
    #[must_use]
    pub fn new(model: Vec<u8>, cutoff: f32, window: usize) -> Self {
        Self {
            model,
            cutoff,
            window,
            rounding: Rounding::Reference,
        }
    }

    /// The same word with another rounding.
    #[must_use]
    pub fn with_rounding(mut self, rounding: Rounding) -> Self {
        self.rounding = rounding;
        self
    }
}

/// A detection before the daemon's refractory filter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RawDetection {
    /// Index of the word in the order given to [`Engine::new`].
    pub word: usize,
    /// The window mean, 0.0 to 1.0.
    pub score: f32,
    /// Stream offset in samples where the audio of the last feature ended.
    pub end_sample: u64,
}

#[derive(Debug, Clone)]
struct Word {
    interp: Interpreter,
    sha256: [u8; 32],
    stride: usize,
    features: Vec<i8>,
    gathered: usize,
    probabilities: Vec<f32>,
    next: usize,
    filled: usize,
    cutoff: f32,
    /// Inferences that failed to run. A model that plans but never runs would otherwise just never fire.
    faults: u64,
}

/// Frontend plus one interpreter per word.
#[derive(Debug, Clone)]
pub struct Engine {
    frontend: Frontend,
    words: Vec<Word>,
}

impl Word {
    fn new(cfg: WordConfig) -> Result<Self> {
        if cfg.window == 0 || cfg.window > 64 {
            return Err(Error::Invalid("sliding window size must be 1 to 64".into()));
        }
        if !(0.5..=0.999).contains(&cfg.cutoff) {
            return Err(Error::Invalid(
                "probability cutoff must be from 0.5 to 0.999".into(),
            ));
        }
        let hash = sha256(&cfg.model);
        let interp = Interpreter::with_rounding(cfg.model, cfg.rounding)?;
        let shape = interp.input_shape();
        let stride = match shape {
            [1, s, c] if *c == NUM_CHANNELS && (1..=16).contains(s) => *s,
            _ => {
                return Err(Error::Unsupported(format!(
                    "model input shape {shape:?}; expected [1, 1..16, {NUM_CHANNELS}]"
                )));
            }
        };
        Ok(Self {
            features: vec![0; stride * NUM_CHANNELS],
            interp,
            sha256: hash,
            stride,
            gathered: 0,
            probabilities: vec![0.0; cfg.window],
            next: 0,
            filled: 0,
            cutoff: cfg.cutoff,
            faults: 0,
        })
    }

    fn reset(&mut self) {
        self.interp.reset();
        self.gathered = 0;
        self.next = 0;
        self.filled = 0;
    }

    /// Take one feature frame; returns the window mean when the frame completed an inference and the window is full.
    fn push(&mut self, frame: &[u16; NUM_CHANNELS]) -> Option<f32> {
        let (scale, zp) = self.interp.input_quant();
        let base = self.gathered * NUM_CHANNELS;
        for (d, &raw) in self.features[base..base + NUM_CHANNELS]
            .iter_mut()
            .zip(frame.iter())
        {
            let q = (f64::from(raw) * FEATURE_SCALE / f64::from(scale) + f64::from(zp))
                .round_ties_even();
            *d = q.clamp(-128.0, 127.0) as i8;
        }
        self.gathered += 1;
        if self.gathered < self.stride {
            return None;
        }
        self.gathered = 0;
        if self.interp.invoke(&self.features).is_err() {
            // A model that planned but fails to run produces no detection rather than a panic; it is counted so
            // the caller can tell a silent model from a broken one (`Engine::faults`).
            self.faults += 1;
            return None;
        }
        let p = self.interp.output_value();
        self.probabilities[self.next] = p;
        self.next = (self.next + 1) % self.probabilities.len();
        self.filled = (self.filled + 1).min(self.probabilities.len());
        if self.filled < self.probabilities.len() {
            return Some(0.0);
        }
        let sum: f64 = self.probabilities.iter().map(|&v| f64::from(v)).sum();
        Some((sum / self.probabilities.len() as f64) as f32)
    }
}

impl Engine {
    /// Build an engine for the given words.
    pub fn new(words: Vec<WordConfig>) -> Result<Self> {
        if words.is_empty() {
            return Err(Error::Invalid("an engine needs at least one word".into()));
        }
        Ok(Self {
            frontend: Frontend::new(),
            words: words.into_iter().map(Word::new).collect::<Result<_>>()?,
        })
    }

    /// Number of words.
    #[must_use]
    pub fn words(&self) -> usize {
        self.words.len()
    }

    /// SHA-256 of a word's model file.
    #[must_use]
    pub fn model_sha256(&self, word: usize) -> Option<[u8; 32]> {
        self.words.get(word).map(|w| w.sha256)
    }

    /// Set a word's cutoff (clamped to 0.5 to 0.999).
    pub fn set_cutoff(&mut self, word: usize, cutoff: f32) {
        if let Some(w) = self.words.get_mut(word) {
            w.cutoff = if cutoff.is_nan() {
                0.5
            } else {
                cutoff.clamp(0.5, 0.999)
            };
        }
    }

    /// A word's current cutoff.
    #[must_use]
    pub fn cutoff(&self, word: usize) -> Option<f32> {
        self.words.get(word).map(|w| w.cutoff)
    }

    /// Working memory in bytes: every word's arena and state, the model files, and the frontend.
    #[must_use]
    pub fn memory_bytes(&self) -> usize {
        std::mem::size_of::<Frontend>()
            + self
                .words
                .iter()
                .map(|w| {
                    w.interp.working_bytes()
                        + w.interp.model_bytes()
                        + w.features.len()
                        + w.probabilities.len() * 4
                })
                .sum::<usize>()
    }

    /// How many inferences failed to run, over all words, since construction. Zero for a healthy model; a
    /// non-zero count that keeps rising means a word that can never fire.
    #[must_use]
    pub fn faults(&self) -> u64 {
        self.words.iter().map(|w| w.faults).sum()
    }

    /// Samples consumed since construction or the last [`Engine::reset`].
    #[must_use]
    pub fn samples(&self) -> u64 {
        self.frontend.samples()
    }

    /// Drop all streaming state; equivalent to a fresh engine.
    pub fn reset(&mut self) {
        self.frontend.reset();
        for w in &mut self.words {
            w.reset();
        }
    }

    /// Feed audio of any length. Calls `on_window` for every inference that produced a decision, whether or not it
    /// crossed the cutoff, with the word, the window mean and the stream offset; and `on_detect` when it did.
    pub fn process_with(
        &mut self,
        pcm: &[i16],
        mut on_window: impl FnMut(usize, f32, u64),
        mut on_detect: impl FnMut(RawDetection),
    ) {
        let Self { frontend, words } = self;
        frontend.process(pcm, |frame, end| {
            for (i, w) in words.iter_mut().enumerate() {
                if let Some(mean) = w.push(frame) {
                    on_window(i, mean, end);
                    if mean > w.cutoff {
                        on_detect(RawDetection {
                            word: i,
                            score: mean,
                            end_sample: end,
                        });
                    }
                }
            }
        });
    }

    /// Feed audio and collect detections.
    pub fn process(&mut self, pcm: &[i16], out: &mut Vec<RawDetection>) {
        self.process_with(pcm, |_, _, _| {}, |d| out.push(d));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODEL: &[u8] = include_bytes!("../tests/golden/models/okay_nabu.tflite");

    #[test]
    fn req_wak_002_a_model_that_fails_to_run_is_counted_not_silent() {
        let mut e = Engine::new(vec![WordConfig::new(MODEL.to_vec(), 0.97, 5)]).unwrap();
        let silence = vec![0i16; 16_000];
        let mut out = Vec::new();
        e.process(&silence, &mut out);
        assert_eq!(e.faults(), 0, "a healthy model has no faults");
        // Make every inference fail: the input handed to the interpreter no longer has the model's length.
        e.words[0].features.push(0);
        e.process(&silence, &mut out);
        assert!(e.faults() > 0, "failed inferences are counted");
        assert!(out.is_empty());
    }
}
