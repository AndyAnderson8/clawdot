//! The microWakeWord engine behind the `WakeEngine` seam (spec 0007 REQ-WAK-006 to REQ-WAK-008, spec 0006
//! section 7). Built only with the `mww` cargo feature; the default build links none of it.
//!
//! [`MwwEngine`] wraps `clawdot_wake::Engine` (one frontend, one model per word) and adds what the seam asks for:
//! stable word ids, provenance (model hash and licence), one sensitivity per word, and the refractory filter.

use clawdot_wake::{Engine, RawDetection, WordConfig};

use crate::wake::{
    Detection, Refractory, Sensitivity, WakeEngine, WordId, WordInfo, clamp_sensitivity,
};

/// One word to load: the model file, its manifest values and where it came from.
#[derive(Debug, Clone)]
pub struct MwwWord {
    /// Stable id the daemon routes on (not [`WordId::PTT`]).
    pub id: WordId,
    /// The phrase, for status and the portal.
    pub phrase: String,
    /// The `.tflite` file.
    pub model: Vec<u8>,
    /// The manifest's `probability_cutoff`: the cutoff at sensitivity 0.5.
    pub probability_cutoff: f32,
    /// The manifest's `sliding_window_size`.
    pub sliding_window_size: usize,
    /// Declared licence of the model (REQ-VOI-009); must not be empty.
    pub licence: String,
}

impl MwwWord {
    /// Build a word from the model file and the text of its manifest (`okay_nabu.json`), which holds the phrase
    /// and the cutoff and window the model was published with. `licence` is the licence the caller has recorded
    /// for the model file; an empty one is refused (REQ-VOI-009).
    pub fn from_manifest(
        id: WordId,
        manifest: &str,
        model: Vec<u8>,
        licence: &str,
    ) -> Result<Self, String> {
        let v: serde_json::Value =
            serde_json::from_str(manifest).map_err(|e| format!("wake word manifest: {e}"))?;
        let micro = v
            .get("micro")
            .ok_or("wake word manifest: no \"micro\" section")?;
        let cutoff = micro
            .get("probability_cutoff")
            .and_then(serde_json::Value::as_f64)
            .ok_or("wake word manifest: probability_cutoff is missing or not a number")?;
        let window = micro
            .get("sliding_window_size")
            .and_then(serde_json::Value::as_u64)
            .ok_or("wake word manifest: sliding_window_size is missing or not an integer")?;
        if let Some(step) = micro
            .get("feature_step_size")
            .and_then(serde_json::Value::as_u64)
        {
            if step != 10 {
                return Err(format!(
                    "wake word manifest: feature_step_size {step} is not supported (10 only)"
                ));
            }
        }
        let phrase = v
            .get("wake_word")
            .and_then(serde_json::Value::as_str)
            .ok_or("wake word manifest: wake_word is missing")?
            .to_string();
        Ok(Self {
            id,
            phrase,
            model,
            probability_cutoff: cutoff as f32,
            sliding_window_size: usize::try_from(window).unwrap_or(usize::MAX),
            licence: licence.to_string(),
        })
    }
}

/// Map the normalised sensitivity onto a cutoff. 0.5 is the published cutoff `base`; the distance of the cutoff
/// from 1.0 scales by 4 per half step: 0.0 is strict (a quarter of the distance), 1.0 is loose (four times it).
/// The result stays between 0.5 and 0.999.
#[must_use]
pub fn cutoff_for(base: f32, sensitivity: f32) -> f32 {
    let s = clamp_sensitivity(sensitivity);
    let margin = (1.0 - base) * 4f32.powf(2.0 * s - 1.0);
    (1.0 - margin).clamp(0.5, 0.999)
}

/// The microWakeWord engine as a [`WakeEngine`].
#[derive(Debug)]
pub struct MwwEngine {
    engine: Engine,
    words: Vec<WordInfo>,
    base_cutoff: Vec<f32>,
    refractory: Refractory,
    sensitivity: Sensitivity,
    scratch: Vec<RawDetection>,
}

