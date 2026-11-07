//! Carries chat requests to an OpenAI-compatible endpoint over HTTP/1.1 (REQ-AST-001, REQ-SEC-001).
//!
//! One request per connection, streamed and parsed incrementally. `https://` needs the `tls` feature
//! (rustls with a bundled root store); `http://` is for local servers such as Ollama or a LAN agent
//! gateway. Every call has an overall deadline so a stalled provider cannot hang the speaker.

use crate::llm::{
    ChatRequest, Completion, LlmError, StreamAccumulator, StreamEvent, parse_completion,
};
use crate::session::ChatBackend;
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::header::{AUTHORIZATION, CONTENT_TYPE};
use hyper::{Method, Request};
use hyper_util::rt::TokioIo;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::time::timeout;

/// Largest response body read when the provider does not stream.
const MAX_BODY: usize = 1024 * 1024;
/// Largest error body kept for diagnostics.
const MAX_ERROR_BODY: usize = 2048;

/// A parsed `http(s)://host[:port][/base]` URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    /// Use TLS.
    pub tls: bool,
    /// Host name.
    pub host: String,
    /// TCP port.
    pub port: u16,
    /// Path prefix without a trailing slash, for example `/api/v1`.
    pub base: String,
}

impl Endpoint {
    /// Parse a base URL such as `https://openrouter.ai/api/v1`.
    ///
    /// # Errors
    /// A message describing what is wrong with the URL.
    pub fn parse(url: &str) -> Result<Self, String> {
        let (tls, rest) = if let Some(r) = url.strip_prefix("https://") {
            (true, r)
        } else if let Some(r) = url.strip_prefix("http://") {
            (false, r)
        } else {
            return Err("the URL must start with https:// or http://".into());
        };
        let (hostport, path) = rest.split_once('/').map_or((rest, ""), |(h, p)| (h, p));
        let (host, port) = match hostport.rsplit_once(':') {
            Some((h, p)) => (h, p.parse::<u16>().map_err(|_| format!("bad port `{p}`"))?),
            None => (hostport, if tls { 443 } else { 80 }),
        };
        if host.is_empty() || host.contains(char::is_whitespace) {
            return Err("the URL has no valid host".into());
        }
        let base = path.trim_matches('/');
        Ok(Self {
            tls,
            host: host.to_owned(),
            port,
            base: if base.is_empty() {
                String::new()
            } else {
                format!("/{base}")
            },
        })
    }
}

/// An OpenAI-compatible chat endpoint.
pub struct HttpBackend {
    endpoint: Endpoint,
    api_key: String,
    deadline: Duration,
    connector: Connector,
}

impl std::fmt::Debug for HttpBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print the key.
        f.debug_struct("HttpBackend")
            .field("endpoint", &self.endpoint)
            .field("deadline", &self.deadline)
            .finish_non_exhaustive()
    }
}

impl HttpBackend {
    /// A backend for `base_url`, authenticating with `api_key` (may be empty for local servers).
    ///
    /// # Errors
    /// A bad URL, or `https://` without the `tls` feature.
    pub fn new(base_url: &str, api_key: &str, deadline: Duration) -> Result<Self, String> {
        let endpoint = Endpoint::parse(base_url)?;
        #[cfg(not(feature = "tls"))]
        if endpoint.tls {
            return Err("this build has no TLS support (feature `tls`); use an http:// URL".into());
        }
        Ok(Self {
            connector: Connector::new()?,
            endpoint,
            api_key: api_key.to_owned(),
            deadline,
        })
    }

    /// Send a request, calling `on_text` for each piece of streamed text, and return the whole response.
    ///
    /// # Errors
    /// Transport, provider, and parse failures; the call is abandoned after the deadline.
    pub async fn stream(
        &self,
        req: &ChatRequest,
        mut on_text: impl FnMut(&str) + Send,
    ) -> Result<Completion, LlmError> {
        match timeout(self.deadline, self.exchange(req, &mut on_text)).await {
            Ok(r) => r,
            Err(_) => Err(LlmError::Transport(format!(
                "no complete response within {} s",
                self.deadline.as_secs()
            ))),
        }
    }

