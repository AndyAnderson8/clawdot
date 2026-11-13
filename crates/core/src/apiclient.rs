//! Bounded HTTP calls to the key-authenticated services behind speech and model lists.
//!
//! This is the one client the speech providers ([`crate::speech`]) and the language-model list
//! ([`crate::transport::list_models`]) share: a [`Client`] sends one [`Call`], enforces a deadline and a size cap on
//! the answer, chooses the key (one pasted into the portal wins over the environment), and turns a failure into a
//! typed [`ProviderError`] whose text never holds the key. What differs between services (the header that carries
//! the key, whether a key is required, how an error body is worded) is a [`Wire`], so this file knows no vendor.
//!
//! Audio is sensitive (REQ-SEC-001, REQ-SEC-002). Plain `http://` is accepted only where [`crate::netpolicy`] says
//! so, and error messages never include audio or the API key.

use crate::secrets::Secrets;
use crate::transport::{Connector, Endpoint};
use http_body_util::{BodyExt, Full};
use hyper::Method;
use hyper::body::Bytes;
use serde_json::Value;
use std::time::Duration;
use tokio::time::timeout;

/// Largest response accepted. The whole body is held in memory, and the largest legitimate one is the speech for one
/// request: at most `speech::MAX_TEXT` (1,000) characters. Speech runs at about 12 to 15 characters a second, so that
/// is 67 to 83 seconds; as 24 kHz 16-bit mono PCM (48,000 bytes a second) that is 3.2 to 4.0 million bytes. 4 MiB
/// (4,194,304 bytes, 87 seconds) holds it with a little to spare. Lists and transcripts are a few kilobytes.
const MAX_RESPONSE: usize = 4 * 1024 * 1024;

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

/// How one kind of service is talked to at the HTTP level: what makes a service's key, errors and base address
/// different from the next one's. A speech provider supplies its own ([`crate::speech::Provider::wire`]);
/// [`Bearer`] is the OpenAI style that servers copy.
pub trait Wire: Sync {
    /// How messages name the service.
    fn title(&self) -> &'static str;

    /// What is sent to the service that must not cross the internet unencrypted, as the refusal of plain `http://`
    /// words it.
    fn carries(&self) -> &'static str {
        "the key and audio"
    }

    /// Where to put a key, for the message that says none is set.
    fn key_hint(&self) -> &'static str {
        "Paste one in the voice settings, or name an environment variable that holds it."
    }

    /// Whether a call without a key is refused before it is sent.
    fn needs_key(&self) -> bool {
        false
    }

    /// The header that carries `key`: its name and value.
    fn auth_header(&self, key: &str) -> (&'static str, String);

    /// The base address with anything the service's own paths already include removed.
    fn trim_base<'a>(&self, base: &'a str) -> &'a str {
        base
    }

    /// The message inside the service's error body, or empty.
    fn error_message(&self, body: &Value) -> String {
        error_field(body).unwrap_or_default().to_owned()
    }

    /// A plain reason for the failures a person can fix (the key, the limit, the plan), or empty.
    fn failure_reason(&self, _code: u16) -> &'static str {
        ""
    }

    /// The text of a failed request: `HTTP <code>`, then the reason when there is one, then the service's own
    /// words (cut to 200 characters).
    fn failure(&self, code: u16, msg: &str) -> String {
        let msg: String = msg.chars().take(200).collect();
        let reason = self.failure_reason(code);
        match (reason.is_empty(), msg.is_empty()) {
            (true, true) => format!("HTTP {code}"),
            (true, false) => format!("HTTP {code}: {msg}"),
            (false, true) => format!("HTTP {code}: {reason}"),
            (false, false) => format!("HTTP {code}: {reason} ({msg})"),
        }
    }
}

/// The `{"error": {"message": ...}}` shape most services use.
#[must_use]
pub fn error_field(body: &Value) -> Option<&str> {
    body["error"]["message"].as_str()
}

