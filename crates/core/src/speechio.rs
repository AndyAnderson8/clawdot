//! The speech clients (speech to text, text to speech) built from the `speech` configuration section, and
//! the registry that lets a change of voice or language in the portal reach them while running.

use crate::config::SpeechConfig;
use crate::speech::{HttpSynthesizer, HttpTranscriber};
use std::sync::Arc;
use std::time::Duration;

/// Build the speech providers from the `speech` configuration section.
/// The speech clients that exist, so a change of voice or language in the portal reaches them while running.
#[derive(Default)]
struct SpeechHandles {
    voices: Vec<Arc<std::sync::Mutex<String>>>,
    languages: Vec<Arc<std::sync::Mutex<Option<String>>>>,
}

static SPEECH_HANDLES: std::sync::Mutex<SpeechHandles> = std::sync::Mutex::new(SpeechHandles {
    voices: Vec::new(),
    languages: Vec::new(),
});

/// Build the speech clients and remember how to change their voice and language later.
pub fn build_registered_speech(
    speech: &SpeechConfig,
) -> Result<(HttpTranscriber, HttpSynthesizer), String> {
    let (stt, tts) = build_speech(speech)?;
    let mut h = SPEECH_HANDLES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    h.voices.push(tts.voice_handle());
    h.languages.push(stt.language_handle());
    Ok((stt, tts))
}

/// Make every speech client use `voice` now, and return the voice they were using (none when there is no client).
pub fn swap_live_voice(voice: &str) -> Option<String> {
    let h = SPEECH_HANDLES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut old = None;
    for slot in &h.voices {
        let mut v = slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        old.get_or_insert_with(|| v.clone());
        *v = voice.to_owned();
    }
    old
}

/// Apply the voice and language that are now set to every speech client.
pub fn apply_speech(live: &serde_json::Map<String, serde_json::Value>) {
    let h = SPEECH_HANDLES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(v) = live
        .get("voice")
        .and_then(serde_json::Value::as_str)
        .filter(|v| !v.is_empty())
    {
        for slot in &h.voices {
            *slot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = v.to_owned();
        }
    }
    let language = live
        .get("speech_language")
        .and_then(serde_json::Value::as_str)
        .filter(|l| !l.is_empty())
        .map(str::to_owned);
    for slot in &h.languages {
        *slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = language.clone();
    }
}

pub fn build_speech(speech: &SpeechConfig) -> Result<(HttpTranscriber, HttpSynthesizer), String> {
    let missing =
        |k: &str| format!("speech.{k} is not configured (see docs/architecture/voice-pipeline.md)");
    let url = speech.url.as_deref().ok_or_else(|| missing("url"))?;
    let key = speech
        .key_env
        .as_deref()
        .map(|v| std::env::var(v).unwrap_or_default())
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
    .with_stored_key();
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
    .with_stored_key();
    Ok((stt, tts))
}
