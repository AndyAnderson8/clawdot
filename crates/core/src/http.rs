//! HTTP front end: webhook ingress, the SSE reply stream for agent bridges, and acknowledgements
//! (REQ-INB-001/002/008, REQ-SEC-004, REQ-NFR-110).
//!
//! Routes:
//!
//! * `GET  /healthz`
//! * `POST /hook/{endpoint}`                      an agent posts its native hook payload
//! * `GET  /bridge/{endpoint}/replies`            SSE stream of replies for the agent bridge
//! * `POST /bridge/{endpoint}/replies/{id}/ack`   the bridge confirms it handled a reply
//!
//! Authentication is `Authorization: Bearer <token>` or `X-ClawDot-Token: <token>`. Hook responses are
//! deliberately an empty JSON object: agent runtimes such as Claude Code parse hook responses as
//! *hook output*, so any field we invented could be misread as a decision. The event id travels in the
//! `X-ClawDot-Event` header instead.

use crate::app::{Core, HookError, HookOutcome};
use http_body_util::{BodyExt, Full, Limited, combinators::BoxBody};
use hyper::body::{Body, Bytes, Frame, Incoming};
use hyper::header::{
    AUTHORIZATION, CONTENT_LENGTH, CONTENT_TYPE, HeaderMap, HeaderValue, RETRY_AFTER,
    WWW_AUTHENTICATE,
};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use std::convert::Infallible;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::{Semaphore, mpsc};

type Resp = Response<BoxBody<Bytes, Infallible>>;

/// Server tuning.
#[derive(Debug, Clone, Copy)]
pub struct ServeOptions {
    /// Connections handled at once; further connections are dropped immediately.
    pub max_connections: usize,
    /// Reply streams open at once (each holds a connection for a long time).
    pub max_streams: usize,
    /// How long a client may take to send request headers.
    pub header_timeout: Duration,
    /// Idle time before an SSE comment keeps the connection alive.
    pub heartbeat: Duration,
}

impl Default for ServeOptions {
    fn default() -> Self {
        Self {
            max_connections: 32,
            max_streams: 8,
            header_timeout: Duration::from_secs(10),
            heartbeat: Duration::from_secs(15),
        }
    }
}

/// Serve until `shutdown` completes.
///
/// # Errors
/// Fails if accepting connections fails.
pub async fn serve(
    listener: TcpListener,
    core: Arc<Core>,
    opts: ServeOptions,
    shutdown: impl Future<Output = ()> + Send,
) -> io::Result<()> {
    let slots = Arc::new(Semaphore::new(opts.max_connections));
    let streams = Arc::new(AtomicUsize::new(0));
    tokio::pin!(shutdown);
    loop {
        let (stream, _) = tokio::select! {
            accepted = listener.accept() => accepted?,
            () = &mut shutdown => return Ok(()),
        };
        // Shed load instead of queueing it: a flood must not grow memory (REQ-NFR-110).
        let Ok(permit) = Arc::clone(&slots).try_acquire_owned() else {
            drop(stream);
            continue;
        };
        let core = Arc::clone(&core);
        let streams = Arc::clone(&streams);
        tokio::spawn(async move {
            let _permit = permit;
            let service =
                service_fn(move |req| route(Arc::clone(&core), Arc::clone(&streams), opts, req));
            let conn = http1::Builder::new()
                .timer(TokioTimer::new())
                .header_read_timeout(opts.header_timeout)
                .serve_connection(TokioIo::new(stream), service);
            let _ = conn.await;
        });
    }
}

fn text(status: StatusCode, body: &'static str) -> Resp {
    let mut r = Response::new(Full::new(Bytes::from_static(body.as_bytes())).boxed());
    *r.status_mut() = status;
    r.headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    r
}

fn json(status: StatusCode, body: &'static str) -> Resp {
    text(status, body)
}

fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 32
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

fn token_from(headers: &HeaderMap) -> Option<String> {
    if let Some(v) = headers.get(AUTHORIZATION).and_then(|v| v.to_str().ok()) {
        let (scheme, rest) = v.split_once(' ')?;
        return scheme
            .eq_ignore_ascii_case("bearer")
            .then(|| rest.trim().to_owned());
    }
    headers
        .get("x-clawdot-token")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_owned())
}

