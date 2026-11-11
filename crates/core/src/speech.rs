//! Speech-to-text and text-to-speech over HTTP APIs (REQ-VOI-003, REQ-VOI-004).
//!
//! The wire format is a [`Dialect`]: `openai` (the default; also Kokoro, Piper helpers, Whisper servers and anything
//! else that copies OpenAI), `elevenlabs` and `deepgram` (REQ-VOI-021). Text below that says "OpenAI style"
//! describes the default dialect.
//!
//! * [`HttpTranscriber`] uploads one utterance as a WAV file to `POST {base}/audio/transcriptions` (batch mode).
//! * [`HttpSynthesizer`] asks `POST {base}/audio/speech` for raw 16-bit PCM and converts it to 16 kHz.
//!
//! Audio is sensitive (REQ-SEC-001, REQ-SEC-002). Plain `http://` is accepted only for hosts on the local
//! machine or private network, such as a self-hosted Whisper server; anything on the internet needs `https://`.
//! Audio is never written to disk or logged, and error messages never include it or the API key.

use crate::secrets::Secrets;
use crate::transport::{Connector, Endpoint};
use crate::voice::{Synthesizer, Transcriber};
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::{Method, Request};
use serde_json::{Value, json};
use std::net::IpAddr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::time::timeout;

/// Sample rate used inside ClawDot.
const RATE: u32 = 16_000;
/// Largest response accepted (about three minutes of 24 kHz audio).
const MAX_RESPONSE: usize = 9 * 1024 * 1024;
/// Longest text sent for synthesis in one request.
const MAX_TEXT: usize = 1_000;

/// Whether plain HTTP to this host stays on the local machine or private network.
#[must_use]
pub fn is_private_host(host: &str) -> bool {
    let h = host.trim_matches(['[', ']']);
    if h.eq_ignore_ascii_case("localhost") || h.to_ascii_lowercase().ends_with(".local") {
        return true;
    }
    match h.parse::<IpAddr>() {
        Ok(IpAddr::V4(a)) => a.is_loopback() || a.is_private() || a.is_link_local(),
        Ok(IpAddr::V6(a)) => a.is_loopback() || (a.segments()[0] & 0xfe00) == 0xfc00,
        Err(_) => false,
    }
}

fn check_url(url: &str) -> Result<Endpoint, String> {
    let ep = Endpoint::parse(url)?;
    if !ep.tls && !is_private_host(&ep.host) {
        return Err(format!(
            "{url}: audio must not cross the internet unencrypted; use https:// (plain http:// is for localhost and the home network)"
        ));
    }
    Ok(ep)
}

/// Encode 16 kHz mono 16-bit samples as a WAV file.
#[must_use]
pub fn wav(pcm: &[i16]) -> Vec<u8> {
    let data_len = u32::try_from(pcm.len() * 2).unwrap_or(u32::MAX - 36);
    let mut out = Vec::with_capacity(44 + pcm.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in pcm {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// Read a WAV file that is already in ClawDot's format: PCM, mono, 16-bit, 16 kHz.
///
/// # Errors
/// A message saying what is wrong with the file.
pub fn parse_wav(bytes: &[u8]) -> Result<Vec<i16>, String> {
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a WAV file".into());
    }
    let mut pos = 12;
    let mut format_ok = false;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let len = u32::from_le_bytes([
            bytes[pos + 4],
            bytes[pos + 5],
            bytes[pos + 6],
            bytes[pos + 7],
        ]) as usize;
        let body = pos + 8;
        let end = body.saturating_add(len).min(bytes.len());
        if id == b"fmt " {
            let f = &bytes[body..end];
            if f.len() < 16 {
                return Err("the WAV format block is too short".into());
            }
            let tag = u16::from_le_bytes([f[0], f[1]]);
            let channels = u16::from_le_bytes([f[2], f[3]]);
            let rate = u32::from_le_bytes([f[4], f[5], f[6], f[7]]);
            let bits = u16::from_le_bytes([f[14], f[15]]);
            if (tag, channels, rate, bits) != (1, 1, RATE, 16) {
                return Err(format!(
                    "the audio must be mono 16-bit PCM at 16000 Hz (this is format {tag}, {channels} channel(s), {rate} Hz, {bits}-bit)"
                ));
            }
            format_ok = true;
        } else if id == b"data" {
            if !format_ok {
                return Err("the WAV data comes before its format".into());
            }
            return Ok(decode_pcm16(&bytes[body..end]));
        }
        pos = body.saturating_add(len).saturating_add(len & 1);
    }
    Err("the WAV file has no audio data".into())
}

/// Decode little-endian 16-bit samples; a trailing odd byte is dropped.
#[must_use]
pub fn decode_pcm16(bytes: &[u8]) -> Vec<i16> {
    bytes
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]))
        .collect()
}

/// Convert to 16 kHz by linear interpolation. Good enough for speech playback.
#[must_use]
pub fn resample_to_16k(samples: &[i16], from_rate: u32) -> Vec<i16> {
    if from_rate == RATE || samples.is_empty() || from_rate == 0 {
        return samples.to_vec();
    }
    let out_len = (samples.len() as u64 * u64::from(RATE) / u64::from(from_rate)) as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let pos = i as u64 * u64::from(from_rate);
        let idx = (pos / u64::from(RATE)) as usize;
        let frac = (pos % u64::from(RATE)) as f64 / f64::from(RATE);
        let a = f64::from(samples[idx.min(samples.len() - 1)]);
        let b = f64::from(samples[(idx + 1).min(samples.len() - 1)]);
        #[allow(clippy::cast_possible_truncation)]
        out.push((a + (b - a) * frac).round() as i16);
    }
    out
}

fn multipart(boundary: &str, fields: &[(&str, &str)], file: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    for (k, v) in fields {
        body.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n")
                .as_bytes(),
        );
    }
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"speech.wav\"\r\nContent-Type: audio/wav\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(file);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

/// Which service's wire format is spoken (`speech.provider`, REQ-VOI-021).
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
    /// Every name `speech.provider` accepts.
    pub const NAMES: [&'static str; 3] = ["openai", "elevenlabs", "deepgram"];

    /// The dialect called `name` (case does not matter).
    ///
    /// # Errors
    /// A plain message listing the names that exist.
    pub fn parse(name: &str) -> Result<Self, String> {
        match name.trim().to_ascii_lowercase().as_str() {
            "openai" => Ok(Self::OpenAi),
            "elevenlabs" => Ok(Self::ElevenLabs),
            "deepgram" => Ok(Self::Deepgram),
            _ => Err(format!(
                "The speech provider must be one of: {}.",
                Self::NAMES.join(", ")
            )),
        }
    }

    /// The name used in settings and the configuration file.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::ElevenLabs => "elevenlabs",
            Self::Deepgram => "deepgram",
        }
    }

    /// The service's own address, used when `speech.url` is not set. The OpenAI style has none: the address says
    /// which server it is.
    #[must_use]
    pub fn default_url(self) -> Option<&'static str> {
        match self {
            Self::OpenAi => None,
            Self::ElevenLabs => Some("https://api.elevenlabs.io"),
            Self::Deepgram => Some("https://api.deepgram.com"),
        }
    }

    /// The transcription model used when `speech.stt_model` is not set.
    #[must_use]
    pub fn default_stt_model(self) -> Option<&'static str> {
        match self {
            Self::OpenAi => None,
            Self::ElevenLabs => Some("scribe_v2"),
            Self::Deepgram => Some("nova-3"),
        }
    }

    /// The speech model used when `speech.tts_model` is not set. Deepgram picks its model by voice, so this is only
    /// a placeholder there.
    #[must_use]
    pub fn default_tts_model(self) -> Option<&'static str> {
        match self {
            Self::OpenAi => None,
            Self::ElevenLabs => Some("eleven_flash_v2_5"),
            Self::Deepgram => Some("aura-2"),
        }
    }

    /// The voice used when `speech.voice` is not set.
    #[must_use]
    pub fn default_voice(self) -> Option<&'static str> {
        match self {
            Self::OpenAi => None,
            // "Rachel", a premade voice from ElevenLabs' own quick start.
            Self::ElevenLabs => Some("21m00Tcm4TlvDq8ikWAM"),
            Self::Deepgram => Some("aura-2-thalia-en"),
        }
    }

    /// The environment variable the key is read from when `speech.key_env` is not set.
    #[must_use]
    pub fn default_key_env(self) -> Option<&'static str> {
        match self {
            Self::OpenAi => None,
            Self::ElevenLabs => Some("ELEVENLABS_API_KEY"),
            Self::Deepgram => Some("DEEPGRAM_API_KEY"),
        }
    }

    fn needs_key(self) -> bool {
        self != Self::OpenAi
    }

    /// How messages name the service.
    fn title(self) -> &'static str {
        match self {
            Self::OpenAi => "the speech server",
            Self::ElevenLabs => "ElevenLabs",
            Self::Deepgram => "Deepgram",
        }
    }

    /// Output rates (Hz) the service can produce as raw 16-bit PCM.
    fn pcm_rates(self) -> &'static [u32] {
        match self {
            Self::OpenAi => &[],
            Self::ElevenLabs => &[8_000, 16_000, 22_050, 24_000, 44_100],
            Self::Deepgram => &[8_000, 16_000, 24_000, 32_000, 48_000],
        }
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

