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

/// Longest message from a provider that is passed on to be shown.
const MAX_SHOWN: usize = 300;

/// A remote server's message made safe to show: its first line only, with `secret` replaced everywhere (before the
/// cut, so a key straddling the limit leaves no prefix behind), at most `max` characters.
pub(crate) fn scrub(text: &str, secret: &str, max: usize) -> String {
    let line = text.lines().next().unwrap_or_default();
    let line = if secret.is_empty() {
        line.to_owned()
    } else {
        line.replace(secret, "***")
    };
    if line.chars().count() <= max {
        return line;
    }
    line.chars().take(max).collect::<String>() + "…"
}

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
    /// The value of the `Host` header: the port is part of it unless it is the scheme's default (RFC 9110
    /// section 7.2). A name-based proxy in front of a server on its own port picks the site by it.
    #[must_use]
    pub fn host_header(&self) -> String {
        if self.port == if self.tls { 443 } else { 80 } {
            self.host.clone()
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }

    /// A request to `path` on this server, with the headers every client here sends: `host`, `connection: close`
    /// (one request per connection) and `user-agent`. The caller adds the rest and the body.
    #[must_use]
    pub fn request_builder(
        &self,
        method: Method,
        path: &str,
        user_agent: &str,
    ) -> hyper::http::request::Builder {
        Request::builder()
            .method(method)
            .uri(path)
            .header("host", self.host_header())
            .header("connection", "close")
            .header("user-agent", user_agent)
    }

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
        // Letters, digits, dots, dashes and underscores only: no user name (`user@host`), query, fragment or
        // bracket. Such a text is not a name that resolves, but it can end in `.local` or `localhost` and so pass
        // the plain-http rule for the home network (REQ-SEC-001) while naming something else.
        if host.is_empty()
            || !host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
        {
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
        // The key, the conversation and agent summaries travel here: same rule as the speech clients
        // (REQ-SEC-001), plain http only for this machine and the home network.
        if !endpoint.tls && !crate::speech::is_private_host(&endpoint.host) {
            return Err(format!(
                "{base_url}: the language model API key and conversation must not cross the internet unencrypted; use https:// (plain http:// is for localhost and the home network)"
            ));
        }
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
            // The provider's words are shown to the person: never with the key in them.
            Ok(Err(LlmError::Provider(m))) => {
                Err(LlmError::Provider(scrub(&m, &self.api_key, MAX_SHOWN)))
            }
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
        let mut builder = self
            .endpoint
            .request_builder(
                Method::POST,
                &format!("{}/chat/completions", self.endpoint.base),
                "clawdot",
            )
            .header(CONTENT_TYPE, "application/json")
            .header("accept", "text/event-stream, application/json");
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
#[derive(Clone)]
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
        let io = self.dial(endpoint).await.map_err(|e| e.to_string())?;
        roundtrip(TokioIo::new(io), request).await
    }

    /// Open a connection to `endpoint`, with TLS when its URL asks for it, and hand back the stream. The one
    /// place a connection is made: HTTP requests and the realtime WebSocket both start here.
    ///
    /// # Errors
    /// [`DialError`] saying which step failed.
    pub async fn dial(&self, endpoint: &Endpoint) -> Result<Box<dyn Io>, DialError> {
        let tcp = TcpStream::connect((endpoint.host.as_str(), endpoint.port))
            .await
            .map_err(DialError::Connect)?;
        #[cfg(feature = "tls")]
        if endpoint.tls {
            let name = rustls::pki_types::ServerName::try_from(endpoint.host.clone())
                .map_err(|e| DialError::HostName(e.to_string()))?;
            let stream = self.tls.connect(name, tcp).await.map_err(DialError::Tls)?;
            return Ok(Box::new(stream));
        }
        #[cfg(not(feature = "tls"))]
        if endpoint.tls {
            return Err(DialError::NoTls);
        }
        Ok(Box::new(tcp))
    }
}

/// A byte stream a connection runs over: plain TCP or TLS on top of it.
pub trait Io: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send> Io for T {}

/// Why [`Connector::dial`] failed.
#[derive(Debug)]
pub enum DialError {
    /// The TCP connection could not be made.
    Connect(std::io::Error),
    /// The host name is not one TLS can check.
    HostName(String),
    /// The TLS handshake failed.
    Tls(std::io::Error),
    /// `https://` in a build without the `tls` feature.
    NoTls,
}