fn method_not_allowed() -> Resp {
    json(
        StatusCode::METHOD_NOT_ALLOWED,
        r#"{"error":"method not allowed"}"#,
    )
}

fn error_response(e: &HookError) -> Resp {
    match e {
        HookError::Unauthorized => {
            let mut r = json(StatusCode::UNAUTHORIZED, r#"{"error":"unauthorized"}"#);
            r.headers_mut()
                .insert(WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
            r
        }
        HookError::RateLimited => {
            let mut r = json(StatusCode::TOO_MANY_REQUESTS, r#"{"error":"rate limited"}"#);
            r.headers_mut()
                .insert(RETRY_AFTER, HeaderValue::from_static("1"));
            r
        }
        HookError::BadJson => json(
            StatusCode::BAD_REQUEST,
            r#"{"error":"body is not valid JSON"}"#,
        ),
        HookError::Unmappable => json(
            StatusCode::UNPROCESSABLE_ENTITY,
            r#"{"error":"the endpoint's adapter has no rule for this payload"}"#,
        ),
        HookError::NoAdapter => json(
            StatusCode::UNPROCESSABLE_ENTITY,
            r#"{"error":"the endpoint's adapter is not installed"}"#,
        ),
    }
}

async fn route(
    core: Arc<Core>,
    streams: Arc<AtomicUsize>,
    opts: ServeOptions,
    req: Request<Incoming>,
) -> Result<Resp, Infallible> {
    let (parts, body) = req.into_parts();
    let path = parts.uri.path().to_owned();
    let segs: Vec<&str> = path.trim_matches('/').split('/').collect();
    // First decide whether the path names a real resource (invalid endpoint names are simply 404),
    // then whether the method is allowed on it (405).
    let resp = match segs.as_slice() {
        ["healthz"] => match parts.method {
            Method::GET => json(StatusCode::OK, r#"{"status":"ok"}"#),
            _ => method_not_allowed(),
        },
        ["hook", name] if valid_name(name) => match parts.method {
            Method::POST => hook(&core, &parts.headers, name, body).await,
            _ => method_not_allowed(),
        },
        ["bridge", name, "replies"] if valid_name(name) => match parts.method {
            Method::GET => replies_stream(core, streams, opts, &parts.headers, name),
            _ => method_not_allowed(),
        },
        ["bridge", name, "replies", id, "ack"] if valid_name(name) => match parts.method {
            Method::POST => ack(&core, &parts.headers, name, id),
            _ => method_not_allowed(),
        },
        _ => match crate::ui::handle(&core, &parts.method, &segs, &parts.headers, body).await {
            Some(r) => r,
            None => json(StatusCode::NOT_FOUND, r#"{"error":"not found"}"#),
        },
    };
    Ok(resp)
}

async fn hook(core: &Core, headers: &HeaderMap, name: &str, body: Incoming) -> Resp {
    let max = core.limits().max_body;
    // Refuse oversized bodies before reading any of them.
    let declared = headers
        .get(CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok());
    if declared.is_some_and(|n| n > max) {
        return json(
            StatusCode::PAYLOAD_TOO_LARGE,
            r#"{"error":"body too large"}"#,
        );
    }
    let Some(token) = token_from(headers) else {
        return error_response(&HookError::Unauthorized);
    };
    // Authenticate and throttle before spending effort on the body.
    if let Err(e) = core.bridge_check(name, &token) {
        return error_response(&e);
    }
    let bytes = match Limited::new(body, max).collect().await {
        Ok(c) => c.to_bytes(),
        Err(_) => {
            return json(
                StatusCode::PAYLOAD_TOO_LARGE,
                r#"{"error":"body too large or unreadable"}"#,
            );
        }
    };
    match core.ingest_hook(name, &token, &bytes) {
        Ok(outcome) => {
            let (label, id) = match outcome {
                HookOutcome::Accepted(id) => ("accepted", id),
                HookOutcome::Coalesced(id) => ("coalesced", id),
                HookOutcome::Filtered(_) => ("filtered", 0),
            };
            let mut r = json(StatusCode::ACCEPTED, "{}");
            if let Ok(v) = HeaderValue::from_str(&format!("{label}; id={id}")) {
                r.headers_mut().insert("x-clawdot-event", v);
            }
            r
        }
        Err(e) => error_response(&e),
    }
}

fn ack(core: &Core, headers: &HeaderMap, name: &str, id: &str) -> Resp {
    let Some(token) = token_from(headers) else {
        return error_response(&HookError::Unauthorized);
    };
    let Ok(id) = id.parse::<u64>() else {
        return json(StatusCode::BAD_REQUEST, r#"{"error":"bad reply id"}"#);
    };
    match core.bridge_ack(name, &token, id) {
        Ok(true) => Response::builder()
            .status(StatusCode::NO_CONTENT)
            .body(Full::new(Bytes::new()).boxed())
            .unwrap_or_else(|_| json(StatusCode::NO_CONTENT, "")),
        Ok(false) => json(StatusCode::NOT_FOUND, r#"{"error":"no such reply"}"#),
        Err(e) => error_response(&e),
    }
}

/// A response body fed from a channel, so a spawned task can push SSE events as they happen.
struct ChannelBody {
    rx: mpsc::Receiver<Bytes>,
}

impl Body for ChannelBody {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Infallible>>> {
        self.get_mut()
            .rx
            .poll_recv(cx)
            .map(|item| item.map(|b| Ok(Frame::data(b))))
    }
}

struct StreamGuard(Arc<AtomicUsize>);

impl Drop for StreamGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

fn replies_stream(
    core: Arc<Core>,
    streams: Arc<AtomicUsize>,
    opts: ServeOptions,
    headers: &HeaderMap,
    name: &str,
) -> Resp {
    let Some(token) = token_from(headers) else {
        return error_response(&HookError::Unauthorized);
    };
    if let Err(e) = core.bridge_check(name, &token) {
        return error_response(&e);
    }
    if streams.fetch_add(1, Ordering::SeqCst) >= opts.max_streams {
        streams.fetch_sub(1, Ordering::SeqCst);
        return json(
            StatusCode::SERVICE_UNAVAILABLE,
            r#"{"error":"too many open streams"}"#,
        );
    }
    let guard = StreamGuard(streams);
    let (tx, rx) = mpsc::channel::<Bytes>(16);
    let name = name.to_owned();

    tokio::spawn(async move {
        let _guard = guard;
        let mut last_sent = 0_u64;
        if tx
            .send(Bytes::from_static(b"retry: 3000\n\n"))
            .await
            .is_err()
        {
            return;
        }
        loop {
            // Register for wake-ups BEFORE reading state, so a reply queued in between is not missed.
            let notified = core.changed().notified();
            tokio::pin!(notified);
            notified.as_mut().enable();

            // Re-checking credentials each round ends the stream as soon as a token is rotated or
            // revoked (REQ-INB-002).
            let Ok(replies) = core.bridge_pending(&name, &token, last_sent) else {
                return;
            };
            for r in replies {
                let Ok(data) = serde_json::to_string(&r) else {
                    continue;
                };
                let event = format!("id: {}\nevent: reply\ndata: {}\n\n", r.id, data);
                if tx.send(Bytes::from(event)).await.is_err() {
                    return;
                }
                last_sent = last_sent.max(r.id);
            }
            tokio::select! {
                () = &mut notified => {}
                () = tokio::time::sleep(opts.heartbeat) => {
                    if tx.send(Bytes::from_static(b": keepalive\n\n")).await.is_err() {
                        return;
                    }
                }
                () = tx.closed() => return,
            }
        }
    });

    let mut r = Response::new(ChannelBody { rx }.boxed());
    let h = r.headers_mut();
    h.insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    h.insert("cache-control", HeaderValue::from_static("no-cache"));
    h.insert("x-accel-buffering", HeaderValue::from_static("no"));
    // One stream per connection: when the stream ends, the connection ends too.
    h.insert("connection", HeaderValue::from_static("close"));
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Adapters, Limits};
    use crate::inbox::reply::ReplyKind;
    use crate::sched::SystemClock;
    use std::net::SocketAddr;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;
    use tokio::sync::oneshot;
    use tokio::time::timeout;

    const STOP: &str = r#"{"session_id":"s1","cwd":"/home/a/clawdot","hook_event_name":"Stop","last_assistant_message":"Done."}"#;

    struct Harness {
        core: Arc<Core>,
        addr: SocketAddr,
        token: String,
        _shutdown: oneshot::Sender<()>,
    }

    async fn start(limits: Limits, opts: ServeOptions) -> Harness {
        let core = Arc::new(Core::in_memory(
            Arc::new(SystemClock),
            Adapters::builtin(),
            limits,
        ));
        let (_, token) = core.create_endpoint("claude-code", "claude-code").unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = oneshot::channel::<()>();
        let served = Arc::clone(&core);
        tokio::spawn(async move {
            let _ = serve(listener, served, opts, async {
                let _ = rx.await;
            })
            .await;
        });
        Harness {
            core,
            addr,
            token,
            _shutdown: tx,
        }
    }

    /// One request, one response, then the connection closes.
    async fn call(
        addr: SocketAddr,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: &str,
    ) -> (u16, String, String) {
        let mut s = TcpStream::connect(addr).await.unwrap();
        let mut req = format!(
            "{method} {path} HTTP/1.1\r\nHost: t\r\nConnection: close\r\nContent-Length: {}\r\n",
            body.len()
        );
        for (k, v) in headers {
            req.push_str(&format!("{k}: {v}\r\n"));
        }
        req.push_str("\r\n");
        req.push_str(body);
        s.write_all(req.as_bytes()).await.unwrap();
        let mut raw = Vec::new();
        timeout(Duration::from_secs(5), s.read_to_end(&mut raw))
            .await
            .unwrap()
            .unwrap();
        let text = String::from_utf8_lossy(&raw).into_owned();
        let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
        let status = head
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        (status, head.to_owned(), body.to_owned())
    }

    fn bearer(token: &str) -> String {
        format!("Bearer {token}")
    }

    #[tokio::test(flavor = "current_thread")]
    async fn health_check_works_without_credentials() {
        let h = start(Limits::default(), ServeOptions::default()).await;
        let (status, _, body) = call(h.addr, "GET", "/healthz", &[], "").await;
        assert_eq!((status, body.as_str()), (200, r#"{"status":"ok"}"#));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_claude_code_hook_is_accepted_and_the_response_is_an_empty_object() {
        let h = start(Limits::default(), ServeOptions::default()).await;
        let auth = bearer(&h.token);
        let (status, head, body) = call(
            h.addr,
            "POST",
            "/hook/claude-code",
            &[
                ("Authorization", &auth),
                ("Content-Type", "application/json"),
            ],
            STOP,
        )
        .await;
        assert_eq!(status, 202);
        assert_eq!(
            body, "{}",
            "hook runtimes parse the body, so it must carry no fields"
        );
        assert!(
            head.to_lowercase()
                .contains("x-clawdot-event: accepted; id=1"),
            "{head}"
        );
        assert_eq!(h.core.events().len(), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn the_alternate_token_header_works() {
        let h = start(Limits::default(), ServeOptions::default()).await;
        let (status, _, _) = call(
            h.addr,
            "POST",
            "/hook/claude-code",
            &[("X-ClawDot-Token", &h.token)],
            STOP,
        )
        .await;
        assert_eq!(status, 202);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn missing_wrong_and_unknown_credentials_all_get_401() {
        let h = start(Limits::default(), ServeOptions::default()).await;
        let wrong = bearer("cdt_wrong");
        for (path, headers) in [
            ("/hook/claude-code", vec![]),
            ("/hook/claude-code", vec![("Authorization", wrong.as_str())]),
            ("/hook/unknown", vec![("Authorization", wrong.as_str())]),
        ] {
            let (status, head, _) = call(h.addr, "POST", path, &headers, STOP).await;
            assert_eq!(status, 401, "{path}");
            assert!(head.to_lowercase().contains("www-authenticate: bearer"));
        }
        assert!(h.core.events().is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn oversized_bodies_are_refused_without_being_read() {
        let limits = Limits {
            max_body: 1024,
            ..Limits::default()
        };
        let h = start(limits, ServeOptions::default()).await;
        let auth = bearer(&h.token);
        // Declares 10 MB but sends almost nothing: the server must answer 413 immediately.
        let mut s = TcpStream::connect(h.addr).await.unwrap();
        let req = format!(
            "POST /hook/claude-code HTTP/1.1\r\nHost: t\r\nAuthorization: {auth}\r\nContent-Length: 10485760\r\nConnection: close\r\n\r\n{{}}"
        );
        s.write_all(req.as_bytes()).await.unwrap();
        let mut raw = Vec::new();
        timeout(Duration::from_secs(5), s.read_to_end(&mut raw))
            .await
            .unwrap()
            .unwrap();
        assert!(String::from_utf8_lossy(&raw).starts_with("HTTP/1.1 413"));
        // And an actually-large body is cut off at the limit.
        let big = format!(r#"{{"x":"{}"}}"#, "a".repeat(5000));
        let (status, _, _) = call(
            h.addr,
            "POST",
            "/hook/claude-code",
            &[("Authorization", &auth)],
            &big,
        )
        .await;
        assert_eq!(status, 413);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn bad_json_unknown_paths_and_wrong_methods_get_the_right_statuses() {
        let h = start(Limits::default(), ServeOptions::default()).await;
        let auth = bearer(&h.token);
        assert_eq!(
            call(
                h.addr,
                "POST",
                "/hook/claude-code",
                &[("Authorization", &auth)],
                "not json"
            )
            .await
            .0,
            400
        );
        assert_eq!(
            call(h.addr, "GET", "/hook/claude-code", &[], "").await.0,
            405
        );
        assert_eq!(call(h.addr, "GET", "/nope", &[], "").await.0, 404);
        assert_eq!(
            call(
                h.addr,
                "POST",
                "/hook/..%2f..",
                &[("Authorization", &auth)],
                "{}"
            )
            .await
            .0,
            404
        );
        assert_eq!(
            call(
                h.addr,
                "POST",
                "/hook/UPPER",
                &[("Authorization", &auth)],
                "{}"
            )
            .await
            .0,
            404
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_flood_gets_429s() {
        let h = start(
            Limits {
                hook_burst: 3.0,
                hook_per_sec: 0.1,
                ..Limits::default()
            },
            ServeOptions::default(),
        )
        .await;
        let auth = bearer(&h.token);
        let mut seen_429 = false;
        for i in 0..10 {
            let body = format!(r#"{{"hook_event_name":"Stop","session_id":"s{i}","cwd":"/a/b"}}"#);
            let (status, head, _) = call(
                h.addr,
                "POST",
                "/hook/claude-code",
                &[("Authorization", &auth)],
                &body,
            )
            .await;
            if status == 429 {
                assert!(head.to_lowercase().contains("retry-after"));
                seen_429 = true;
            }
        }
        assert!(seen_429);
    }

    /// Read from an SSE connection until `needle` appears or time runs out.
    async fn read_until(s: &mut TcpStream, buf: &mut String, needle: &str) {
        let mut chunk = [0_u8; 1024];
        timeout(Duration::from_secs(5), async {
            while !buf.contains(needle) {
                let n = s.read(&mut chunk).await.unwrap();
                assert!(n > 0, "stream closed early; got {buf:?}");
                buf.push_str(&String::from_utf8_lossy(&chunk[..n]));
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {needle:?}; got {buf:?}"));
    }

    async fn open_stream(addr: SocketAddr, token: &str) -> TcpStream {
        let mut s = TcpStream::connect(addr).await.unwrap();
        let req = format!(
            "GET /bridge/claude-code/replies HTTP/1.1\r\nHost: t\r\nAuthorization: Bearer {token}\r\nAccept: text/event-stream\r\n\r\n"
        );
        s.write_all(req.as_bytes()).await.unwrap();
        s
    }

    async fn queue_reply(h: &Harness, text: &str) {
        let auth = bearer(&h.token);
        call(
            h.addr,
            "POST",
            "/hook/claude-code",
            &[("Authorization", &auth)],
            STOP,
        )
        .await;
        let ev = h.core.next_announcement(true, 0).unwrap().event;
        h.core.mark_announced(&[ev.id]);
        h.core.reply(ReplyKind::Instruct, text).unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_bridge_receives_a_reply_queued_before_and_after_it_connects() {
        let h = start(Limits::default(), ServeOptions::default()).await;
        queue_reply(&h, "keep going").await; // queued before connecting

        let mut s = open_stream(h.addr, &h.token).await;
        let mut buf = String::new();
        read_until(&mut s, &mut buf, "event: reply").await;
        assert!(
            buf.starts_with("HTTP/1.1 200") && buf.to_lowercase().contains("text/event-stream")
        );
        assert!(
            buf.contains("\"text\":\"keep going\"") && buf.contains("\"session_ref\":\"s1\""),
            "{buf}"
        );

        // A reply queued while the stream is open is pushed promptly.
        let auth = bearer(&h.token);
        let second = r#"{"session_id":"s2","cwd":"/a/b","hook_event_name":"Stop"}"#;
        call(
            h.addr,
            "POST",
            "/hook/claude-code",
            &[("Authorization", &auth)],
            second,
        )
        .await;
        let ev = h.core.next_announcement(true, 0).unwrap().event;
        h.core.mark_announced(&[ev.id]);
        h.core.reply(ReplyKind::Instruct, "and now this").unwrap();
        read_until(&mut s, &mut buf, "and now this").await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn unacknowledged_replies_are_resent_on_reconnect_and_acks_clear_them() {
        let h = start(Limits::default(), ServeOptions::default()).await;
        queue_reply(&h, "do the thing").await;
        {
            let mut s = open_stream(h.addr, &h.token).await;
            let mut buf = String::new();
            read_until(&mut s, &mut buf, "do the thing").await;
        } // connection dropped without acknowledging

        let mut s = open_stream(h.addr, &h.token).await;
        let mut buf = String::new();
        read_until(&mut s, &mut buf, "do the thing").await; // resent

        let auth = bearer(&h.token);
        let (status, _, _) = call(
            h.addr,
            "POST",
            "/bridge/claude-code/replies/1/ack",
            &[("Authorization", &auth)],
            "",
        )
        .await;
        assert_eq!(status, 204);
        assert_eq!(
            call(
                h.addr,
                "POST",
                "/bridge/claude-code/replies/1/ack",
                &[("Authorization", &auth)],
                ""
            )
            .await
            .0,
            404
        );
        assert!(
            h.core
                .bridge_pending("claude-code", &h.token, 0)
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn streams_require_credentials_and_are_capped() {
        let h = start(
            Limits::default(),
            ServeOptions {
                max_streams: 1,
                ..ServeOptions::default()
            },
        )
        .await;
        assert_eq!(
            call(h.addr, "GET", "/bridge/claude-code/replies", &[], "")
                .await
                .0,
            401
        );
        let mut first = open_stream(h.addr, &h.token).await;
        let mut buf = String::new();
        read_until(&mut first, &mut buf, "retry:").await;
        let auth = bearer(&h.token);
        let (status, _, _) = call(
            h.addr,
            "GET",
            "/bridge/claude-code/replies",
            &[("Authorization", &auth)],
            "",
        )
        .await;
        assert_eq!(status, 503, "the second stream is refused");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn rotating_the_token_ends_the_stream() {
        let h = start(
            Limits::default(),
            ServeOptions {
                heartbeat: Duration::from_millis(100),
                ..ServeOptions::default()
            },
        )
        .await;
        let mut s = open_stream(h.addr, &h.token).await;
        let mut buf = String::new();
        read_until(&mut s, &mut buf, "retry:").await;
        h.core.rotate_endpoint("claude-code").unwrap();
        // The next heartbeat re-checks credentials and the server closes the stream.
        let mut chunk = [0_u8; 256];
        let closed = timeout(Duration::from_secs(5), async {
            loop {
                match s.read(&mut chunk).await {
                    Ok(0) | Err(_) => return true,
                    Ok(_) => {}
                }
            }
        })
        .await;
        assert_eq!(closed, Ok(true));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn the_server_stops_when_asked() {
        let core = Arc::new(Core::in_memory(
            Arc::new(SystemClock),
            Adapters::builtin(),
            Limits::default(),
        ));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (tx, rx) = oneshot::channel::<()>();
        let task = tokio::spawn(serve(listener, core, ServeOptions::default(), async {
            let _ = rx.await;
        }));
        tx.send(()).unwrap();
        assert!(
            timeout(Duration::from_secs(5), task)
                .await
                .unwrap()
                .unwrap()
                .is_ok()
        );
    }

    // ----- web UI (REQ-CFG-001, 003, 005, 006) -----

    fn admin(h: &Harness) -> String {
        h.core
            .set_admin_token_hash(&crate::inbox::endpoint::hash_token("adm-secret"));
        bearer("adm-secret")
    }

    #[tokio::test]
    async fn the_page_is_static_self_contained_and_locked_down() {
        let h = start(Limits::default(), ServeOptions::default()).await;
        for path in ["/", "/ui"] {
            let (status, head, body) = call(h.addr, "GET", path, &[], "").await;
            assert_eq!(status, 200, "{path}");
            let lower = head.to_ascii_lowercase();
            assert!(
                lower.contains("content-security-policy: default-src 'none'"),
                "{head}"
            );
            assert!(lower.contains("x-frame-options: deny"));
            assert!(body.contains("<title>ClawDot</title>"));
            // No external assets (REQ-CFG-001).
            assert!(
                !body.contains("http://") && !body.contains("src=\"https://"),
                "external reference in page"
            );
            assert!(
                !body.contains("innerHTML"),
                "agent text must only be rendered as text"
            );
        }
    }

    #[tokio::test]
    async fn the_api_needs_the_administrator_token() {
        let h = start(Limits::default(), ServeOptions::default()).await;
        // No administrator token configured at all: everyone is refused.
        assert_eq!(call(h.addr, "GET", "/api/overview", &[], "").await.0, 401);
        let good = admin(&h);
        let bad = bearer("nope");
        assert_eq!(call(h.addr, "GET", "/api/overview", &[], "").await.0, 401);
        assert_eq!(
            call(
                h.addr,
                "GET",
                "/api/overview",
                &[("Authorization", bad.as_str())],
                ""
            )
            .await
            .0,
            401
        );
        // An endpoint token is not an administrator token.
        let ep = bearer(&h.token);
        assert_eq!(
            call(
                h.addr,
                "GET",
                "/api/overview",
                &[("Authorization", ep.as_str())],
                ""
            )
            .await
            .0,
            401
        );
        let (status, _, body) = call(
            h.addr,
            "GET",
            "/api/overview",
            &[("Authorization", good.as_str())],
            "",
        )
        .await;
        assert_eq!(status, 200);
        assert!(body.contains("claude-code"));
        assert!(!body.contains("token_hash"), "hashes are never shown");
    }

    #[tokio::test]
    async fn endpoints_can_be_created_rotated_and_deleted_and_tokens_show_once() {
        let h = start(Limits::default(), ServeOptions::default()).await;
        let a = admin(&h);
        let auth = [("Authorization", a.as_str())];

        let (status, _, body) = call(
            h.addr,
            "POST",
            "/api/endpoints",
            &auth,
            r#"{"name":"ci","adapter":"generic"}"#,
        )
        .await;
        assert_eq!(status, 201);
        let token = serde_json::from_str::<serde_json::Value>(&body).unwrap()["token"]
            .as_str()
            .unwrap()
            .to_owned();
        // The new token works for posting events.
        let tb = bearer(&token);
        let (status, ..) = call(
            h.addr,
            "POST",
            "/hook/ci",
            &[("Authorization", tb.as_str())],
            r#"{"title":"build done","text":"ok"}"#,
        )
        .await;
        assert_eq!(status, 202);
        // The overview shows the event but never the token.
        let (_, _, ov) = call(h.addr, "GET", "/api/overview", &auth, "").await;
        assert!(ov.contains("build done") && !ov.contains(&token));

        let (status, ..) = call(
            h.addr,
            "POST",
            "/api/endpoints",
            &auth,
            r#"{"name":"ci","adapter":"generic"}"#,
        )
        .await;
        assert_eq!(status, 409);
        let (status, ..) = call(
            h.addr,
            "POST",
            "/api/endpoints",
            &auth,
            r#"{"name":"x","adapter":"nope"}"#,
        )
        .await;
        assert_eq!(status, 400);
        let (status, ..) = call(
            h.addr,
            "POST",
            "/api/endpoints",
            &auth,
            r#"{"name":"Bad Name","adapter":"generic"}"#,
        )
        .await;
        assert_eq!(status, 400);
        let (status, ..) = call(h.addr, "POST", "/api/endpoints", &auth, "{nope").await;
        assert_eq!(status, 400);

        let (status, _, body) = call(h.addr, "POST", "/api/endpoints/ci/rotate", &auth, "").await;
        assert_eq!(status, 200);
        assert!(body.contains("cdt_") && !body.contains(&token));
        let (status, ..) = call(
            h.addr,
            "POST",
            "/hook/ci",
            &[("Authorization", tb.as_str())],
            r#"{"title":"x"}"#,
        )
        .await;
        assert_eq!(status, 401, "the old token stops working");

        assert_eq!(
            call(h.addr, "DELETE", "/api/endpoints/ci", &auth, "")
                .await
                .0,
            200
        );
        assert_eq!(
            call(h.addr, "DELETE", "/api/endpoints/ci", &auth, "")
                .await
                .0,
            404
        );
        assert_eq!(call(h.addr, "GET", "/api/nothing", &auth, "").await.0, 404);
    }

    #[tokio::test]
    async fn wrong_administrator_guesses_get_rate_limited() {
        let h = start(Limits::default(), ServeOptions::default()).await;
        admin(&h);
        let bad = bearer("guess");
        let mut limited = false;
        for _ in 0..200 {
            let (status, ..) = call(
                h.addr,
                "GET",
                "/api/overview",
                &[("Authorization", bad.as_str())],
                "",
            )
            .await;
            if status == 429 {
                limited = true;
                break;
            }
        }
        assert!(limited, "guessing must be throttled");
    }

    #[tokio::test]
    async fn status_and_test_speak_need_the_administrator_and_are_bounded() {
        let h = start(Limits::default(), ServeOptions::default()).await;
        let a = admin(&h);
        let auth = [("Authorization", a.as_str())];
        for (m, p) in [("GET", "/api/status"), ("POST", "/api/speak")] {
            assert_eq!(call(h.addr, m, p, &[], "{}").await.0, 401, "{p}");
        }

        h.core
            .set_ui_status(serde_json::json!({"version": "9", "llm": {"configured": false}}));
        let (status, _, body) = call(h.addr, "GET", "/api/status", &auth, "").await;
        assert_eq!(status, 200);
        assert!(
            body.contains(r#""version":"9""#) && body.contains(r#""now""#),
            "{body}"
        );

        let speak = |t: &str| format!(r#"{{"text":"{t}"}}"#);
        assert_eq!(
            call(h.addr, "POST", "/api/speak", &auth, &speak("hello there"))
                .await
                .0,
            202
        );
        assert_eq!(h.core.take_speech().as_deref(), Some("hello there"));
        assert!(h.core.take_speech().is_none());

        for bad in [
            speak(""),
            speak("   "),
            speak(&"x".repeat(201)),
            "{nope".to_owned(),
            r#"{"text":"a","extra":1}"#.to_owned(),
        ] {
            assert_eq!(
                call(h.addr, "POST", "/api/speak", &auth, &bad).await.0,
                400,
                "{bad}"
            );
        }
        // Only a few phrases may wait; the rest are refused rather than piling up.
        let mut refused = 0;
        for _ in 0..10 {
            if call(h.addr, "POST", "/api/speak", &auth, &speak("again"))
                .await
                .0
                == 400
            {
                refused += 1;
            }
        }
        assert_eq!(refused, 10 - crate::app::MAX_PENDING_SPEECH);
    }
}