    async fn exchange(
        &self,
        req: &ChatRequest,
        on_text: &mut (dyn FnMut(&str) + Send),
    ) -> Result<Completion, LlmError> {
        let body = serde_json::to_vec(&req.to_json(true))
            .map_err(|e| LlmError::Malformed(e.to_string()))?;
        let mut builder = Request::builder()
            .method(Method::POST)
            .uri(format!("{}/chat/completions", self.endpoint.base))
            .header("host", &self.endpoint.host)
            .header(CONTENT_TYPE, "application/json")
            .header("accept", "text/event-stream, application/json")
            .header("connection", "close")
            .header("user-agent", "clawdot");
        if !self.api_key.is_empty() {
            builder = builder.header(AUTHORIZATION, format!("Bearer {}", self.api_key));
        }
        let request = builder
            .body(Full::new(Bytes::from(body)))
            .map_err(|e| LlmError::Malformed(e.to_string()))?;
        let response = self
            .connector
            .request(&self.endpoint, request)
            .await
            .map_err(LlmError::Transport)?;

        let status = response.status();
        let streaming = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.contains("text/event-stream"));
        let mut body = response.into_body();

        if !status.is_success() {
            let mut kept = Vec::new();
            while let Some(frame) = body.frame().await {
                let Ok(frame) = frame else { break };
                if let Some(data) = frame.data_ref() {
                    kept.extend_from_slice(data);
                    if kept.len() >= MAX_ERROR_BODY {
                        break;
                    }
                }
            }
            let text =
                String::from_utf8_lossy(&kept[..kept.len().min(MAX_ERROR_BODY)]).into_owned();
            // A JSON error body carries the provider's own message; otherwise show the status.
            return Err(match parse_completion(&text) {
                Err(e @ LlmError::Provider(_)) => e,
                _ => LlmError::Provider(format!("HTTP {status}")),
            });
        }

        if !streaming {
            let mut all = Vec::new();
            while let Some(frame) = body.frame().await {
                let frame = frame.map_err(|e| LlmError::Transport(format!("read: {e}")))?;
                if let Some(data) = frame.data_ref() {
                    all.extend_from_slice(data);
                    if all.len() > MAX_BODY {
                        return Err(LlmError::Malformed("response too large".into()));
                    }
                }
            }
            let completion = parse_completion(&String::from_utf8_lossy(&all))?;
            if !completion.text.is_empty() {
                on_text(&completion.text);
            }
            return Ok(completion);
        }

        let mut acc = StreamAccumulator::default();
        let mut buf: Vec<u8> = Vec::new();
        let mut done = false;
        while !done {
            let Some(frame) = body.frame().await else {
                break;
            };
            let frame = frame.map_err(|e| LlmError::Transport(format!("read: {e}")))?;
            let Some(data) = frame.data_ref() else {
                continue;
            };
            buf.extend_from_slice(data);
            if buf.len() > MAX_BODY {
                return Err(LlmError::Malformed("stream line too long".into()));
            }
            while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = buf.drain(..=pos).collect();
                let line = String::from_utf8_lossy(&line);
                for ev in acc.feed_line(line.trim_end_matches(['\n', '\r']))? {
                    match ev {
                        StreamEvent::Text(t) => on_text(&t),
                        StreamEvent::Done => done = true,
                    }
                }
            }
        }
        Ok(acc.finish())
    }
}

/// Opens a connection (with TLS when the URL asks for it) and performs one HTTP/1.1 request.
pub struct Connector {
    #[cfg(feature = "tls")]
    tls: tokio_rustls::TlsConnector,
}

impl std::fmt::Debug for Connector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connector").finish_non_exhaustive()
    }
}

