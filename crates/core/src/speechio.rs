//! The speech clients (speech to text, text to speech) built from the `speech` configuration section, and
//! the registry that lets a change of voice or language in the portal reach them while running.

use crate::config::SpeechConfig;
use crate::env::Env;
use crate::runtime::{Brain, respond_inline};
use crate::secrets::Secrets;
use crate::speech::{HttpSynthesizer, HttpTranscriber, parse_wav, wav};
use crate::turns::Hub;
use crate::voice::voice_turn;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

#[derive(Default)]
struct Registered {
    voices: Vec<Arc<Mutex<String>>>,
    languages: Vec<Arc<Mutex<Option<String>>>>,
}

/// The speech clients that exist, so a change of voice or language in the portal reaches them while running.
/// A cheap handle: clones share one registry. Each [`crate::app::Core`] owns one, so two cores in one process
/// never change each other's voice.
#[derive(Clone, Default)]
pub struct SpeechHandles(Arc<Mutex<Registered>>);

impl std::fmt::Debug for SpeechHandles {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SpeechHandles")
    }
}

impl SpeechHandles {
    fn lock(&self) -> std::sync::MutexGuard<'_, Registered> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Remember how to change the voice of `tts` and the language of `stt` later.
    pub fn register(&self, stt: &HttpTranscriber, tts: &HttpSynthesizer) {
        let mut h = self.lock();
        h.voices.push(tts.voice_handle());
        h.languages.push(stt.language_handle());
    }

    /// Make every speech client use `voice` now, and return the voice they were using (none when there is no
    /// client).
    pub fn swap_voice(&self, voice: &str) -> Option<String> {
        let h = self.lock();
        let mut old = None;
        for slot in &h.voices {
            let mut v = slot.lock().unwrap_or_else(PoisonError::into_inner);
            old.get_or_insert_with(|| v.clone());
            *v = voice.to_owned();
        }
        old
    }

    /// Apply the voice and language that are now set to every speech client.
    pub fn apply(&self, live: &serde_json::Map<String, serde_json::Value>) {
        let h = self.lock();
        if let Some(v) = live
            .get("voice")
            .and_then(serde_json::Value::as_str)
            .filter(|v| !v.is_empty())
        {
            for slot in &h.voices {
                *slot.lock().unwrap_or_else(PoisonError::into_inner) = v.to_owned();
            }
        }
        let language = live
            .get("speech_language")
            .and_then(serde_json::Value::as_str)
            .filter(|l| !l.is_empty())
            .map(str::to_owned);
        for slot in &h.languages {
            *slot.lock().unwrap_or_else(PoisonError::into_inner) = language.clone();
        }
    }
}

/// Build the speech clients and register them with `handles` so their voice and language can change later.
pub fn build_registered_speech(
    speech: &SpeechConfig,
    secrets: &Secrets,
    env: &Env,
    handles: &SpeechHandles,
) -> Result<(HttpTranscriber, HttpSynthesizer), String> {
    let (stt, tts) = build_speech(speech, secrets, env)?;
    handles.register(&stt, &tts);
    Ok((stt, tts))
}

pub fn build_speech(
    speech: &SpeechConfig,
    secrets: &Secrets,
    env: &Env,
) -> Result<(HttpTranscriber, HttpSynthesizer), String> {
    let missing =
        |k: &str| format!("speech.{k} is not configured (see docs/architecture/voice-pipeline.md)");
    let url = speech.url.as_deref().ok_or_else(|| missing("url"))?;
    let key = speech
        .key_env
        .as_deref()
        .map(|v| env.get(v).unwrap_or_default())
        .unwrap_or_default();
    let deadline = Duration::from_secs(30);
    let dialect = speech.dialect();
    let stt = HttpTranscriber::new(
        url,
        &key,
        speech
            .stt_model
            .as_deref()
            .ok_or_else(|| missing("stt_model"))?,
        speech.language.as_deref(),
        deadline,
    )?
    .with_dialect(dialect)
    .with_stored_key(secrets.clone());
    let tts = HttpSynthesizer::new(
        url,
        &key,
        speech
            .tts_model
            .as_deref()
            .ok_or_else(|| missing("tts_model"))?,
        speech.voice.as_deref().ok_or_else(|| missing("voice"))?,
        speech.tts_sample_rate.unwrap_or(24_000),
        deadline,
    )?
    .with_dialect(dialect)?
    .with_stored_key(secrets.clone());
    Ok((stt, tts))
}

/// Answer one recorded utterance: transcribe the WAV, respond, speak the reply into a WAV.
pub async fn voice_once(
    brain: &mut Brain,
    hub: &mut Hub,
    speech: &SpeechConfig,
    secrets: &Secrets,
    input: &std::path::Path,
    output: Option<&std::path::Path>,
) -> Result<(), String> {
    let (stt, tts) = build_speech(speech, secrets, &brain.env)?;
    let bytes =
        std::fs::read(input).map_err(|e| format!("cannot read {}: {e}", input.display()))?;
    let pcm = parse_wav(&bytes).map_err(|e| format!("{}: {e}", input.display()))?;

    let effects = std::cell::RefCell::new(Vec::new());
    let out = voice_turn(&stt, &tts, &pcm, |text| {
        let brain = &mut *brain;
        let hub = &mut *hub;
        let effects = &effects;
        async move {
            let r = respond_inline(brain, hub, &text).await;
            effects.borrow_mut().extend(r.effects);
            r.say
        }
    })
    .await;
    match &out.transcript {
        Some(t) => println!("heard> {t}"),
        None => println!("heard> (nothing understood)"),
    }
    if !out.reply.is_empty() {
        println!("speaker> {}", out.reply);
    }
    for e in effects.into_inner() {
        println!("satellite> {e:?}");
    }
    for p in &out.problems {
        eprintln!("clawdot: {p}");
    }
    if let Some(path) = output {
        let audio: Vec<i16> = out.audio.concat();
        std::fs::write(path, wav(&audio))
            .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        println!(
            "clawdot: wrote {} sample(s) of speech to {}",
            audio.len(),
            path.display()
        );
    }
    Ok(())
}