/// The message inside a provider's error body.
fn error_text(dialect: Dialect, v: &Value) -> String {
    if let Some(m) = v["error"]["message"].as_str() {
        return m.to_owned();
    }
    if dialect == Dialect::OpenAi {
        return String::new();
    }
    // ElevenLabs: {"detail":{"status","message"}}, {"detail":"text"} or {"detail":[{"msg"}]};
    // Deepgram: {"err_msg"} or {"message"}.
    [
        &v["detail"]["message"],
        &v["detail"],
        &v["detail"][0]["msg"],
        &v["err_msg"],
        &v["message"],
    ]
    .iter()
    .find_map(|x| x.as_str())
    .unwrap_or_default()
    .to_owned()
}

/// The text of a failed request. The OpenAI style keeps `HTTP <code>: <message>`; the others lead with a plain reason
/// for the failures a person can fix (the key, the limit, the plan), then the service's own words.
fn failure(dialect: Dialect, code: u16, msg: &str) -> String {
    let msg: String = msg.chars().take(200).collect();
    let reason = if dialect == Dialect::OpenAi {
        ""
    } else {
        match code {
            401 | 403 => {
                "the key was not accepted, check that it is the whole key for this service"
            }
            402 => "the plan has no credit or characters left",
            429 => "too many requests or the plan's limit is reached; wait a little and try again",
            _ => "",
        }
    };
    match (reason.is_empty(), msg.is_empty()) {
        (true, true) => format!("HTTP {code}"),
        (true, false) => format!("HTTP {code}: {msg}"),
        (false, true) => format!("HTTP {code}: {reason}"),
        (false, false) => format!("HTTP {code}: {reason} ({msg})"),
    }
}

/// Why a call to a speech or model service failed, by the kinds a caller acts on. `Display` is the text a person reads;
/// it never holds a key (the service's own message is cut to 200 characters and the key is only ever sent, not kept).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderError {
    /// There is no key to send and this service needs one.
    NoKey(String),
    /// The service answered 401 or 403: the key was not accepted.
    Unauthorized(String),
    /// Anything else: a network failure, another HTTP status, an unreadable answer.
    Other(String),
}

impl ProviderError {
    /// The key is missing or was refused, so the fix is the key and not the service.
    #[must_use]
    pub fn is_key_problem(&self) -> bool {
        matches!(self, Self::NoKey(_) | Self::Unauthorized(_))
    }

    /// The service refused the key (HTTP 401 or 403).
    #[must_use]
    pub fn is_unauthorized(&self) -> bool {
        matches!(self, Self::Unauthorized(_))
    }
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (Self::NoKey(t) | Self::Unauthorized(t) | Self::Other(t)) = self;
        f.write_str(t)
    }
}

impl std::error::Error for ProviderError {}

impl From<String> for ProviderError {
    fn from(text: String) -> Self {
        Self::Other(text)
    }
}

impl From<ProviderError> for String {
    fn from(e: ProviderError) -> Self {
        e.to_string()
    }
}

struct Client {
    endpoint: Endpoint,
    key: String,
    /// Prefer the key pasted into the portal over `key` (REQ-VOI-023).
    stored: Option<Secrets>,
    deadline: Duration,
    connector: Connector,
    dialect: Dialect,
}

impl Client {
    fn new(base_url: &str, key: &str, deadline: Duration) -> Result<Self, String> {
        Ok(Self {
            endpoint: check_url(base_url)?,
            key: key.to_owned(),
            stored: None,
            deadline,
            connector: Connector::new()?,
            dialect: Dialect::OpenAi,
        })
    }

    /// The key to send now: one pasted into the portal wins, so it takes effect without a restart.
    fn current_key(&self) -> String {
        if let Some(k) = self
            .stored
            .as_ref()
            .and_then(|s| s.get(crate::secrets::SPEECH_KEY))
        {
            return k;
        }
        self.key.clone()
    }

    async fn post(
        &self,
        path: &str,
        content_type: &str,
        body: Vec<u8>,
    ) -> Result<Vec<u8>, ProviderError> {
        self.send(Method::POST, path, content_type, body).await
    }

    async fn get(&self, path: &str) -> Result<Vec<u8>, ProviderError> {
        self.send(Method::GET, path, "application/json", Vec::new())
            .await
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        content_type: &str,
        body: Vec<u8>,
    ) -> Result<Vec<u8>, ProviderError> {
        // The other dialects' paths start with /v1 themselves; an address that already ends in it still works.
        let base = self.endpoint.base.as_str();
        let base = if self.dialect == Dialect::OpenAi {
            base
        } else {
            base.strip_suffix("/v1").unwrap_or(base)
        };
        let mut req = Request::builder()
            .method(method)
            .uri(format!("{base}{path}"))
            .header("host", &self.endpoint.host)
            .header("content-type", content_type)
            .header("connection", "close")
            .header("user-agent", "clawdot");
        let key = self.current_key();
        if key.is_empty() && self.dialect.needs_key() {
            return Err(ProviderError::NoKey(format!(
                "No key is set for {}. Paste one in the voice settings, or name an environment variable that holds it.",
                self.dialect.title()
            )));
        }
        if !key.is_empty() {
            req = match self.dialect {
                Dialect::OpenAi => req.header("authorization", format!("Bearer {key}")),
                Dialect::ElevenLabs => req.header("xi-api-key", key),
                Dialect::Deepgram => req.header("authorization", format!("Token {key}")),
            };
        }
        let request = req
            .body(Full::new(Bytes::from(body)))
            .map_err(|e| ProviderError::Other(e.to_string()))?;
        let work = async {
            let response = self.connector.request(&self.endpoint, request).await?;
            let status = response.status();
            let mut body = response.into_body();
            let mut all = Vec::new();
            while let Some(frame) = body.frame().await {
                let frame = frame.map_err(|e| format!("read: {e}"))?;
                if let Some(d) = frame.data_ref() {
                    all.extend_from_slice(d);
                    if all.len() > MAX_RESPONSE {
                        return Err(ProviderError::Other("response too large".to_owned()));
                    }
                }
            }
            if status.is_success() {
                Ok(all)
            } else {
                // Provider error bodies are JSON with a message; show that, never anything else.
                let msg = serde_json::from_slice::<Value>(&all)
                    .ok()
                    .map(|v| error_text(self.dialect, &v))
                    .unwrap_or_default();
                let text = failure(self.dialect, status.as_u16(), &msg);
                Err(match status.as_u16() {
                    401 | 403 => ProviderError::Unauthorized(text),
                    _ => ProviderError::Other(text),
                })
            }
        };
        timeout(self.deadline, work).await.map_err(|_| {
            ProviderError::Other(format!("no response within {} s", self.deadline.as_secs()))
        })?
    }
}