impl Connector {
    /// A connector.
    ///
    /// # Errors
    /// A TLS configuration failure.
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            #[cfg(feature = "tls")]
            tls: tls_connector()?,
        })
    }

    /// Send `request` to `endpoint` and return the response, whose body is still to be read. The request
    /// must carry its own `host` header.
    ///
    /// # Errors
    /// Connection, TLS, and protocol failures, or `https://` in a build without the `tls` feature.
    pub async fn request(
        &self,
        endpoint: &Endpoint,
        request: Request<Full<Bytes>>,
    ) -> Result<hyper::Response<hyper::body::Incoming>, String> {
        let tcp = TcpStream::connect((endpoint.host.as_str(), endpoint.port))
            .await
            .map_err(|e| format!("connect: {e}"))?;
        #[cfg(feature = "tls")]
        if endpoint.tls {
            let name = rustls::pki_types::ServerName::try_from(endpoint.host.clone())
                .map_err(|e| format!("bad host name: {e}"))?;
            let stream = self
                .tls
                .connect(name, tcp)
                .await
                .map_err(|e| format!("TLS: {e}"))?;
            return roundtrip(TokioIo::new(stream), request).await;
        }
        #[cfg(not(feature = "tls"))]
        if endpoint.tls {
            return Err("this build has no TLS support (feature `tls`); use an http:// URL".into());
        }
        roundtrip(TokioIo::new(tcp), request).await
    }
}

async fn roundtrip<T>(
    io: T,
    request: Request<Full<Bytes>>,
) -> Result<hyper::Response<hyper::body::Incoming>, String>
where
    T: hyper::rt::Read + hyper::rt::Write + Unpin + Send + 'static,
{
    let (mut sender, conn) = hyper::client::conn::http1::handshake(io)
        .await
        .map_err(|e| format!("handshake: {e}"))?;
    tokio::spawn(async move {
        let _ = conn.await;
    });
    sender
        .send_request(request)
        .await
        .map_err(|e| format!("request: {e}"))
}

#[cfg(feature = "tls")]
fn tls_connector() -> Result<tokio_rustls::TlsConnector, String> {
    use std::sync::Arc;
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("TLS configuration: {e}"))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(tokio_rustls::TlsConnector::from(Arc::new(config)))
}

