//! Speech-to-text and text-to-speech over HTTP APIs (REQ-VOI-003, REQ-VOI-004).
//!
//! A speech service is a [`Provider`]: `openai` (the default; also Kokoro, Piper helpers, Whisper servers and
//! anything else that copies OpenAI), `elevenlabs` and `deepgram` (REQ-VOI-021). Everything specific to one
//! service lives in that service's file ([`openai`], [`elevenlabs`], [`deepgram`]): its defaults, how it takes a
//! key, how a transcription or synthesis request is built and its answer read, how its errors are worded, and
//! how it lists voices. This file holds what they share: the [`Provider`] trait, the [`Dialect`] that names one
//! in settings and configuration, and the two clients that use it.
//!
//! * [`HttpTranscriber`] uploads one utterance and reads the text back (batch mode).
//! * [`HttpSynthesizer`] asks for raw 16-bit PCM and converts it to 16 kHz.
//!
//! The HTTP itself (deadline, size cap, key handling, typed errors) is [`crate::apiclient`]; the plain-http rule
//! is [`crate::netpolicy`]; WAV files and resampling are [`crate::audio`]. Audio is never written to disk or
//! logged, and error messages never include it or the API key.
//!
//! # Adding a provider
//!
//! 1. Write `speech/<name>.rs`: a unit struct that implements [`Provider`] (and [`Wire`], unless it is
//!    OpenAI-style and can return [`crate::apiclient::Bearer`] from `wire`).
//! 2. Add `mod <name>;` here, a variant to [`Dialect`], and its arm to [`Dialect::provider`] (the only `match`
//!    on a provider), and the variant to [`Dialect::ALL`].
//! 3. Add its row to docs/guides/speech-providers.md and tests beside the others in `speech/tests.rs`.
//!
//! Nothing in the transcribe, synthesise or voice-list paths names a provider.

mod deepgram;
mod elevenlabs;
mod openai;

use crate::apiclient::{Call, Client, Wire};
use crate::secrets::Secrets;
use crate::voice::{Synthesizer, Transcriber};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

pub use crate::apiclient::ProviderError;
pub use openai::list_voices;

/// Longest text sent for synthesis in one request.
const MAX_TEXT: usize = 1_000;

/// What a provider uses when the matching `speech.*` setting is empty. `None` means the setting stays unset: the
/// OpenAI style has no default server, because the address says which server it is.
#[derive(Debug, Clone, Copy, Default)]
pub struct Defaults {
    /// The service's own address (`speech.url`).
    pub url: Option<&'static str>,
    /// The transcription model (`speech.stt_model`).
    pub stt_model: Option<&'static str>,
    /// The speech model (`speech.tts_model`). Deepgram picks its model by voice, so it is only a placeholder there.
    pub tts_model: Option<&'static str>,
    /// The voice (`speech.voice`).
    pub voice: Option<&'static str>,
    /// The environment variable the key is read from (`speech.key_env`).
    pub key_env: Option<&'static str>,
}

/// One synthesis request: what to say, in which voice and model, at which PCM rate.
#[derive(Debug, Clone, Copy)]
pub struct Speak<'a> {
    /// The speech model (`speech.tts_model`).
    pub model: &'a str,
    /// The voice for this one request.
    pub voice: &'a str,
    /// The text, already cut to the per-request limit.
    pub text: &'a str,
    /// The PCM sample rate asked of the service.
    pub rate: u32,
}