/// Batch speech-to-text.
pub struct HttpTranscriber {
    client: Client,
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
        Ok(Self {
            client: Client::new(base_url, key, deadline)?,
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
        self.client.dialect = dialect;
        self
    }

    /// Use the key pasted into the portal when there is one (REQ-VOI-023).
    #[must_use]
    pub fn with_stored_key(mut self, secrets: Secrets) -> Self {
        self.client.stored = Some(secrets);
        self
    }

    async fn transcribe_elevenlabs(
        &self,
        pcm: &[i16],
        language: Option<&str>,
    ) -> Result<String, String> {
        let boundary = format!(
            "clawdot-boundary-{:016x}",
            pcm.len() as u64 ^ 0x9e37_79b9_7f4a_7c15
        );
        let mut fields = vec![("model_id", self.model.as_str())];
        // ElevenLabs takes ISO 639 codes, so `pt-BR` is sent as `pt`.
        let code = language.map(|l| l.split('-').next().unwrap_or(l).to_owned());
        if let Some(c) = &code {
            fields.push(("language_code", c.as_str()));
        }
        let body = multipart(&boundary, &fields, &wav(pcm));
        let raw = self
            .client
            .post(
                "/v1/speech-to-text",
                &format!("multipart/form-data; boundary={boundary}"),
                body,
            )
            .await?;
        text_of(&raw, |v| v["text"].as_str())
    }

    async fn transcribe_deepgram(
        &self,
        pcm: &[i16],
        language: Option<&str>,
    ) -> Result<String, String> {
        plain_id("The transcription model", &self.model)?;
        let lang = match language {
            Some(l) => {
                plain_id("The language", l)?;
                format!("language={l}")
            }
            None => "detect_language=true".to_owned(),
        };
        let path = format!("/v1/listen?model={}&smart_format=true&{lang}", self.model);
        let raw = self.client.post(&path, "audio/wav", wav(pcm)).await?;
        text_of(&raw, |v| {
            v["results"]["channels"][0]["alternatives"][0]["transcript"].as_str()
        })
    }
}

/// The text a provider's JSON answer holds, found by `pick`.
fn text_of(raw: &[u8], pick: impl Fn(&Value) -> Option<&str>) -> Result<String, String> {
    let v: Value = serde_json::from_slice(raw)
        .map_err(|_| "the provider returned something that is not JSON".to_owned())?;
    pick(&v)
        .map(|t| t.trim().to_owned())
        .ok_or_else(|| "the response has no `text`".into())
}

impl Transcriber for HttpTranscriber {
    async fn transcribe(&self, pcm: &[i16]) -> Result<String, String> {
        let language = self
            .language
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        match self.client.dialect {
            Dialect::OpenAi => {}
            Dialect::ElevenLabs => {
                return self.transcribe_elevenlabs(pcm, language.as_deref()).await;
            }
            Dialect::Deepgram => return self.transcribe_deepgram(pcm, language.as_deref()).await,
        }
        // The boundary only needs to be absent from the data; WAV bytes could contain anything, so mix in the length.
        let boundary = format!(
            "clawdot-boundary-{:016x}",
            pcm.len() as u64 ^ 0x9e37_79b9_7f4a_7c15
        );
        let mut fields = vec![("model", self.model.as_str()), ("response_format", "json")];
        if let Some(l) = &language {
            fields.push(("language", l.as_str()));
        }
        let body = multipart(&boundary, &fields, &wav(pcm));
        let raw = self
            .client
            .post(
                "/audio/transcriptions",
                &format!("multipart/form-data; boundary={boundary}"),
                body,
            )
            .await?;
        let v: Value = serde_json::from_slice(&raw)
            .map_err(|_| "the provider returned something that is not JSON".to_owned())?;
        v["text"]
            .as_str()
            .map(|t| t.trim().to_owned())
            .ok_or_else(|| "the response has no `text`".into())
    }
}

/// Text-to-speech returning raw PCM.
pub struct HttpSynthesizer {
    client: Client,
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
        Ok(Self {
            client: Client::new(base_url, key, deadline)?,
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
        let rates = dialect.pcm_rates();
        if !rates.is_empty() && !rates.contains(&self.source_rate) {
            return Err(format!(
                "{} cannot produce {} Hz audio; use one of {rates:?} for `speech.tts_sample_rate`.",
                dialect.title(),
                self.source_rate
            ));
        }
        self.client.dialect = dialect;
        Ok(self)
    }

    /// Use the key pasted into the portal when there is one (REQ-VOI-023).
    #[must_use]
    pub fn with_stored_key(mut self, secrets: Secrets) -> Self {
        self.client.stored = Some(secrets);
        self
    }
}

impl Synthesizer for HttpSynthesizer {
    async fn synthesize(&self, text: &str) -> Result<Vec<i16>, String> {
        let text: String = text.chars().take(MAX_TEXT).collect();
        let voice = self
            .voice
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let (path, body) = match self.client.dialect {
            Dialect::OpenAi => (
                "/audio/speech".to_owned(),
                json!({ "model": self.model, "voice": voice, "input": text, "response_format": "pcm" }),
            ),
            Dialect::ElevenLabs => {
                plain_id("The voice", &voice)?;
                (
                    format!(
                        "/v1/text-to-speech/{voice}?output_format=pcm_{}",
                        self.source_rate
                    ),
                    json!({ "text": text, "model_id": self.model }),
                )
            }
            Dialect::Deepgram => {
                // The voice is the model: `aura-2-thalia-en`.
                plain_id("The voice", &voice)?;
                (
                    format!(
                        "/v1/speak?model={voice}&encoding=linear16&container=none&sample_rate={}",
                        self.source_rate
                    ),
                    json!({ "text": text }),
                )
            }
        };
        let raw = self
            .client
            .post(&path, "application/json", body.to_string().into_bytes())
            .await?;
        let samples = decode_pcm16(&raw);
        if samples.is_empty() {
            return Err("the provider returned no audio".into());
        }
        Ok(resample_to_16k(&samples, self.source_rate))
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
    pub fn resolve(&self, secrets: &Secrets) -> SpeechSource {
        SpeechSource {
            url: self.url.clone(),
            key: secrets.get(crate::secrets::SPEECH_KEY).unwrap_or_else(|| {
                if self.key_env.is_empty() {
                    String::new()
                } else {
                    std::env::var(&self.key_env).unwrap_or_default()
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

/// Deepgram has no list to ask for; these are its Aura 2 English voices (the model name is the voice).
const DEEPGRAM_VOICES: [(&str, &str); 12] = [
    ("aura-2-thalia-en", "Thalia"),
    ("aura-2-andromeda-en", "Andromeda"),
    ("aura-2-helena-en", "Helena"),
    ("aura-2-luna-en", "Luna"),
    ("aura-2-athena-en", "Athena"),
    ("aura-2-aurora-en", "Aurora"),
    ("aura-2-apollo-en", "Apollo"),
    ("aura-2-arcas-en", "Arcas"),
    ("aura-2-orion-en", "Orion"),
    ("aura-2-atlas-en", "Atlas"),
    ("aura-2-zeus-en", "Zeus"),
    ("aura-2-hermes-en", "Hermes"),
];

/// The voices ElevenLabs lists for this key, cloned ones included, with their names. Pages of 100, at most 10 pages.
async fn elevenlabs_voices(base_url: &str, key: &str) -> Result<Vec<VoiceInfo>, ProviderError> {
    let mut c = Client::new(base_url, key, Duration::from_secs(10))?;
    c.dialect = Dialect::ElevenLabs;
    let mut out = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..10 {
        let mut path = "/v2/voices?page_size=100&include_total_count=false".to_owned();
        if let Some(t) = &token {
            plain_id("The page token", t)?;
            path.push_str("&next_page_token=");
            path.push_str(t);
        }
        let raw = c.get(&path).await?;
        let v: Value = serde_json::from_slice(&raw)
            .map_err(|_| ProviderError::from("the list of voices is not readable".to_owned()))?;
        for x in v["voices"].as_array().into_iter().flatten() {
            let (Some(id), name) = (x["voice_id"].as_str(), x["name"].as_str()) else {
                continue;
            };
            if id.is_empty() || id.len() > 128 {
                continue;
            }
            let label: String = name.unwrap_or(id).chars().take(128).collect();
            out.push(VoiceInfo {
                id: id.to_owned(),
                label,
                category: x["category"].as_str().map(|s| s.chars().take(32).collect()),
            });
        }
        token = v["next_page_token"].as_str().map(str::to_owned);
        if v["has_more"].as_bool() != Some(true) || token.is_none() {
            break;
        }
    }
    out.sort_by_key(|a| (a.label.to_lowercase(), a.id.clone()));
    out.dedup_by(|a, b| a.id == b.id);
    out.truncate(1000);
    if out.is_empty() {
        return Err("the service did not list any voices".to_owned().into());
    }
    Ok(out)
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
    match dialect {
        Dialect::OpenAi => {
            let names = list_voices(base_url, key).await?;
            Ok((
                names
                    .into_iter()
                    .map(|n| VoiceInfo {
                        label: n.clone(),
                        id: n,
                        category: None,
                    })
                    .collect(),
                "server",
            ))
        }
        Dialect::ElevenLabs => Ok((elevenlabs_voices(base_url, key).await?, "server")),
        Dialect::Deepgram => Ok((
            DEEPGRAM_VOICES
                .iter()
                .map(|(id, label)| VoiceInfo {
                    id: (*id).to_owned(),
                    label: (*label).to_owned(),
                    category: None,
                })
                .collect(),
            "builtin",
        )),
    }
}

/// The names in a list a server returns: a plain array, or an object holding one under `voices`, `data` or `models`,
/// whose entries are strings or objects with an `id`, `name`, `voice_id` or `voice`. Sorted, without repeats, at most
/// 1000, each at most 128 characters.
#[must_use]
pub fn names_in(v: &Value) -> Vec<String> {
    let items = v
        .as_array()
        .or_else(|| v["voices"].as_array())
        .or_else(|| v["data"].as_array())
        .or_else(|| v["models"].as_array());
    let mut names: Vec<String> = items
        .map(|a| {
            a.iter()
                .filter_map(|x| {
                    x.as_str().map(str::to_owned).or_else(|| {
                        ["id", "name", "voice_id", "voice"]
                            .iter()
                            .find_map(|k| x[*k].as_str().map(str::to_owned))
                    })
                })
                .filter(|n| !n.is_empty() && n.len() <= 128)
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names.dedup();
    names.truncate(1000);
    names
}

async fn list_at(
    base_url: &str,
    key: &str,
    path: &str,
    what: &str,
) -> Result<Vec<String>, ProviderError> {
    let c = Client::new(base_url, key, Duration::from_secs(10))?;
    let raw = c.get(path).await?;
    let v: Value = serde_json::from_slice(&raw).map_err(|_| {
        ProviderError::Other(format!("the server's list of {what} is not readable"))
    })?;
    let names = names_in(&v);
    if names.is_empty() {
        return Err(ProviderError::Other(format!(
            "the server did not list any {what}"
        )));
    }
    Ok(names)
}

/// The voices a speech server offers, from `GET <base>/audio/voices` or the other places servers put them (not every server has a list).
///
/// # Errors
/// A plain description of why the list could not be had.
pub async fn list_voices(base_url: &str, key: &str) -> Result<Vec<String>, ProviderError> {
    // Servers that list their voices put the list in a few different places: OpenAI-compatible ones under
    // /audio/voices (Kokoro, the Piper helper), others at /voices or /audio/speech/voices.
    let mut first = None;
    for path in ["/audio/voices", "/voices", "/audio/speech/voices"] {
        match list_at(base_url, key, path, "voices").await {
            Ok(v) => return Ok(v),
            Err(e) => {
                first.get_or_insert(e);
            }
        }
    }
    Err(first.unwrap_or_else(|| ProviderError::Other("no list".to_owned())))
}

/// The models a language-model server offers, from `GET <base>/models` (the OpenAI-style list that OpenRouter, Ollama
/// and most others serve).
///
/// # Errors
/// A plain description of why the list could not be had.
pub async fn list_models(base_url: &str, key: &str) -> Result<Vec<String>, ProviderError> {
    list_at(base_url, key, "/models", "models").await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn plain_http_is_only_for_the_home_network() {
        for h in [
            "localhost",
            "127.0.0.1",
            "[::1]",
            "192.168.1.5",
            "10.0.0.2",
            "172.16.4.4",
            "dot.local",
            "169.254.1.1",
            "fd00::1",
        ] {
            assert!(is_private_host(h), "{h}");
        }
        for h in [
            "api.openai.com",
            "8.8.8.8",
            "172.32.0.1",
            "example.com",
            "2001:db8::1",
        ] {
            assert!(!is_private_host(h), "{h}");
        }
        assert!(check_url("https://api.openai.com/v1").is_ok());
        assert!(check_url("http://192.168.1.9:9000/v1").is_ok());
        let e = check_url("http://api.openai.com/v1").unwrap_err();
        assert!(e.contains("https://"), "{e}");
    }

    #[test]
    fn wav_files_round_trip_and_wrong_formats_are_explained() {
        let samples = vec![5, -5, 300, -300];
        assert_eq!(parse_wav(&wav(&samples)).unwrap(), samples);
        assert!(parse_wav(b"hello").unwrap_err().contains("not a WAV"));
        let mut stereo = wav(&samples);
        stereo[22] = 2;
        let e = parse_wav(&stereo).unwrap_err();
        assert!(
            e.contains("mono 16-bit PCM at 16000 Hz") && e.contains("2 channel"),
            "{e}"
        );
        let mut truncated = wav(&samples);
        truncated.truncate(30);
        assert!(parse_wav(&truncated).is_err());
        let mut no_data = wav(&samples);
        no_data.truncate(36);
        assert!(parse_wav(&no_data).unwrap_err().contains("no audio"));
    }

    #[test]
    fn wav_has_a_valid_header() {
        let w = wav(&[1, -2, 3]);
        assert_eq!(&w[..4], b"RIFF");
        assert_eq!(&w[8..16], b"WAVEfmt ");
        assert_eq!(u32::from_le_bytes(w[24..28].try_into().unwrap()), 16_000);
        assert_eq!(u32::from_le_bytes(w[40..44].try_into().unwrap()), 6);
        assert_eq!(w.len(), 44 + 6);
        assert_eq!(decode_pcm16(&w[44..]), vec![1, -2, 3]);
    }

    #[test]
    fn resampling_changes_length_by_the_ratio_and_keeps_the_shape() {
        let up: Vec<i16> = (0..2400).map(|i| (i % 100) as i16 * 100).collect();
        let down = resample_to_16k(&up, 24_000);
        assert_eq!(down.len(), 1600);
        assert_eq!(down[0], up[0]);
        assert_eq!(resample_to_16k(&up, 16_000), up);
        assert!(resample_to_16k(&[], 24_000).is_empty());
        // 8 kHz input is stretched to twice the length.
        assert_eq!(resample_to_16k(&[0, 100, 200], 8_000).len(), 6);
    }

    #[test]
    fn multipart_has_fields_then_the_file() {
        let b = multipart("XX", &[("model", "m")], b"DATA");
        let s = String::from_utf8(b).unwrap();
        assert!(
            s.starts_with("--XX\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\nm\r\n")
        );
        assert!(s.contains("name=\"file\"; filename=\"speech.wav\""));
        assert!(s.ends_with("DATA\r\n--XX--\r\n"));
    }

    /// One canned response; returns the request head and body.
    async fn serve_once(
        status: &'static str,
        content_type: &'static str,
        body: Vec<u8>,
    ) -> (u16, tokio::task::JoinHandle<(String, Vec<u8>)>) {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let h = tokio::spawn(async move {
            let (mut c, _) = l.accept().await.unwrap();
            let mut raw = Vec::new();
            let mut buf = [0u8; 8192];
            let (head, req_body) = loop {
                let n = c.read(&mut buf).await.unwrap();
                raw.extend_from_slice(&buf[..n]);
                if let Some(p) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&raw[..p]).into_owned();
                    let want = head
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if raw.len() >= p + 4 + want {
                        break (head, raw[p + 4..p + 4 + want].to_vec());
                    }
                }
                assert!(n > 0, "client hung up early");
            };
            let mut resp = format!("HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes();
            resp.extend_from_slice(&body);
            c.write_all(&resp).await.unwrap();
            (head, req_body)
        });
        (port, h)
    }

    const T: Duration = Duration::from_secs(5);

    #[tokio::test]
    async fn a_transcription_uploads_a_wav_and_reads_the_text() {
        let (port, server) = serve_once(
            "200 OK",
            "application/json",
            br#"{"text":" turn on the light "}"#.to_vec(),
        )
        .await;
        let t = HttpTranscriber::new(
            &format!("http://127.0.0.1:{port}/v1"),
            "k3y",
            "whisper-1",
            Some("en"),
            T,
        )
        .unwrap();
        let text = t.transcribe(&[10, 20, 30, 40]).await.unwrap();
        assert_eq!(text, "turn on the light");
        let (head, body) = server.await.unwrap();
        assert!(
            head.starts_with("POST /v1/audio/transcriptions HTTP/1.1"),
            "{head}"
        );
        let lower = head.to_ascii_lowercase();
        assert!(
            lower.contains("authorization: bearer k3y")
                && lower.contains("multipart/form-data; boundary=")
        );
        let text = String::from_utf8_lossy(&body);
        assert!(
            text.contains("name=\"model\"\r\n\r\nwhisper-1")
                && text.contains("name=\"language\"\r\n\r\nen")
        );
        assert!(
            body.windows(4).any(|w| w == b"RIFF"),
            "the audio is sent as a WAV file"
        );
    }

    #[tokio::test]
    async fn transcription_failures_have_clear_messages_without_secrets() {
        let (port, _s) = serve_once(
            "401 Unauthorized",
            "application/json",
            br#"{"error":{"message":"bad key"}}"#.to_vec(),
        )
        .await;
        let t =
            HttpTranscriber::new(&format!("http://127.0.0.1:{port}"), "k3y", "m", None, T).unwrap();
        let e = t.transcribe(&[1]).await.unwrap_err();
        assert_eq!(e, "HTTP 401: bad key");
        assert!(!e.contains("k3y"));
        let (port, _s) = serve_once("200 OK", "text/plain", b"hello".to_vec()).await;
        let t =
            HttpTranscriber::new(&format!("http://127.0.0.1:{port}"), "", "m", None, T).unwrap();
        assert!(t.transcribe(&[1]).await.unwrap_err().contains("not JSON"));
        let (port, _s) = serve_once("200 OK", "application/json", b"{}".to_vec()).await;
        let t =
            HttpTranscriber::new(&format!("http://127.0.0.1:{port}"), "", "m", None, T).unwrap();
        assert!(t.transcribe(&[1]).await.unwrap_err().contains("no `text`"));
    }

    #[tokio::test]
    async fn synthesis_asks_for_pcm_and_returns_16k_audio() {
        let pcm24: Vec<u8> = (0..2400i16).flat_map(|i| (i * 3).to_le_bytes()).collect();
        let (port, server) = serve_once("200 OK", "audio/pcm", pcm24).await;
        let s = HttpSynthesizer::new(
            &format!("http://127.0.0.1:{port}/v1"),
            "",
            "tts-1",
            "alloy",
            24_000,
            T,
        )
        .unwrap();
        let audio = s.synthesize("Hello there.").await.unwrap();
        assert_eq!(audio.len(), 1600);
        let (head, body) = server.await.unwrap();
        assert!(head.starts_with("POST /v1/audio/speech HTTP/1.1"), "{head}");
        let sent: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(sent["input"], "Hello there.");
        assert_eq!(sent["voice"], "alloy");
        assert_eq!(sent["response_format"], "pcm");
        assert!(
            !head.to_ascii_lowercase().contains("authorization"),
            "no key configured, none sent"
        );
    }

    // REQ-CFG-080
    #[tokio::test]
    async fn the_voice_and_language_can_change_while_running() {
        let pcm: Vec<u8> = (0..2400i16).flat_map(|i| i.to_le_bytes()).collect();
        let (port, server) = serve_once("200 OK", "audio/pcm", pcm).await;
        let s = HttpSynthesizer::new(
            &format!("http://127.0.0.1:{port}/v1"),
            "",
            "tts-1",
            "alloy",
            24_000,
            T,
        )
        .unwrap();
        *s.voice_handle().lock().unwrap() = "nova".into();
        s.synthesize("Hi.").await.unwrap();
        let (_, body) = server.await.unwrap();
        let sent: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            sent["voice"], "nova",
            "the new voice is used from the next request"
        );

        let (port, server) =
            serve_once("200 OK", "application/json", br#"{"text":"hi"}"#.to_vec()).await;
        let t = HttpTranscriber::new(&format!("http://127.0.0.1:{port}"), "", "m", Some("en"), T)
            .unwrap();
        *t.language_handle().lock().unwrap() = Some("de".into());
        t.transcribe(&[1, 2, 3]).await.unwrap();
        let (_, body) = server.await.unwrap();
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("name=\"language\"\r\n\r\nde"), "{text}");
    }

    // REQ-CFG-081
    #[test]
    fn lists_of_names_are_read_in_every_common_shape() {
        use serde_json::json;
        assert_eq!(names_in(&json!(["b", "a", "a"])), ["a", "b"]);
        assert_eq!(
            names_in(&json!({"voices": [{"id": "x"}, {"name": "y"}, {"voice_id": "z"}]})),
            ["x", "y", "z"]
        );
        assert_eq!(
            names_in(&json!({"data": [{"id": "gpt-a"}, {"id": "gpt-b"}], "object": "list"})),
            ["gpt-a", "gpt-b"]
        );
        assert_eq!(names_in(&json!({"models": ["m"]})), ["m"]);
        assert!(names_in(&json!({"error": "no"})).is_empty());
        assert!(
            names_in(&json!([1, 2, {"other": "x"}, ""])).is_empty(),
            "only names are taken"
        );
        let long = "x".repeat(129);
        assert!(
            names_in(&json!([long])).is_empty(),
            "a name that is too long is dropped"
        );
    }

    // REQ-CFG-081
    #[tokio::test]
    async fn a_server_list_is_fetched_and_a_missing_one_says_so() {
        let (port, server) = serve_once(
            "200 OK",
            "application/json",
            br#"{"data":[{"id":"m2"},{"id":"m1"}]}"#.to_vec(),
        )
        .await;
        let got = list_models(&format!("http://127.0.0.1:{port}/v1"), "")
            .await
            .unwrap();
        assert_eq!(got, ["m1", "m2"]);
        let (head, _) = server.await.unwrap();
        assert!(head.starts_with("GET /v1/models HTTP/1.1"), "{head}");
        let (port, _s) = serve_once("404 Not Found", "application/json", b"{}".to_vec()).await;
        let e = list_voices(&format!("http://127.0.0.1:{port}/v1"), "")
            .await
            .unwrap_err();
        assert!(e.to_string().contains("404"), "{e}");
        let (port, _s) = serve_once("200 OK", "application/json", b"[]".to_vec()).await;
        let e = list_voices(&format!("http://127.0.0.1:{port}/v1"), "")
            .await
            .unwrap_err();
        assert!(e.to_string().contains("did not list any voices"), "{e}");
    }

    // REQ-VOI-021
    #[test]
    fn providers_are_named_and_have_their_own_defaults() {
        assert_eq!(Dialect::parse(" ElevenLabs ").unwrap(), Dialect::ElevenLabs);
        assert_eq!(Dialect::parse("deepgram").unwrap(), Dialect::Deepgram);
        assert_eq!(Dialect::parse("openai").unwrap(), Dialect::OpenAi);
        let e = Dialect::parse("acme").unwrap_err();
        assert!(e.contains("openai, elevenlabs, deepgram"), "{e}");
        for d in [Dialect::ElevenLabs, Dialect::Deepgram] {
            assert!(d.default_url().unwrap().starts_with("https://"));
            assert!(d.default_voice().is_some() && d.default_key_env().is_some());
            assert_eq!(Dialect::parse(d.name()).unwrap(), d);
        }
        assert!(
            Dialect::OpenAi.default_url().is_none(),
            "no default server for the OpenAI style"
        );
        // A voice goes into a web address: nothing that could change the path or add a query.
        assert!(plain_id("v", "21m00Tcm4TlvDq8ikWAM").is_ok());
        assert!(plain_id("v", "aura-2-thalia-en").is_ok());
        for bad in ["", "a/b", "a?x=1", "a b", "../x", "a&b"] {
            assert!(plain_id("The voice", bad).is_err(), "{bad}");
        }
    }

    // REQ-VOI-021
    #[test]
    fn error_texts_are_plain_and_keep_the_openai_style_as_it_was() {
        assert_eq!(
            failure(Dialect::OpenAi, 401, "bad key"),
            "HTTP 401: bad key"
        );
        assert_eq!(failure(Dialect::OpenAi, 429, ""), "HTTP 429");
        let e = failure(Dialect::ElevenLabs, 401, "Invalid API key");
        assert!(
            e.starts_with("HTTP 401: the key was not accepted") && e.ends_with("(Invalid API key)"),
            "{e}"
        );
        assert!(failure(Dialect::Deepgram, 429, "").contains("wait a little"));
        assert_eq!(failure(Dialect::Deepgram, 500, "boom"), "HTTP 500: boom");
        let v = json!({"detail": {"status": "invalid_api_key", "message": "Invalid API key"}});
        assert_eq!(error_text(Dialect::ElevenLabs, &v), "Invalid API key");
        assert_eq!(
            error_text(Dialect::ElevenLabs, &json!({"detail": "plain"})),
            "plain"
        );
        assert_eq!(
            error_text(Dialect::ElevenLabs, &json!({"detail": [{"msg": "bad"}]})),
            "bad"
        );
        assert_eq!(
            error_text(
                Dialect::Deepgram,
                &json!({"err_msg": "Invalid credentials"})
            ),
            "Invalid credentials"
        );
        assert_eq!(error_text(Dialect::OpenAi, &json!({"message": "x"})), "");
    }

    /// Answer each request in turn with `responses`; returns the request heads and bodies.
    async fn serve_seq(
        responses: Vec<(&'static str, Vec<u8>)>,
    ) -> (u16, tokio::task::JoinHandle<Vec<(String, Vec<u8>)>>) {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let h = tokio::spawn(async move {
            let mut seen = Vec::new();
            for (status, body) in responses {
                let (mut c, _) = l.accept().await.unwrap();
                let mut raw = Vec::new();
                let mut buf = [0u8; 8192];
                let (head, req_body) = loop {
                    let n = c.read(&mut buf).await.unwrap();
                    raw.extend_from_slice(&buf[..n]);
                    if let Some(p) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&raw[..p]).into_owned();
                        let want = head
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        if raw.len() >= p + 4 + want {
                            break (head, raw[p + 4..p + 4 + want].to_vec());
                        }
                    }
                    assert!(n > 0, "client hung up early");
                };
                let mut resp = format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes();
                resp.extend_from_slice(&body);
                c.write_all(&resp).await.unwrap();
                seen.push((head, req_body));
            }
            seen
        });
        (port, h)
    }

    fn pcm_bytes(n: i16) -> Vec<u8> {
        (0..n).flat_map(|i| (i * 3).to_le_bytes()).collect()
    }

    fn eleven(port: u16, key: &str, rate: u32) -> HttpSynthesizer {
        HttpSynthesizer::new(
            &format!("http://127.0.0.1:{port}"),
            key,
            "eleven_flash_v2_5",
            "21m00Tcm4TlvDq8ikWAM",
            rate,
            T,
        )
        .unwrap()
        .with_dialect(Dialect::ElevenLabs)
        .unwrap()
    }

    // REQ-VOI-021
    #[tokio::test]
    async fn elevenlabs_speaks_with_its_key_header_and_gives_16k_audio() {
        let (port, server) = serve_once("200 OK", "audio/pcm", pcm_bytes(2400)).await;
        let s = eleven(port, "xi-secret", 24_000);
        let audio = s.synthesize("Hello there.").await.unwrap();
        assert_eq!(audio.len(), 1600, "24 kHz becomes 16 kHz");
        let (head, body) = server.await.unwrap();
        assert!(
            head.starts_with(
                "POST /v1/text-to-speech/21m00Tcm4TlvDq8ikWAM?output_format=pcm_24000 HTTP/1.1"
            ),
            "{head}"
        );
        let lower = head.to_ascii_lowercase();
        assert!(lower.contains("xi-api-key: xi-secret"), "{head}");
        assert!(!lower.contains("authorization"), "{head}");
        let sent: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(sent["text"], "Hello there.");
        assert_eq!(sent["model_id"], "eleven_flash_v2_5");
        // A voice changed while running is the id used next, and an odd id never reaches the address.
        *s.voice_handle().lock().unwrap() = "../admin".into();
        let e = s.synthesize("x").await.unwrap_err();
        assert!(e.contains("not a name this service accepts"), "{e}");
        // 22.05 kHz is asked for as such and converted.
        let (port, server) = serve_once("200 OK", "audio/pcm", pcm_bytes(2205)).await;
        let audio = eleven(port, "k", 22_050).synthesize("x").await.unwrap();
        assert_eq!(audio.len(), 1600);
        assert!(server.await.unwrap().0.contains("output_format=pcm_22050"));
        // A rate ElevenLabs cannot make is refused when set up.
        let bad = HttpSynthesizer::new("http://127.0.0.1:1", "k", "m", "v", 48_000, T)
            .unwrap()
            .with_dialect(Dialect::ElevenLabs);
        assert!(bad.unwrap_err().contains("22050"));
    }

    // REQ-VOI-021
    #[tokio::test]
    async fn elevenlabs_failures_are_plain_and_a_missing_key_is_caught_first() {
        let (port, _s) = serve_once(
            "401 Unauthorized",
            "application/json",
            br#"{"detail":{"status":"invalid_api_key","message":"Invalid API key"}}"#.to_vec(),
        )
        .await;
        let e = eleven(port, "xi-secret", 24_000)
            .synthesize("x")
            .await
            .unwrap_err();
        assert!(
            e.starts_with("HTTP 401: the key was not accepted") && e.contains("Invalid API key"),
            "{e}"
        );
        assert!(!e.contains("xi-secret"));
        let (port, _s) = serve_once(
            "429 Too Many Requests",
            "application/json",
            br#"{"detail":{"status":"too_many_concurrent_requests","message":"Too many concurrent requests"}}"#.to_vec(),
        )
        .await;
        let e = eleven(port, "k", 24_000).synthesize("x").await.unwrap_err();
        assert!(
            e.starts_with("HTTP 429: too many requests") && e.contains("concurrent"),
            "{e}"
        );
        // No key at all: said before anything is sent.
        let e = eleven(1, "", 24_000).synthesize("x").await.unwrap_err();
        assert!(e.contains("No key is set for ElevenLabs"), "{e}");
    }

    // REQ-VOI-021
    #[tokio::test]
    async fn elevenlabs_transcribes_with_scribe_and_a_two_letter_language() {
        let (port, server) = serve_once(
            "200 OK",
            "application/json",
            br#"{"language_code":"en","text":" lights on "}"#.to_vec(),
        )
        .await;
        let t = HttpTranscriber::new(
            &format!("http://127.0.0.1:{port}"),
            "xi-k",
            "scribe_v2",
            Some("pt-BR"),
            T,
        )
        .unwrap()
        .with_dialect(Dialect::ElevenLabs);
        assert_eq!(t.transcribe(&[1, 2, 3, 4]).await.unwrap(), "lights on");
        let (head, body) = server.await.unwrap();
        assert!(
            head.starts_with("POST /v1/speech-to-text HTTP/1.1"),
            "{head}"
        );
        assert!(head.to_ascii_lowercase().contains("xi-api-key: xi-k"));
        let text = String::from_utf8_lossy(&body);
        assert!(
            text.contains("name=\"model_id\"\r\n\r\nscribe_v2")
                && text.contains("name=\"language_code\"\r\n\r\npt"),
            "{text}"
        );
        assert!(body.windows(4).any(|w| w == b"RIFF"));
    }

    // REQ-VOI-021, REQ-VOI-022
    #[tokio::test]
    async fn elevenlabs_voices_include_cloned_ones_by_name_across_pages() {
        let page1 = br#"{"voices":[{"voice_id":"v2","name":"Zed","category":"premade"},{"voice_id":"v1","name":"My Mum","category":"cloned"}],"has_more":true,"next_page_token":"tok_2"}"#.to_vec();
        let page2 = br#"{"voices":[{"voice_id":"v3","name":"Ana","category":"professional"},{"voice_id":"v1","name":"My Mum","category":"cloned"}],"has_more":false,"next_page_token":null}"#.to_vec();
        let (port, server) = serve_seq(vec![("200 OK", page1), ("200 OK", page2)]).await;
        let (voices, source) = list_voice_details(
            Dialect::ElevenLabs,
            &format!("http://127.0.0.1:{port}"),
            "xi-k",
        )
        .await
        .unwrap();
        assert_eq!(source, "server");
        let got: Vec<(&str, &str, &str)> = voices
            .iter()
            .map(|v| {
                (
                    v.id.as_str(),
                    v.label.as_str(),
                    v.category.as_deref().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            got,
            [
                ("v3", "Ana", "professional"),
                ("v1", "My Mum", "cloned"),
                ("v2", "Zed", "premade")
            ],
            "sorted by name, the name shown while the id is what is sent, no repeats"
        );
        let seen = server.await.unwrap();
        assert!(
            seen[0].0.starts_with("GET /v2/voices?page_size=100"),
            "{}",
            seen[0].0
        );
        assert!(seen[1].0.contains("next_page_token=tok_2"), "{}", seen[1].0);
        assert!(seen[0].0.to_ascii_lowercase().contains("xi-api-key: xi-k"));
        // A bad key is a plain failure; an empty list is said too.
        let (port, _s) = serve_once(
            "401 Unauthorized",
            "application/json",
            br#"{"detail":{"message":"Invalid API key"}}"#.to_vec(),
        )
        .await;
        let e = list_voice_details(
            Dialect::ElevenLabs,
            &format!("http://127.0.0.1:{port}"),
            "bad",
        )
        .await
        .unwrap_err();
        assert!(
            e.is_unauthorized()
                && e.to_string().contains("401")
                && e.to_string().contains("key was not accepted"),
            "{e}"
        );
        let (port, _s) = serve_once(
            "200 OK",
            "application/json",
            br#"{"voices":[],"has_more":false}"#.to_vec(),
        )
        .await;
        let e = list_voice_details(
            Dialect::ElevenLabs,
            &format!("http://127.0.0.1:{port}"),
            "k",
        )
        .await
        .unwrap_err();
        assert!(e.to_string().contains("did not list any voices"), "{e}");
    }

    #[test]
    fn provider_error_text_is_the_text_shown_and_kinds_are_exclusive() {
        let a = ProviderError::Unauthorized("HTTP 401: x".into());
        let n = ProviderError::NoKey("No key is set for X.".into());
        let o = ProviderError::Other("HTTP 500".into());
        assert_eq!(
            (a.to_string(), n.to_string(), o.to_string()),
            (
                "HTTP 401: x".into(),
                "No key is set for X.".into(),
                "HTTP 500".into()
            )
        );
        assert_eq!(String::from(o.clone()), "HTTP 500");
        assert_eq!(
            [&a, &n, &o].map(|e| (e.is_unauthorized(), e.is_key_problem())),
            [(true, true), (false, true), (false, false)]
        );
    }

    // The error for a refused key carries the service's words, never the key that was sent.
    #[tokio::test]
    async fn failures_are_typed_by_kind_and_never_hold_the_key() {
        let key = "sk-secret-0123456789abcdef";
        for (status, want) in [
            ("401 Unauthorized", "unauthorized"),
            ("403 Forbidden", "unauthorized"),
            ("429 Too Many Requests", "other"),
            ("500 Internal Server Error", "other"),
        ] {
            let (port, _s) = serve_once(
                status,
                "application/json",
                br#"{"error":{"message":"status 401 in the text"}}"#.to_vec(),
            )
            .await;
            let e = list_models(&format!("http://127.0.0.1:{port}/v1"), key)
                .await
                .unwrap_err();
            let got = match &e {
                ProviderError::Unauthorized(_) => "unauthorized",
                ProviderError::Other(_) => "other",
                ProviderError::NoKey(_) => "nokey",
            };
            assert_eq!(got, want, "{status}: {e}");
            assert!(!e.to_string().contains(key), "{e}");
        }
        // No key for a service that needs one is its own kind, said before anything is sent.
        let e = list_voice_details(Dialect::ElevenLabs, "http://127.0.0.1:1", "")
            .await
            .unwrap_err();
        assert!(matches!(e, ProviderError::NoKey(_)), "{e}");
        assert!(e.is_key_problem() && !e.is_unauthorized());
        // A network failure is neither.
        let e = list_models("http://127.0.0.1:1/v1", key).await.unwrap_err();
        assert!(!e.is_key_problem(), "{e}");
        assert!(!e.to_string().contains(key), "{e}");
    }

    fn deepgram_tts(port: u16, rate: u32) -> HttpSynthesizer {
        HttpSynthesizer::new(
            &format!("http://127.0.0.1:{port}"),
            "dg-key",
            "aura-2",
            "aura-2-thalia-en",
            rate,
            T,
        )
        .unwrap()
        .with_dialect(Dialect::Deepgram)
        .unwrap()
    }

    // REQ-VOI-021
    #[tokio::test]
    async fn deepgram_speaks_headerless_pcm_and_gives_16k_audio() {
        let (port, server) = serve_once("200 OK", "audio/l16", pcm_bytes(4800)).await;
        let audio = deepgram_tts(port, 48_000)
            .synthesize("Hello there.")
            .await
            .unwrap();
        assert_eq!(audio.len(), 1600, "48 kHz becomes 16 kHz");
        let (head, body) = server.await.unwrap();
        assert!(
            head.starts_with("POST /v1/speak?model=aura-2-thalia-en&encoding=linear16&container=none&sample_rate=48000 HTTP/1.1"),
            "{head}"
        );
        assert!(
            head.to_ascii_lowercase()
                .contains("authorization: token dg-key"),
            "{head}"
        );
        let sent: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(sent, json!({"text": "Hello there."}));
        // 16 kHz needs no conversion.
        let (port, _s) = serve_once("200 OK", "audio/l16", pcm_bytes(1600)).await;
        assert_eq!(
            deepgram_tts(port, 16_000)
                .synthesize("x")
                .await
                .unwrap()
                .len(),
            1600
        );
        // 22.05 kHz is not a Deepgram rate.
        let bad = HttpSynthesizer::new("http://127.0.0.1:1", "k", "m", "v", 22_050, T)
            .unwrap()
            .with_dialect(Dialect::Deepgram);
        assert!(bad.unwrap_err().contains("32000"));
    }

    // REQ-VOI-021
    #[tokio::test]
    async fn deepgram_transcribes_a_raw_wav_body_and_reads_the_nested_transcript() {
        let ok =
            br#"{"results":{"channels":[{"alternatives":[{"transcript":" turn on the lamp "}]}]}}"#
                .to_vec();
        let (port, server) = serve_once("200 OK", "application/json", ok.clone()).await;
        let t = HttpTranscriber::new(
            &format!("http://127.0.0.1:{port}"),
            "dg-key",
            "nova-3",
            Some("de"),
            T,
        )
        .unwrap()
        .with_dialect(Dialect::Deepgram);
        assert_eq!(t.transcribe(&[5, 6, 7]).await.unwrap(), "turn on the lamp");
        let (head, body) = server.await.unwrap();
        assert!(
            head.starts_with("POST /v1/listen?model=nova-3&smart_format=true&language=de HTTP/1.1"),
            "{head}"
        );
        let lower = head.to_ascii_lowercase();
        assert!(
            lower.contains("authorization: token dg-key")
                && lower.contains("content-type: audio/wav"),
            "{head}"
        );
        assert_eq!(
            &body[..4],
            b"RIFF",
            "the audio is the body itself, not a form"
        );
        // No language set: ask the service to detect it.
        let (port, server) = serve_once("200 OK", "application/json", ok).await;
        let t = HttpTranscriber::new(
            &format!("http://127.0.0.1:{port}"),
            "dg-key",
            "nova-3",
            None,
            T,
        )
        .unwrap()
        .with_dialect(Dialect::Deepgram);
        t.transcribe(&[1]).await.unwrap();
        assert!(server.await.unwrap().0.contains("detect_language=true"));
        // A bad key and a rate limit.
        let (port, _s) = serve_once(
            "401 Unauthorized",
            "application/json",
            br#"{"err_code":"INVALID_AUTH","err_msg":"Invalid credentials.","request_id":"r"}"#
                .to_vec(),
        )
        .await;
        let t = HttpTranscriber::new(
            &format!("http://127.0.0.1:{port}"),
            "dg-key",
            "nova-3",
            None,
            T,
        )
        .unwrap()
        .with_dialect(Dialect::Deepgram);
        let e = t.transcribe(&[1]).await.unwrap_err();
        assert!(
            e.starts_with("HTTP 401: the key was not accepted")
                && e.contains("Invalid credentials"),
            "{e}"
        );
        assert!(!e.contains("dg-key"));
        let (port, _s) = serve_once(
            "429 Too Many Requests",
            "application/json",
            br#"{"err_msg":"slow down"}"#.to_vec(),
        )
        .await;
        let e = deepgram_tts(port, 24_000)
            .synthesize("x")
            .await
            .unwrap_err();
        assert!(
            e.starts_with("HTTP 429: too many requests") && e.contains("slow down"),
            "{e}"
        );
        // The model name cannot change the address.
        let t = HttpTranscriber::new("http://127.0.0.1:1", "k", "nova&x=1", None, T)
            .unwrap()
            .with_dialect(Dialect::Deepgram);
        assert!(t.transcribe(&[1]).await.unwrap_err().contains("not a name"));
    }

    // REQ-VOI-022
    #[tokio::test]
    async fn deepgram_has_a_built_in_voice_list() {
        let (voices, source) =
            list_voice_details(Dialect::Deepgram, "https://api.deepgram.com", "")
                .await
                .unwrap();
        assert_eq!(source, "builtin");
        assert!(
            voices
                .iter()
                .any(|v| v.id == "aura-2-thalia-en" && v.label == "Thalia")
        );
        assert!(voices.iter().all(|v| plain_id("v", &v.id).is_ok()));
    }

    // REQ-VOI-022
    #[tokio::test]
    async fn the_openai_style_voice_list_keeps_names_as_labels() {
        let (port, _s) = serve_once("200 OK", "application/json", br#"["b","a"]"#.to_vec()).await;
        let (voices, source) =
            list_voice_details(Dialect::OpenAi, &format!("http://127.0.0.1:{port}"), "")
                .await
                .unwrap();
        assert_eq!(source, "server");
        assert_eq!(
            voices
                .iter()
                .map(|v| (v.id.as_str(), v.label.as_str()))
                .collect::<Vec<_>>(),
            [("a", "a"), ("b", "b")]
        );
    }

    #[tokio::test]
    async fn synthesis_rejects_empty_audio_and_bad_settings() {
        let (port, _s) = serve_once("200 OK", "audio/pcm", Vec::new()).await;
        let s = HttpSynthesizer::new(&format!("http://127.0.0.1:{port}"), "", "m", "v", 24_000, T)
            .unwrap();
        assert!(s.synthesize("x").await.unwrap_err().contains("no audio"));
        assert!(HttpSynthesizer::new("http://127.0.0.1:1", "", "m", "v", 100, T).is_err());
        assert!(HttpTranscriber::new("http://speech.example.com/v1", "", "m", None, T).is_err());
    }

    #[tokio::test]
    async fn a_silent_server_times_out_instead_of_hanging() {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let _hold = tokio::spawn(async move {
            let (_c, _) = l.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(30)).await;
        });
        let s = HttpSynthesizer::new(
            &format!("http://127.0.0.1:{port}"),
            "",
            "m",
            "v",
            24_000,
            Duration::from_millis(300),
        )
        .unwrap();
        assert!(s.synthesize("x").await.unwrap_err().contains("no response"));
    }
}
