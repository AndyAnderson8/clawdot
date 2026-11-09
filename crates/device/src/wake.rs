//! The wake-engine seam (REQ-DEV-017, spec 0006 section 7, R-11 section 9).
//!
//! Frame in, detection out. The daemon ships this seam and push-to-talk, not an engine; an engine is a separate
//! step. [`Refractory`] and [`Sensitivity`] are the pieces every engine needs and are tested here so engines
//! share one behaviour.

/// Identifies a wake word. [`WordId::PTT`] is push-to-talk, the always-available source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WordId(pub u16);

impl WordId {
    /// Push-to-talk: a button or the protocol, not an engine.
    pub const PTT: WordId = WordId(0);
}

/// A word an engine listens for, with provenance so status can show it and REQ-VOI-009 can be checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WordInfo {
    /// Stable id.
    pub id: WordId,
    /// The phrase.
    pub phrase: String,
    /// SHA-256 of the model file.
    pub model_sha256: [u8; 32],
    /// Declared licence of the model.
    pub licence: String,
}

/// One detection.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Detection {
    /// Which word.
    pub word: WordId,
    /// Engine confidence, 0.0 to 1.0.
    pub score: f32,
    /// Offset in samples since the stream began where the phrase ended.
    pub end_sample: u64,
}

/// A wake-word engine. 16 kHz mono signed 16-bit PCM in; detections out. No audio leaves the engine and none is
/// stored by it.
pub trait WakeEngine: Send {
    /// Words this engine listens for, in a stable order.
    fn words(&self) -> &[WordInfo];
    /// Feed audio of any length (10 ms is the natural unit). Appends zero or more detections to `out`.
    fn process(&mut self, pcm: &[i16], out: &mut Vec<Detection>);
    /// 0.0 (strict) to 1.0 (loose). Values outside the range are clamped.
    fn set_sensitivity(&mut self, word: WordId, sensitivity: f32);
    /// Drop all streaming state (after mute, after a turn, after a long gap).
    fn reset(&mut self);
}

/// Clamp a sensitivity to 0.0 to 1.0; NaN becomes 0.5, the model's published cutoff.
#[must_use]
pub fn clamp_sensitivity(s: f32) -> f32 {
    if s.is_nan() { 0.5 } else { s.clamp(0.0, 1.0) }
}

/// Per-word sensitivity table with the default 0.5.
#[derive(Debug, Clone, Default)]
pub struct Sensitivity {
    values: Vec<(WordId, f32)>,
}

impl Sensitivity {
    /// Set a word's sensitivity.
    pub fn set(&mut self, word: WordId, s: f32) {
        let s = clamp_sensitivity(s);
        match self.values.iter_mut().find(|(w, _)| *w == word) {
            Some(e) => e.1 = s,
            None => self.values.push((word, s)),
        }
    }

    /// A word's sensitivity (0.5 if never set).
    #[must_use]
    pub fn get(&self, word: WordId) -> f32 {
        self.values
            .iter()
            .find(|(w, _)| *w == word)
            .map_or(0.5, |(_, s)| *s)
    }
}

/// The refractory period: after a detection, further detections of the same word within the window are dropped
/// so one utterance yields one detection. Measured in samples of the stream.
#[derive(Debug, Clone)]
pub struct Refractory {
    window: u64,
    last: Vec<(WordId, u64)>,
}

/// Default refractory period, 2 s at 16 kHz.
pub const DEFAULT_REFRACTORY_SAMPLES: u64 = 2 * 16_000;

impl Default for Refractory {
    fn default() -> Self {
        Self::new(DEFAULT_REFRACTORY_SAMPLES)
    }
}

impl Refractory {
    /// A filter with a window of `samples`.
    #[must_use]
    pub fn new(samples: u64) -> Self {
        Self {
            window: samples,
            last: Vec::new(),
        }
    }

    /// Whether `d` passes the filter (and, if so, starts a new window for its word).
    pub fn accept(&mut self, d: &Detection) -> bool {
        match self.last.iter_mut().find(|(w, _)| *w == d.word) {
            Some((_, at)) => {
                if d.end_sample.saturating_sub(*at) < self.window && d.end_sample >= *at {
                    false
                } else {
                    *at = d.end_sample;
                    true
                }
            }
            None => {
                self.last.push((d.word, d.end_sample));
                true
            }
        }
    }

    /// Forget past detections (with an engine reset).
    pub fn reset(&mut self) {
        self.last.clear();
    }
}

/// Feeds microphone frames to an engine while the daemon is listening for the wake word, and drops them while
/// muted. The detection that results is the same event push-to-talk produces.
pub struct WakeRouter<E: WakeEngine> {
    engine: E,
    muted: bool,
    scratch: Vec<Detection>,
}

impl<E: WakeEngine> std::fmt::Debug for WakeRouter<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WakeRouter")
            .field("muted", &self.muted)
            .finish_non_exhaustive()
    }
}

impl<E: WakeEngine> WakeRouter<E> {
    /// Wrap an engine.
    pub fn new(engine: E) -> Self {
        Self {
            engine,
            muted: false,
            scratch: Vec::new(),
        }
    }

    /// Mute or unmute. Muting means the engine is not fed; unmuting resets it (R-11 rule 2).
    pub fn set_muted(&mut self, muted: bool) {
        if self.muted && !muted {
            self.engine.reset();
        }
        self.muted = muted;
    }