/// A list of voices, and where it came from: `server` for a list the service gave, `builtin` for a fixed one.
pub type VoiceList<'a> = Pin<
    Box<dyn Future<Output = Result<(Vec<VoiceInfo>, &'static str), ProviderError>> + Send + 'a>,
>;

/// A speech service: what it is called, what it defaults to, and how to ask it for text and for audio. Every
/// method describes one provider only; the clients below add the HTTP, the deadline and the key.
pub trait Provider: Sync {
    /// The name used in settings and the configuration file (`speech.provider`), lower case.
    fn name(&self) -> &'static str;

    /// How the service takes its key, words its errors and shapes its base address.
    fn wire(&self) -> &'static dyn Wire;

    /// What the service defaults to when a setting is empty.
    fn defaults(&self) -> Defaults {
        Defaults::default()
    }

    /// Output rates (Hz) the service can produce as raw 16-bit PCM; empty means any (the server decides).
    fn pcm_rates(&self) -> &'static [u32] {
        &[]
    }

    /// The request that transcribes `pcm` (16 kHz mono). `language` is as the person set it, or `None` to detect.
    ///
    /// # Errors
    /// A plain message when a setting cannot be put into the request.
    fn transcription(
        &self,
        model: &str,
        language: Option<&str>,
        pcm: &[i16],
    ) -> Result<Call, String>;

    /// The transcript in the service's JSON answer.
    fn transcript<'a>(&self, reply: &'a Value) -> Option<&'a str> {
        reply["text"].as_str()
    }

    /// The request that speaks `speak.text`.
    ///
    /// # Errors
    /// A plain message when a setting cannot be put into the request.
    fn synthesis(&self, speak: &Speak<'_>) -> Result<Call, String>;

    /// The voices the service offers to the key at `base_url`.
    fn voices<'a>(&'a self, base_url: &'a str, key: &'a str) -> VoiceList<'a>;
}

/// Which service's wire format is spoken (`speech.provider`, REQ-VOI-021): the small, copyable name of a
/// [`Provider`]. [`Dialect::provider`] is the one place a name becomes a provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Dialect {
    /// `POST /audio/speech` and `/audio/transcriptions` with a bearer key. The default.
    #[default]
    OpenAi,
    /// ElevenLabs: `xi-api-key`, `POST /v1/text-to-speech/{voice_id}`, `POST /v1/speech-to-text` (Scribe), voices
    /// (including cloned ones) from `GET /v2/voices`.
    ElevenLabs,
    /// Deepgram: `Authorization: Token`, `POST /v1/listen` (Nova) and `POST /v1/speak` (Aura); no voice list.
    Deepgram,
}

impl Dialect {
    /// Every dialect, in the order the names are listed to a person.
    pub const ALL: [Self; 3] = [Self::OpenAi, Self::ElevenLabs, Self::Deepgram];

    /// The provider this dialect names.
    #[must_use]
    pub fn provider(self) -> &'static dyn Provider {
        match self {
            Self::OpenAi => &openai::OpenAi,
            Self::ElevenLabs => &elevenlabs::ElevenLabs,
            Self::Deepgram => &deepgram::Deepgram,
        }
    }

    /// The dialect called `name` (case does not matter).
    ///
    /// # Errors
    /// A plain message listing the names that exist.
    pub fn parse(name: &str) -> Result<Self, String> {
        let name = name.trim().to_ascii_lowercase();
        Self::ALL
            .into_iter()
            .find(|d| d.name() == name)
            .ok_or_else(|| {
                let names: Vec<&str> = Self::ALL.iter().map(|d| d.name()).collect();
                format!("The speech provider must be one of: {}.", names.join(", "))
            })
    }

    /// The name used in settings and the configuration file.
    #[must_use]
    pub fn name(self) -> &'static str {
        self.provider().name()
    }

    /// The service's own address, used when `speech.url` is not set. The OpenAI style has none: the address says
    /// which server it is.
    #[must_use]
    pub fn default_url(self) -> Option<&'static str> {
        self.provider().defaults().url
    }

    /// The transcription model used when `speech.stt_model` is not set.
    #[must_use]
    pub fn default_stt_model(self) -> Option<&'static str> {
        self.provider().defaults().stt_model
    }

    /// The speech model used when `speech.tts_model` is not set.
    #[must_use]
    pub fn default_tts_model(self) -> Option<&'static str> {
        self.provider().defaults().tts_model
    }

    /// The voice used when `speech.voice` is not set.
    #[must_use]
    pub fn default_voice(self) -> Option<&'static str> {
        self.provider().defaults().voice
    }

    /// The environment variable the key is read from when `speech.key_env` is not set.
    #[must_use]
    pub fn default_key_env(self) -> Option<&'static str> {
        self.provider().defaults().key_env
    }
}

/// One voice a service offers: the `id` is what is sent as the voice, the `label` is what a person recognises.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceInfo {
    /// The value to use as `voice`.
    pub id: String,
    /// A display name: for a cloned voice, the name it was given.
    pub label: String,
    /// `premade`, `cloned`, `professional`, ... where the service says.
    pub category: Option<String>,
}