impl std::fmt::Display for DialError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Connect(e) => write!(f, "connect: {e}"),
            Self::HostName(e) => write!(f, "bad host name: {e}"),
            Self::Tls(e) => write!(f, "TLS: {e}"),
            Self::NoTls => {
                f.write_str("this build has no TLS support (feature `tls`); use an http:// URL")
            }
        }
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
pub(crate) fn tls_connector() -> Result<tokio_rustls::TlsConnector, String> {
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

    #[test]
    fn requests_carry_the_headers_every_client_sends() {
        let ep = Endpoint::parse("http://hub.local:8123/api").unwrap();
        let r = ep
            .request_builder(Method::PUT, "/api/x", "agent/1")
            .body(())
            .unwrap();
        assert_eq!((r.method(), r.uri().path()), (&Method::PUT, "/api/x"));
        let h = r.headers();
        assert_eq!(h["host"], "hub.local:8123");
        assert_eq!(h["connection"], "close");
        assert_eq!(h["user-agent"], "agent/1");
    }

    #[tokio::test]
    async fn dialing_a_closed_port_says_the_connection_failed() {
        let free = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = free.local_addr().unwrap().port();
        drop(free);
        let ep = Endpoint::parse(&format!("http://127.0.0.1:{port}")).unwrap();
        let e = Connector::new().unwrap().dial(&ep).await.err().unwrap();
        assert!(matches!(e, DialError::Connect(_)), "{e}");
        assert!(e.to_string().starts_with("connect: "), "{e}");
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
        for (url, host) in [
            ("http://h", "h"),
            ("http://h:80/x", "h"),
            ("http://h:8080", "h:8080"),
            ("https://h:443", "h"),
            ("https://h:8443/v1", "h:8443"),
            ("https://h:80", "h:80"),
        ] {
            assert_eq!(Endpoint::parse(url).unwrap().host_header(), host, "{url}");
        }
        for bad in [
            "ftp://h",
            "h",
            "http://",
            "http://h:x",
            "http://a b",
            "http://x@evil.example.local",
            "http://evil.example#.local/",
            "http://evil.example?.local",
            "http://[::1]:8080",
            "http://h\u{e9}/",
        ] {
            assert!(Endpoint::parse(bad).is_err(), "{bad}");
        }
        // The plain-http rule cannot be passed with such a host.
        let d = Duration::from_secs(1);
        assert!(HttpBackend::new("http://evil.example#.local/v1", "", d).is_err());
    }

    #[test]
    fn plain_http_to_the_language_model_is_only_for_this_machine_and_the_home_network() {
        let d = Duration::from_secs(1);
        for ok in [
            "http://localhost:11434/v1",
            "http://127.0.0.1:8080/v1",
            "http://192.168.1.20:11434/v1",
            "http://ollama.local/v1",
        ] {
            assert!(HttpBackend::new(ok, "", d).is_ok(), "{ok}");
        }
        for bad in [
            "http://api.example.com/v1",
            "http://8.8.8.8/v1",
            "http://openrouter.ai/api/v1",
        ] {
            let e = HttpBackend::new(bad, "sk-secret", d).unwrap_err();
            assert!(e.contains("https://") && !e.contains("sk-secret"), "{e}");
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
        // The port is part of the Host header when it is not the default one.
        assert!(
            seen.to_lowercase()
                .contains(&format!("host: 127.0.0.1:{port}\r\n")),
            "{seen}"
        );
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

    // REQ-SEC-001
    #[tokio::test(flavor = "current_thread")]
    async fn a_provider_that_echoes_the_key_does_not_get_it_into_the_error() {
        const ECHO: &[u8] = b"HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{\"error\":{\"message\":\"Bad key sk-secret-12345 for you\"}}";
        let (port, _s) = fake_server(ECHO, false).await;
        let backend = HttpBackend::new(
            &format!("http://127.0.0.1:{port}"),
            "sk-secret-12345",
            Duration::from_secs(10),
        )
        .unwrap();
        let err = backend.complete(&request()).await.unwrap_err();
        assert!(!err.to_string().contains("sk-secret-12345"), "{err}");
        assert!(err.to_string().contains("Bad key"), "{err}");
    }

    #[test]
    fn scrubbing_hides_the_key_clips_and_keeps_one_line() {
        assert_eq!(scrub("a sk-1 b", "sk-1", 100), "a *** b");
        assert_eq!(scrub("first\nsecond", "", 100), "first");
        assert_eq!(scrub(&"x".repeat(500), "k", 300).chars().count(), 301);
        // A key cut by the length limit must not survive as a prefix.
        let msg = format!("{}sk-abcdef", "x".repeat(295));
        assert!(!scrub(&msg, "sk-abcdef", 300).contains("sk-"));
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