impl ChatBackend for HttpBackend {
    async fn complete(&self, req: &ChatRequest) -> Result<Completion, LlmError> {
        self.stream(req, |_| {}).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::Message;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn request() -> ChatRequest {
        ChatRequest {
            model: "m".into(),
            messages: vec![Message::user("hi")],
            tools: vec![],
            max_tokens: 50,
        }
    }

    /// Serve exactly one canned response and report the request that arrived.
    async fn fake_server(
        response: &'static [u8],
        pause_after_headers: bool,
    ) -> (u16, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut seen = Vec::new();
            let mut chunk = [0_u8; 4096];
            // Read until the headers and the declared body are in.
            loop {
                let n = s.read(&mut chunk).await.unwrap();
                seen.extend_from_slice(&chunk[..n]);
                let text = String::from_utf8_lossy(&seen).into_owned();
                if let Some((head, body)) = text.split_once("\r\n\r\n") {
                    let want = head
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                        })
                        .unwrap_or(0);
                    if body.len() >= want {
                        break;
                    }
                }
                if n == 0 {
                    break;
                }
            }
            if pause_after_headers {
                tokio::time::sleep(Duration::from_secs(30)).await;
            }
            s.write_all(response).await.unwrap();
            let _ = s.shutdown().await;
            String::from_utf8_lossy(&seen).into_owned()
        });
        (port, handle)
    }

    const SSE: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n: PROCESSING\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"lo.\"}}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":4,\"completion_tokens\":2,\"cost\":0.001}}\n\ndata: [DONE]\n\n";

    #[test]
    fn urls_parse() {
        let e = Endpoint::parse("https://openrouter.ai/api/v1/").unwrap();
        assert_eq!(
            (e.tls, e.host.as_str(), e.port, e.base.as_str()),
            (true, "openrouter.ai", 443, "/api/v1")
        );
        let l = Endpoint::parse("http://localhost:11434/v1").unwrap();
        assert_eq!((l.tls, l.port, l.base.as_str()), (false, 11434, "/v1"));
        assert_eq!(Endpoint::parse("http://h").unwrap().base, "");
        for bad in ["ftp://h", "h", "http://", "http://h:x", "http://a b"] {
            assert!(Endpoint::parse(bad).is_err(), "{bad}");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_streamed_reply_is_parsed_and_text_is_delivered_as_it_arrives() {
        let (port, server) = fake_server(SSE, false).await;
        let backend = HttpBackend::new(
            &format!("http://127.0.0.1:{port}/v1"),
            "sk-test",
            Duration::from_secs(10),
        )
        .unwrap();
        let mut pieces = Vec::new();
        let c = backend
            .stream(&request(), |t| pieces.push(t.to_owned()))
            .await
            .unwrap();
        assert_eq!(
            (c.text.as_str(), c.finish_reason.as_deref()),
            ("Hello.", Some("stop"))
        );
        assert_eq!(pieces, vec!["Hel", "lo."]);
        assert_eq!(c.usage.and_then(|u| u.cost_usd), Some(0.001));

        let seen = server.await.unwrap();
        assert!(
            seen.starts_with("POST /v1/chat/completions HTTP/1.1"),
            "{seen}"
        );
        assert!(
            seen.to_lowercase()
                .contains("authorization: bearer sk-test")
        );
        assert!(seen.contains("\"stream\":true") && seen.contains("\"max_tokens\":50"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_plain_json_reply_works_too() {
        const JSON: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{\"choices\":[{\"message\":{\"content\":\"Hi.\"},\"finish_reason\":\"stop\"}]}";
        let (port, _s) = fake_server(JSON, false).await;
        let backend = HttpBackend::new(
            &format!("http://127.0.0.1:{port}"),
            "",
            Duration::from_secs(10),
        )
        .unwrap();
        assert_eq!(backend.complete(&request()).await.unwrap().text, "Hi.");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn http_errors_surface_the_providers_message_without_leaking_anything() {
        const E401: &[u8] = b"HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{\"error\":{\"message\":\"No auth credentials found\"}}";
        let (port, _s) = fake_server(E401, false).await;
        let backend = HttpBackend::new(
            &format!("http://127.0.0.1:{port}"),
            "sk-secret",
            Duration::from_secs(10),
        )
        .unwrap();
        let err = backend.complete(&request()).await.unwrap_err();
        assert_eq!(err, LlmError::Provider("No auth credentials found".into()));
        assert!(
            !format!("{backend:?}").contains("sk-secret"),
            "the key must never be printed"
        );

        const E500: &[u8] =
            b"HTTP/1.1 500 Internal Server Error\r\nConnection: close\r\n\r\n<html>oops</html>";
        let (port2, _s2) = fake_server(E500, false).await;
        let b2 = HttpBackend::new(
            &format!("http://127.0.0.1:{port2}"),
            "",
            Duration::from_secs(10),
        )
        .unwrap();
        assert!(
            matches!(b2.complete(&request()).await, Err(LlmError::Provider(m)) if m.contains("500"))
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_stalled_provider_hits_the_deadline() {
        let (port, _s) = fake_server(SSE, true).await;
        let backend = HttpBackend::new(
            &format!("http://127.0.0.1:{port}"),
            "",
            Duration::from_millis(300),
        )
        .unwrap();
        let err = backend.complete(&request()).await.unwrap_err();
        assert!(matches!(err, LlmError::Transport(m) if m.contains("no complete response")));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_refused_connection_is_a_transport_error() {
        let port = {
            let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
            l.local_addr().unwrap().port()
        };
        let backend = HttpBackend::new(
            &format!("http://127.0.0.1:{port}"),
            "",
            Duration::from_secs(5),
        )
        .unwrap();
        assert!(matches!(
            backend.complete(&request()).await,
            Err(LlmError::Transport(_))
        ));
    }
}