/// A voice or model name goes into a web address, so it is only letters, digits, dashes, underscores and dots.
fn plain_id(what: &str, v: &str) -> Result<(), String> {
    if v.is_empty()
        || v.len() > 128
        || !v
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err(format!(
            "{what} `{}` is not a name this service accepts (letters, digits, dashes and underscores only).",
            v.chars().take(40).collect::<String>()
        ));
    }
    Ok(())
}

/// Batch speech-to-text.
pub struct HttpTranscriber {
    client: Client,
    provider: &'static dyn Provider,
    model: String,
    language: Arc<Mutex<Option<String>>>,
}

impl std::fmt::Debug for HttpTranscriber {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpTranscriber")
            .field("model", &self.model)
            .finish_non_exhaustive()
    }
}

impl HttpTranscriber {
    /// A transcriber for `base_url` (for example `https://api.openai.com/v1`).
    ///
    /// # Errors
    /// A bad URL, or plain http:// to a host outside the private network.
    pub fn new(
        base_url: &str,
        key: &str,
        model: &str,
        language: Option<&str>,
        deadline: Duration,
    ) -> Result<Self, String> {
        let provider = Dialect::default().provider();
        Ok(Self {
            client: Client::new(base_url, key, deadline, provider.wire())?,
            provider,
            model: model.to_owned(),
            language: Arc::new(Mutex::new(language.map(str::to_owned))),
        })
    }

    /// A handle to change the spoken language while running (`None` means detect it). Every clone shares it.
    #[must_use]
    pub fn language_handle(&self) -> Arc<Mutex<Option<String>>> {
        Arc::clone(&self.language)
    }

    /// Speak another service's wire format (REQ-VOI-021).
    #[must_use]
    pub fn with_dialect(mut self, dialect: Dialect) -> Self {
        self.provider = dialect.provider();
        self.client.set_wire(self.provider.wire());
        self
    }

    /// Use the key pasted into the portal when there is one (REQ-VOI-023).
    #[must_use]
    pub fn with_stored_key(mut self, secrets: Secrets) -> Self {
        self.client
            .set_stored_key(secrets, crate::secrets::SPEECH_KEY);
        self
    }
}

impl Transcriber for HttpTranscriber {
    async fn transcribe(&self, pcm: &[i16]) -> Result<String, String> {
        let language = self
            .language
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let call = self
            .provider
            .transcription(&self.model, language.as_deref(), pcm)?;
        let raw = self.client.send(call).await?;
        let reply: Value = serde_json::from_slice(&raw)
            .map_err(|_| "the provider returned something that is not JSON".to_owned())?;
        self.provider
            .transcript(&reply)
            .map(|t| t.trim().to_owned())
            .ok_or_else(|| "the response has no `text`".into())
    }
}

/// Text-to-speech returning raw PCM.
pub struct HttpSynthesizer {
    client: Client,
    provider: &'static dyn Provider,
    model: String,
    voice: Arc<Mutex<String>>,
    source_rate: u32,
}

impl std::fmt::Debug for HttpSynthesizer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpSynthesizer")
            .field("model", &self.model)
            .field(
                "voice",
                &*self.voice.lock().unwrap_or_else(PoisonError::into_inner),
            )
            .finish_non_exhaustive()
    }
}

impl HttpSynthesizer {
    /// A synthesiser. `source_rate` is the sample rate of the provider's `pcm` output (OpenAI uses 24000).
    ///
    /// # Errors
    /// A bad URL, or plain http:// to a host outside the private network.
    pub fn new(
        base_url: &str,
        key: &str,
        model: &str,
        voice: &str,
        source_rate: u32,
        deadline: Duration,
    ) -> Result<Self, String> {
        if source_rate < 8_000 {
            return Err("the TTS sample rate must be at least 8000".into());
        }
        let provider = Dialect::default().provider();
        Ok(Self {
            client: Client::new(base_url, key, deadline, provider.wire())?,
            provider,
            model: model.to_owned(),
            voice: Arc::new(Mutex::new(voice.to_owned())),
            source_rate,
        })
    }

