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
                Dialect::ElevenLabs => req.header("xi-api-key", &key),
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
                // A server that echoes the Authorization header must not put the key on the status page.
                let msg = crate::transport::scrub(&msg, &key, 200);
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
mod tests;
