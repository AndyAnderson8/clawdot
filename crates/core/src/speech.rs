//! Speech-to-text and text-to-speech over OpenAI-compatible HTTP APIs (REQ-VOI-003, REQ-VOI-004).
//!
//! * [`HttpTranscriber`] uploads one utterance as a WAV file to `POST {base}/audio/transcriptions` (batch mode).
//! * [`HttpSynthesizer`] asks `POST {base}/audio/speech` for raw 16-bit PCM and converts it to 16 kHz.
//!
//! Audio is sensitive (REQ-SEC-001, REQ-SEC-002). Plain `http://` is accepted only for hosts on the local
//! machine or private network, such as a self-hosted Whisper server; anything on the internet needs `https://`.
//! Audio is never written to disk or logged, and error messages never include it or the API key.

use crate::transport::{Connector, Endpoint};
use crate::voice::{Synthesizer, Transcriber};
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::{Method, Request};
use serde_json::{Value, json};
use std::net::IpAddr;
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

struct Client {
    endpoint: Endpoint,
    key: String,
    deadline: Duration,
    connector: Connector,
}

impl Client {
    fn new(base_url: &str, key: &str, deadline: Duration) -> Result<Self, String> {
        Ok(Self {
            endpoint: check_url(base_url)?,
            key: key.to_owned(),
            deadline,
            connector: Connector::new()?,
        })
    }

    async fn post(&self, path: &str, content_type: &str, body: Vec<u8>) -> Result<Vec<u8>, String> {
        let mut req = Request::builder()
            .method(Method::POST)
            .uri(format!("{}{path}", self.endpoint.base))
            .header("host", &self.endpoint.host)
            .header("content-type", content_type)
            .header("connection", "close")
            .header("user-agent", "clawdot");
        if !self.key.is_empty() {
            req = req.header("authorization", format!("Bearer {}", self.key));
        }
        let request = req
            .body(Full::new(Bytes::from(body)))
            .map_err(|e| e.to_string())?;
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
                        return Err("response too large".to_owned());
                    }
                }
            }
            if status.is_success() {
                Ok(all)
            } else {
                // Provider error bodies are JSON with a message; show that, never anything else.
                let msg = serde_json::from_slice::<Value>(&all)
                    .ok()
                    .and_then(|v| v["error"]["message"].as_str().map(str::to_owned))
                    .unwrap_or_default();
                Err(format!(
                    "HTTP {}{}{}",
                    status.as_u16(),
                    if msg.is_empty() { "" } else { ": " },
                    msg.chars().take(200).collect::<String>()
                ))
            }
        };
        timeout(self.deadline, work)
            .await
            .map_err(|_| format!("no response within {} s", self.deadline.as_secs()))?
    }
}

/// Batch speech-to-text.
pub struct HttpTranscriber {
    client: Client,
    model: String,
    language: Option<String>,
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
            language: language.map(str::to_owned),
        })
    }
}

impl Transcriber for HttpTranscriber {
    async fn transcribe(&self, pcm: &[i16]) -> Result<String, String> {
        // The boundary only needs to be absent from the data; WAV bytes could contain anything, so mix in the length.
        let boundary = format!(
            "clawdot-boundary-{:016x}",
            pcm.len() as u64 ^ 0x9e37_79b9_7f4a_7c15
        );
        let mut fields = vec![("model", self.model.as_str()), ("response_format", "json")];
        if let Some(l) = &self.language {
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
    voice: String,
    source_rate: u32,
}

impl std::fmt::Debug for HttpSynthesizer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpSynthesizer")
            .field("model", &self.model)
            .field("voice", &self.voice)
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
            voice: voice.to_owned(),
            source_rate,
        })
    }
}

impl Synthesizer for HttpSynthesizer {
    async fn synthesize(&self, text: &str) -> Result<Vec<i16>, String> {
        let text: String = text.chars().take(MAX_TEXT).collect();
        let body = json!({ "model": self.model, "voice": self.voice, "input": text, "response_format": "pcm" });
        let raw = self
            .client
            .post(
                "/audio/speech",
                "application/json",
                body.to_string().into_bytes(),
            )
            .await?;
        let samples = decode_pcm16(&raw);
        if samples.is_empty() {
            return Err("the provider returned no audio".into());
        }
        Ok(resample_to_16k(&samples, self.source_rate))
    }
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