    /// A handle to change the voice while running. Every clone shares it.
    #[must_use]
    pub fn voice_handle(&self) -> Arc<Mutex<String>> {
        Arc::clone(&self.voice)
    }

    /// Speak another service's wire format (REQ-VOI-021). `source_rate` must be a PCM rate that service can produce.
    ///
    /// # Errors
    /// A rate the service cannot produce, with the rates it can.
    pub fn with_dialect(mut self, dialect: Dialect) -> Result<Self, String> {
        let provider = dialect.provider();
        let rates = provider.pcm_rates();
        if !rates.is_empty() && !rates.contains(&self.source_rate) {
            return Err(format!(
                "{} cannot produce {} Hz audio; use one of {rates:?} for `speech.tts_sample_rate`.",
                provider.wire().title(),
                self.source_rate
            ));
        }
        self.provider = provider;
        self.client.set_wire(provider.wire());
        Ok(self)
    }

    /// Use the key pasted into the portal when there is one (REQ-VOI-023).
    #[must_use]
    pub fn with_stored_key(mut self, secrets: Secrets) -> Self {
        self.client
            .set_stored_key(secrets, crate::secrets::SPEECH_KEY);
        self
    }
}

impl Synthesizer for HttpSynthesizer {
    async fn synthesize(&self, text: &str) -> Result<Vec<i16>, String> {
        let voice = self
            .voice
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        self.synthesize_voice(text, &voice).await
    }

    /// Speak with `voice` for this one call. The voice every other line uses (the shared handle) is not read or
    /// changed, so a preview can overlap a normal reply. All three providers take the voice per request.
    async fn synthesize_voice(&self, text: &str, voice: &str) -> Result<Vec<i16>, String> {
        let text: String = text.chars().take(MAX_TEXT).collect();
        let call = self.provider.synthesis(&Speak {
            model: &self.model,
            voice,
            text: &text,
            rate: self.source_rate,
        })?;
        let raw = self.client.send(call).await?;
        let samples = crate::audio::decode_pcm16(&raw);
        if samples.is_empty() {
            return Err("the provider returned no audio".into());
        }
        Ok(crate::audio::resample_to_16k(&samples, self.source_rate))
    }
}

/// Which speech server (address, the NAME of the key variable, and its dialect) the portal may ask for its voices.
/// The owning [`crate::app::Core`] keeps it.
#[derive(Debug, Clone)]
pub struct SpeechOrigin {
    url: String,
    key_env: String,
    dialect: Dialect,
}

impl SpeechOrigin {
    /// The server at `url`, whose key (when not pasted into the portal) is in the variable `key_env`.
    #[must_use]
    pub fn new(url: &str, key_env: Option<&str>, dialect: Dialect) -> Self {
        Self {
            url: url.to_owned(),
            key_env: key_env.unwrap_or("").to_owned(),
            dialect,
        }
    }

    /// The server with the key it would use now.
    #[must_use]
    pub fn resolve(&self, secrets: &Secrets, env: &crate::env::Env) -> SpeechSource {
        SpeechSource {
            url: self.url.clone(),
            key: secrets.get(crate::secrets::SPEECH_KEY).unwrap_or_else(|| {
                if self.key_env.is_empty() {
                    String::new()
                } else {
                    env.get(&self.key_env).unwrap_or_default()
                }
            }),
            dialect: self.dialect,
        }
    }
}

/// The speech server set up at start.
#[derive(Debug, Clone)]
pub struct SpeechSource {
    /// Its address.
    pub url: String,
    /// The key now in effect: one pasted into the portal, else the one in the named environment variable.
    pub key: String,
    /// Its wire format.
    pub dialect: Dialect,
}

/// The voices a service offers, as `(voices, source)` where `source` is `server` for a list the service gave and
/// `builtin` for a fixed one kept here. The OpenAI style is asked like [`list_voices`].
///
/// # Errors
/// A plain description of why the list could not be had.
pub async fn list_voice_details(
    dialect: Dialect,
    base_url: &str,
    key: &str,
) -> Result<(Vec<VoiceInfo>, &'static str), ProviderError> {
    dialect.provider().voices(base_url, key).await
}

#[cfg(test)]
mod tests;