/// The reasons the paid, keyed services share for the failures a person can fix.
#[must_use]
pub fn keyed_service_reason(code: u16) -> &'static str {
    match code {
        401 | 403 => "the key was not accepted, check that it is the whole key for this service",
        402 => "the plan has no credit or characters left",
        429 => "too many requests or the plan's limit is reached; wait a little and try again",
        _ => "",
    }
}

/// A base address without a trailing `/v1`, for services whose own paths already start with it.
#[must_use]
pub fn without_v1(base: &str) -> &str {
    base.strip_suffix("/v1").unwrap_or(base)
}

/// The OpenAI style: `Authorization: Bearer <key>`, a key only when the server wants one, errors as
/// `HTTP <code>: <message>`. OpenAI itself, Kokoro, Whisper servers, Ollama and OpenRouter all speak it.
#[derive(Debug, Clone, Copy, Default)]
pub struct Bearer;

impl Wire for Bearer {
    fn title(&self) -> &'static str {
        "the speech server"
    }

    fn auth_header(&self, key: &str) -> (&'static str, String) {
        ("authorization", format!("Bearer {key}"))
    }
}

/// The OpenAI style as the language-model server's list of models speaks it: the same bearer key, but no audio is
/// sent, and the key is kept in the model settings.
#[derive(Debug, Clone, Copy, Default)]
pub struct ModelList;

impl Wire for ModelList {
    fn title(&self) -> &'static str {
        "the language model server"
    }

    fn carries(&self) -> &'static str {
        "the key"
    }

    fn key_hint(&self) -> &'static str {
        "Paste one in the model settings, or name an environment variable that holds it."
    }

    fn auth_header(&self, key: &str) -> (&'static str, String) {
        ("authorization", format!("Bearer {key}"))
    }
}

/// One request: where, what it carries and how it is labelled.
#[derive(Debug)]
pub struct Call {
    method: Method,
    path: String,
    content_type: String,
    body: Vec<u8>,
}

impl Call {
    /// A `POST` of `body` to `path` (relative to the service's base address).
    #[must_use]
    pub fn post(path: impl Into<String>, content_type: &str, body: Vec<u8>) -> Self {
        Self {
            method: Method::POST,
            path: path.into(),
            content_type: content_type.to_owned(),
            body,
        }
    }

    /// A `GET` of `path`.
    #[must_use]
    pub fn get(path: impl Into<String>) -> Self {
        Self {
            method: Method::GET,
            path: path.into(),
            content_type: "application/json".to_owned(),
            body: Vec::new(),
        }
    }

    /// A `POST` of a multipart form: the text `fields`, then the audio as a WAV file named `file`.
    #[must_use]
    pub fn wav_form(path: impl Into<String>, fields: &[(&str, &str)], pcm: &[i16]) -> Self {
        // The boundary only needs to be absent from the data; WAV bytes could contain anything, so mix in the length.
        let boundary = format!(
            "clawdot-boundary-{:016x}",
            pcm.len() as u64 ^ 0x9e37_79b9_7f4a_7c15
        );
        let body = multipart(&boundary, fields, &crate::audio::wav(pcm));
        Self::post(
            path,
            &format!("multipart/form-data; boundary={boundary}"),
            body,
        )
    }
}