impl MwwEngine {
    /// Load the words. Fails with a message naming the problem if a model is refused or a licence is missing.
    pub fn new(words: Vec<MwwWord>) -> Result<Self, String> {
        let mut configs = Vec::with_capacity(words.len());
        for w in &words {
            if w.id == WordId::PTT {
                return Err("word id 0 is reserved for push-to-talk".into());
            }
            if w.licence.trim().is_empty() {
                return Err(format!(
                    "wake word \"{}\" has no recorded licence (REQ-VOI-009)",
                    w.phrase
                ));
            }
            configs.push(WordConfig::new(
                w.model.clone(),
                w.probability_cutoff,
                w.sliding_window_size,
            ));
        }
        let engine = Engine::new(configs).map_err(|e| e.to_string())?;
        let infos = words
            .iter()
            .enumerate()
            .map(|(i, w)| WordInfo {
                id: w.id,
                phrase: w.phrase.clone(),
                model_sha256: engine.model_sha256(i).unwrap_or_default(),
                licence: w.licence.clone(),
            })
            .collect();
        Ok(Self {
            engine,
            words: infos,
            base_cutoff: words.iter().map(|w| w.probability_cutoff).collect(),
            refractory: Refractory::default(),
            sensitivity: Sensitivity::default(),
            scratch: Vec::new(),
        })
    }

    /// Use another refractory period, in samples.
    #[must_use]
    pub fn with_refractory(mut self, samples: u64) -> Self {
        self.refractory = Refractory::new(samples);
        self
    }

    /// Working memory of the runtime in bytes (arenas, state, model files, frontend).
    #[must_use]
    pub fn memory_bytes(&self) -> usize {
        self.engine.memory_bytes()
    }

    /// The cutoff currently in force for a word.
    #[must_use]
    pub fn cutoff(&self, word: WordId) -> Option<f32> {
        let i = self.words.iter().position(|w| w.id == word)?;
        self.engine.cutoff(i)
    }
}

impl WakeEngine for MwwEngine {
    fn words(&self) -> &[WordInfo] {
        &self.words
    }

    fn process(&mut self, pcm: &[i16], out: &mut Vec<Detection>) {
        self.scratch.clear();
        self.engine.process(pcm, &mut self.scratch);
        for raw in &self.scratch {
            let Some(info) = self.words.get(raw.word) else {
                continue;
            };
            let d = Detection {
                word: info.id,
                score: raw.score,
                end_sample: raw.end_sample,
            };
            if self.refractory.accept(&d) {
                out.push(d);
            }
        }
    }

    fn set_sensitivity(&mut self, word: WordId, sensitivity: f32) {
        let Some(i) = self.words.iter().position(|w| w.id == word) else {
            return;
        };
        self.sensitivity.set(word, sensitivity);
        let base = self.base_cutoff.get(i).copied().unwrap_or(0.9);
        self.engine
            .set_cutoff(i, cutoff_for(base, self.sensitivity.get(word)));
    }

    fn reset(&mut self) {
        self.engine.reset();
        self.refractory.reset();
    }
}

/// Read a word from a model file and the manifest beside it (`okay_nabu.tflite` and `okay_nabu.json`), as the PC
/// voice loop is configured (REQ-DEV-029). Every error names the file.
///
/// # Errors
/// A file cannot be read, or the manifest is not acceptable (see [`MwwWord::from_manifest`]).
pub fn load_word(id: WordId, model: &std::path::Path, licence: &str) -> Result<MwwWord, String> {
    let bytes =
        std::fs::read(model).map_err(|e| format!("wake word model {}: {e}", model.display()))?;
    let manifest_path = model.with_extension("json");
    let manifest = std::fs::read_to_string(&manifest_path).map_err(|e| {
        format!(
            "wake word manifest {} (it must sit beside the model): {e}",
            manifest_path.display()
        )
    })?;
    MwwWord::from_manifest(id, &manifest, bytes, licence)
        .map_err(|e| format!("{}: {e}", manifest_path.display()))
}

/// The engine as the audio front end's [`WakeSink`] (REQ-DEV-025): 20 ms frames in, the phrase out.
#[derive(Debug)]
pub struct MwwSink {
    engine: MwwEngine,
    scratch: Vec<Detection>,
    name: String,
}

impl MwwSink {
    /// Wrap an engine. Its words, in order, are what the user is told to say.
    #[must_use]
    pub fn new(engine: MwwEngine) -> Self {
        let name = engine
            .words()
            .iter()
            .map(|w| w.phrase.to_lowercase())
            .collect::<Vec<_>>()
            .join("\" or \"");
        Self {
            engine,
            scratch: Vec::new(),
            name,
        }
    }
}

impl clawdot_satellite::front::WakeSink for MwwSink {
    fn name(&self) -> String {
        self.name.clone()
    }