    /// Offer a frame. Returns the first detection, if any. While muted the frame is not read at all.
    pub fn feed(&mut self, pcm: &[i16]) -> Option<Detection> {
        if self.muted {
            return None;
        }
        self.scratch.clear();
        self.engine.process(pcm, &mut self.scratch);
        self.scratch.first().copied()
    }

    /// The engine, for status (words, provenance).
    #[must_use]
    pub fn engine(&self) -> &E {
        &self.engine
    }
}

/// A test engine: detects when the incoming samples cross a stream offset set in advance. Not an engine for
/// real use.
#[derive(Debug, Default)]
pub struct ScriptedEngine {
    words: Vec<WordInfo>,
    fire_at: Vec<(WordId, u64)>,
    seen: u64,
    refractory: Refractory,
    sensitivity: Sensitivity,
    /// Samples processed since the last reset, for tests.
    pub fed: u64,
}

impl ScriptedEngine {
    /// An engine for one word that fires when the stream reaches each offset in `fire_at`.
    #[must_use]
    pub fn new(word: WordInfo, fire_at: Vec<u64>) -> Self {
        let id = word.id;
        Self {
            words: vec![word],
            fire_at: fire_at.into_iter().map(|o| (id, o)).collect(),
            ..Self::default()
        }
    }

    /// The sensitivity currently set for a word.
    #[must_use]
    pub fn sensitivity(&self, word: WordId) -> f32 {
        self.sensitivity.get(word)
    }
}

impl WakeEngine for ScriptedEngine {
    fn words(&self) -> &[WordInfo] {
        &self.words
    }

    fn process(&mut self, pcm: &[i16], out: &mut Vec<Detection>) {
        let start = self.seen;
        self.seen += pcm.len() as u64;
        self.fed += pcm.len() as u64;
        for &(word, at) in &self.fire_at {
            if at >= start && at < self.seen {
                let d = Detection {
                    word,
                    score: 0.9,
                    end_sample: at,
                };
                if self.refractory.accept(&d) {
                    out.push(d);
                }
            }
        }
    }

    fn set_sensitivity(&mut self, word: WordId, sensitivity: f32) {
        self.sensitivity.set(word, sensitivity);
    }

    fn reset(&mut self) {
        self.seen = 0;
        self.fed = 0;
        self.refractory.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word() -> WordInfo {
        WordInfo {
            id: WordId(1),
            phrase: "okay nabu".into(),
            model_sha256: [7; 32],
            licence: "Apache-2.0".into(),
        }
    }

    #[test]
    fn req_dev_017_push_to_talk_has_a_reserved_id() {
        assert_eq!(WordId::PTT, WordId(0));
        assert_ne!(word().id, WordId::PTT);
    }

    #[test]
    fn req_dev_017_a_detection_comes_out_when_its_frame_goes_in() {
        let mut r = WakeRouter::new(ScriptedEngine::new(word(), vec![1_000]));
        assert!(r.feed(&[0; 320]).is_none());
        assert!(r.feed(&[0; 320]).is_none());
        let d = r.feed(&[0; 480]).unwrap();
        assert_eq!(d.word, WordId(1));
        assert_eq!(d.end_sample, 1_000);
        assert!(d.score > 0.0 && d.score <= 1.0);
    }

    #[test]
    fn req_dev_017_muted_means_not_fed_and_unmute_resets() {
        let mut r = WakeRouter::new(ScriptedEngine::new(word(), vec![100]));
        r.set_muted(true);
        assert!(r.feed(&[0; 320]).is_none());
        assert_eq!(r.engine().fed, 0, "a muted engine is not fed");
        r.set_muted(false);
        assert_eq!(r.engine().fed, 0);
        assert!(
            r.feed(&[0; 320]).is_some(),
            "stream offsets restart after the reset"
        );
    }

    #[test]
    fn req_dev_017_one_utterance_yields_one_detection() {
        let mut f = Refractory::new(32_000);
        let d = |end| Detection {
            word: WordId(1),
            score: 0.8,
            end_sample: end,
        };
        assert!(f.accept(&d(10_000)));
        assert!(!f.accept(&d(20_000)));
        assert!(f.accept(&d(42_000)));
        // A different word has its own window.
        assert!(f.accept(&Detection {
            word: WordId(2),
            score: 0.8,
            end_sample: 43_000
        }));
        f.reset();
        assert!(f.accept(&d(43_000)));
    }

    #[test]
    fn req_dev_017_sensitivity_is_per_word_and_clamped() {
        let mut e = ScriptedEngine::new(word(), vec![]);
        assert!((e.sensitivity(WordId(1)) - 0.5).abs() < f32::EPSILON);
        e.set_sensitivity(WordId(1), 1.7);
        assert!((e.sensitivity(WordId(1)) - 1.0).abs() < f32::EPSILON);
        e.set_sensitivity(WordId(1), -3.0);
        assert!(e.sensitivity(WordId(1)).abs() < f32::EPSILON);
        e.set_sensitivity(WordId(1), f32::NAN);
        assert!((e.sensitivity(WordId(1)) - 0.5).abs() < f32::EPSILON);
        assert!((e.sensitivity(WordId(2)) - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn req_dev_017_words_carry_provenance() {
        let e = ScriptedEngine::new(word(), vec![]);
        let w = &e.words()[0];
        assert_eq!(w.licence, "Apache-2.0");
        assert_eq!(w.model_sha256, [7; 32]);
    }
}