pub(crate) fn multipart(boundary: &str, fields: &[(&str, &str)], file: &[u8]) -> Vec<u8> {
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

/// Parse `url` and refuse plain http to a host outside the private network ([`crate::netpolicy`]), in the words of
/// the speech services ([`Bearer`]).
#[cfg(test)]
pub(crate) fn check_url(url: &str) -> Result<Endpoint, String> {
    check_url_for(url, &Bearer)
}

/// [`check_url`] with the refusal worded for `wire`.
pub(crate) fn check_url_for(url: &str, wire: &dyn Wire) -> Result<Endpoint, String> {
    let ep = Endpoint::parse(url)?;
    if crate::netpolicy::crosses_internet_unencrypted(&ep) {
        return Err(format!(
            "{url}: {} must not cross the internet unencrypted; use https:// (plain http:// is for localhost and the home network)",
            wire.carries()
        ));
    }
    Ok(ep)
}

/// A connection to one service: its address, key, deadline and [`Wire`].
pub struct Client {
    endpoint: Endpoint,
    key: String,
    /// Prefer the secret of this name, pasted into the portal, over `key` (REQ-VOI-023).
    stored: Option<(Secrets, &'static str)>,
    deadline: Duration,
    connector: Connector,
    wire: &'static dyn Wire,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print the key.
        f.debug_struct("Client")
            .field("endpoint", &self.endpoint)
            .field("deadline", &self.deadline)
            .finish_non_exhaustive()
    }
}

impl Client {
    /// A client for `base_url` that speaks `wire`.
    ///
    /// # Errors
    /// A bad URL, or plain http:// to a host outside the private network.
    pub fn new(
        base_url: &str,
        key: &str,
        deadline: Duration,
        wire: &'static dyn Wire,
    ) -> Result<Self, String> {
        Ok(Self {
            endpoint: check_url_for(base_url, wire)?,
            key: key.to_owned(),
            stored: None,
            deadline,
            connector: Connector::new()?,
            wire,
        })
    }

    /// Speak another service's wire format.
    pub fn set_wire(&mut self, wire: &'static dyn Wire) {
        self.wire = wire;
    }

    /// Use the secret called `name` (see [`crate::secrets`]) when there is one, for a key pasted into the portal
    /// (REQ-VOI-023).
    pub fn set_stored_key(&mut self, secrets: Secrets, name: &'static str) {
        self.stored = Some((secrets, name));
    }

    /// The key to send now: one pasted into the portal wins, so it takes effect without a restart.
    fn current_key(&self) -> String {
        if let Some(k) = self.stored.as_ref().and_then(|(s, name)| s.get(name)) {
            return k;
        }
        self.key.clone()
    }

    /// Send `call` and return the answer's body.
    ///
    /// # Errors
    /// No key where one is needed, a refused key, any other HTTP status, a network failure, an answer over the
    /// size cap, or no answer within the deadline.
    pub async fn send(&self, call: Call) -> Result<Vec<u8>, ProviderError> {
        let Call {
            method,
            path,
            content_type,
            body,
        } = call;
        let base = self.wire.trim_base(self.endpoint.base.as_str());
        let mut req = self
            .endpoint
            .request_builder(method, &format!("{base}{path}"), "clawdot")
            .header("content-type", content_type);
        let key = self.current_key();
        if key.is_empty() && self.wire.needs_key() {
            return Err(ProviderError::NoKey(format!(
                "No key is set for {}. {}",
                self.wire.title(),
                self.wire.key_hint()
            )));
        }
        if !key.is_empty() {
            let (name, value) = self.wire.auth_header(&key);
            req = req.header(name, value);
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
                    .map(|v| self.wire.error_message(&v))
                    .unwrap_or_default();
                // A server that echoes the Authorization header must not put the key on the status page.
                let msg = crate::transport::scrub(&msg, &key, 200);
                let text = self.wire.failure(status.as_u16(), &msg);
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

/// `GET base_url + path` from an OpenAI-style server (`Authorization: Bearer`, worded as `wire` words it) and read
/// the names in the list it returns. `what` is the plural noun used in messages ("voices", "models"). No stored key
/// is used; the caller passes the one in force.
///
/// # Errors
/// A plain description of why the list could not be had.
pub async fn list_names(
    base_url: &str,
    key: &str,
    path: &str,
    what: &str,
    wire: &'static dyn Wire,
) -> Result<Vec<String>, ProviderError> {
    let c = Client::new(base_url, key, Duration::from_secs(10), wire)?;
    let raw = c.send(Call::get(path)).await?;
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