    fn feed(&mut self, pcm: &[i16]) -> Option<String> {
        self.scratch.clear();
        self.engine.process(pcm, &mut self.scratch);
        let d = self.scratch.first()?;
        self.engine
            .words()
            .iter()
            .find(|w| w.id == d.word)
            .map(|w| w.phrase.to_lowercase())
    }

    fn reset(&mut self) {
        self.engine.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wake::WakeRouter;
    use clawdot_satellite::front::WakeSink;

    const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../wake/tests/golden");

    fn file(name: &str) -> Vec<u8> {
        std::fs::read(format!("{DIR}/{name}")).unwrap()
    }

    fn pcm(name: &str) -> Vec<i16> {
        file(name)
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect()
    }

    fn nabu() -> MwwWord {
        MwwWord::from_manifest(
            WordId(1),
            &String::from_utf8(file("models/okay_nabu.json")).unwrap(),
            file("models/okay_nabu.tflite"),
            "Apache-2.0",
        )
        .unwrap()
    }

    fn engine() -> MwwEngine {
        MwwEngine::new(vec![nabu()]).unwrap()
    }

    #[test]
    fn req_wak_008_the_manifest_gives_phrase_cutoff_and_window() {
        let w = nabu();
        assert_eq!(w.phrase, "Okay Nabu");
        assert!((w.probability_cutoff - 0.97).abs() < 1e-6);
        assert_eq!(w.sliding_window_size, 5);
    }

    #[test]
    fn req_wak_008_bad_manifests_and_missing_licences_are_refused() {
        let m = |s: &str| MwwWord::from_manifest(WordId(1), s, vec![], "Apache-2.0");
        assert!(m("not json").is_err());
        assert!(m("{}").is_err());
        assert!(m(r#"{"wake_word":"x","micro":{}}"#).is_err());
        assert!(
            m(r#"{"wake_word":"x","micro":{"probability_cutoff":0.9,"sliding_window_size":5,"feature_step_size":20}}"#)
                .is_err()
        );
        let mut w = nabu();
        w.licence = "  ".into();
        assert!(MwwEngine::new(vec![w]).unwrap_err().contains("licence"));
        let mut w = nabu();
        w.id = WordId::PTT;
        assert!(MwwEngine::new(vec![w]).is_err());
        let mut w = nabu();
        w.model = vec![1, 2, 3];
        assert!(MwwEngine::new(vec![w]).is_err());
    }

    #[test]
    fn req_wak_008_words_carry_the_model_hash_and_licence() {
        let e = engine();
        let w = &e.words()[0];
        assert_eq!(w.id, WordId(1));
        assert_eq!(w.licence, "Apache-2.0");
        let hex: String = w.model_sha256.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex,
            "0689abe1912a95a3318a0d8cb2e67bad0cbcfe3e24dd6e050c75debddfb6f891"
        );
    }

    #[test]
    fn req_wak_008_one_utterance_gives_one_detection_through_the_router() {
        let mut router = WakeRouter::new(engine());
        let audio = pcm("phrase_a.s16");
        let mut hits = Vec::new();
        for chunk in audio.chunks(160) {
            if let Some(d) = router.feed(chunk) {
                hits.push(d);
            }
        }
        assert_eq!(hits.len(), 1, "the refractory filter keeps one: {hits:?}");
        assert_eq!(hits[0].word, WordId(1));
        assert!(hits[0].score > 0.97 && hits[0].score <= 1.0);
        assert!(hits[0].end_sample <= audio.len() as u64);
    }

    #[test]
    fn req_wak_008_quiet_audio_gives_no_detection_and_reset_forgets() {
        let mut e = engine();
        let mut out = Vec::new();
        e.process(&pcm("speech.s16"), &mut out);
        e.process(&pcm("tones.s16"), &mut out);
        assert!(out.is_empty());
        // A reset makes the engine a fresh one: the phrase fires, and after another reset it fires at exactly the
        // same place with the same score.
        e.reset();
        e.process(&pcm("phrase_a.s16"), &mut out);
        assert_eq!(out.len(), 1, "{out:?}");
        e.reset();
        let mut again = Vec::new();
        e.process(&pcm("phrase_a.s16"), &mut again);
        assert_eq!(again, out);
    }

    #[test]
    fn req_wak_007_sensitivity_maps_monotonically_onto_the_cutoff() {
        let base = 0.97;
        assert!((cutoff_for(base, 0.5) - base).abs() < 1e-6);
        let mut last = 2.0;
        for i in 0..=20 {
            let c = cutoff_for(base, i as f32 / 20.0);
            assert!(c < last, "looser sensitivity must lower the cutoff");
            assert!((0.5..=0.999).contains(&c));
            last = c;
        }
        // Out-of-range and NaN sensitivities are clamped; a low base cutoff cannot go below 0.5.
        assert_eq!(cutoff_for(base, 5.0), cutoff_for(base, 1.0));
        assert_eq!(cutoff_for(base, f32::NAN), cutoff_for(base, 0.5));
        assert!(cutoff_for(0.6, 1.0) >= 0.5);
    }

    #[test]
    fn req_wak_007_sensitivity_changes_what_fires() {
        let phrase_b = pcm("phrase_b.s16");
        // phrase_b scores a window mean of about 0.1 for okay_nabu: nothing fires at the published cutoff,
        // and nothing fires even at the loosest setting (cutoff 0.88).
        let mut e = engine();
        e.set_sensitivity(WordId(1), 1.0);
        assert!(e.cutoff(WordId(1)).unwrap() < 0.9);
        let mut out = Vec::new();
        e.process(&phrase_b, &mut out);
        assert!(out.is_empty());
        // A strict setting turns off the phrase it normally hears (mean 0.977 against cutoff 0.9925).
        let mut strict = engine();
        strict.set_sensitivity(WordId(1), 0.0);
        let mut out = Vec::new();
        strict.process(&pcm("phrase_a.s16"), &mut out);
        assert!(out.is_empty());
        // Unknown ids are ignored.
        strict.set_sensitivity(WordId(9), 1.0);
        assert_eq!(strict.cutoff(WordId(9)), None);
    }

    #[test]
    fn two_words_share_one_engine_with_their_own_ids() {
        let mycroft = MwwWord::from_manifest(
            WordId(2),
            &String::from_utf8(file("models/hey_mycroft.json")).unwrap(),
            file("models/hey_mycroft.tflite"),
            "Apache-2.0",
        )
        .unwrap();
        let mut e = MwwEngine::new(vec![nabu(), mycroft]).unwrap();
        assert_eq!(e.words().len(), 2);
        assert_ne!(e.words()[0].model_sha256, e.words()[1].model_sha256);
        e.set_sensitivity(WordId(2), 1.0);
        let mut out = Vec::new();
        e.process(&pcm("phrase_a.s16"), &mut out);
        e.process(&pcm("phrase_b.s16"), &mut out);
        assert!(out.iter().any(|d| d.word == WordId(1)));
        assert!(e.memory_bytes() > 100_000 && e.memory_bytes() < 400_000);
    }

    #[test]
    fn req_dev_029_words_load_from_a_path_with_the_manifest_beside_it() {
        let model = std::path::Path::new(DIR).join("models/okay_nabu.tflite");
        let w = load_word(WordId(1), &model, "Apache-2.0").unwrap();
        assert_eq!(w.phrase, "Okay Nabu");
        assert_eq!(w.model.len(), 60_264);
        let missing = std::path::Path::new(DIR).join("models/nope.tflite");
        let e = load_word(WordId(1), &missing, "Apache-2.0").unwrap_err();
        assert!(e.contains("nope.tflite"), "{e}");
        let dir = std::env::temp_dir().join(format!("clawdot-mww-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("w.tflite"), b"x").unwrap();
        let e = load_word(WordId(1), &dir.join("w.tflite"), "Apache-2.0").unwrap_err();
        assert!(e.contains("w.json") && e.contains("beside"), "{e}");
        std::fs::write(dir.join("w.json"), b"{}").unwrap();
        let e = load_word(WordId(1), &dir.join("w.tflite"), "Apache-2.0").unwrap_err();
        assert!(e.contains("w.json"), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn req_dev_025_the_sink_hears_the_phrase_in_20_ms_frames_and_forgets_on_reset() {
        let mut sink = MwwSink::new(engine());
        assert_eq!(sink.name(), "okay nabu");
        let audio = pcm("phrase_a.s16");
        let hits = |sink: &mut MwwSink| {
            audio
                .chunks(320)
                .filter_map(|c| sink.feed(c))
                .collect::<Vec<_>>()
        };
        assert_eq!(hits(&mut sink), ["okay nabu"], "one hit for one utterance");
        // The refractory period holds the second utterance back until a reset.
        sink.reset();
        assert_eq!(hits(&mut sink), ["okay nabu"]);
        let mut quiet = MwwSink::new(engine());
        let silence = vec![0i16; 320];
        for _ in 0..500 {
            assert_eq!(quiet.feed(&silence), None);
        }
    }
}
