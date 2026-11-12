//! HTTP front end: webhook ingress, the SSE reply stream for agent bridges, and acknowledgements
//! (REQ-INB-001/002/008, REQ-SEC-004, REQ-NFR-110).
//!
//! Routes (written once, in [`ROUTES`] here and in `ui::ROUTES` for the portal's `/api`; see `routes.rs`):
//!
//! * `GET  /`, `GET /ui`                          the portal page
//! * `GET  /healthz`
//! * `POST /hook/{endpoint}`                      an agent posts its native hook payload
//! * `POST /api/announce`                         a script says something (REQ-INB-021); endpoint token or admin
//! * `POST /mcp`                                 the ClawDot MCP server (REQ-EXT-011); endpoint token only
//! * `GET  /bridge/{endpoint}/replies`            SSE stream of replies for the agent bridge
//! * `POST /bridge/{endpoint}/replies/{id}/ack`   the bridge confirms it handled a reply
//!
//! Authentication is `Authorization: Bearer <token>` or `X-ClawDot-Token: <token>`. Hook responses are
//! deliberately an empty JSON object: agent runtimes such as Claude Code parse hook responses as
//! *hook output*, so any field we invented could be misread as a decision. The event id travels in the
//! `X-ClawDot-Event` header instead.

use crate::app::{Core, HookError, HookOutcome};
use crate::limit::IpLimiter;
use crate::routes::{Auth, Row, find, path_row};
use http_body_util::{BodyExt, Full, Limited, combinators::BoxBody};
use hyper::body::{Body, Bytes, Frame, Incoming};
use hyper::header::{
    AUTHORIZATION, CONTENT_LENGTH, CONTENT_TYPE, HOST, HeaderMap, HeaderValue, ORIGIN, RETRY_AFTER,
    WWW_AUTHENTICATE,
};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use std::convert::Infallible;
use std::future::Future;
use std::io;
use std::net::IpAddr;
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
    /// Connections one address may hold at once, so a single host cannot take every slot (T12).
    pub max_per_ip: usize,
    /// How long a client may take to send request headers.
    pub header_timeout: Duration,
    /// How long a client may take to send a request body, once its credentials were accepted.
    pub body_timeout: Duration,
    /// Idle time before an SSE comment keeps the connection alive.
    pub heartbeat: Duration,
    /// How long a reply stream may wait for its reader to take data before it is ended, so a client that stops
    /// reading cannot hold one of the stream slots for ever.
    pub write_timeout: Duration,
}

/// How long the web UI waits for a request body.
pub const BODY_TIMEOUT: Duration = Duration::from_secs(5);

impl Default for ServeOptions {
    fn default() -> Self {
        Self {
            max_connections: 32,
            max_streams: 8,
            max_per_ip: 8,
            header_timeout: Duration::from_secs(3),
            body_timeout: BODY_TIMEOUT,
            heartbeat: Duration::from_secs(15),
            write_timeout: Duration::from_secs(30),
        }
    }
}

/// Read a request body of at most `max` bytes within `wait`; `None` if it is too big, unreadable, or too
/// slow (a stalled sender must not hold a connection slot indefinitely, REQ-SEC-004).
pub(crate) async fn read_body(body: Incoming, max: usize, wait: Duration) -> Option<Bytes> {
    let collected = tokio::time::timeout(wait, Limited::new(body, max).collect())
        .await
        .ok()?
        .ok()?;
    Some(collected.to_bytes())
}

/// Serve until `shutdown` completes.
///
/// # Errors
/// None today: a connection that cannot be accepted is logged and tried again. The `Result` stays so that callers
/// need not change if a failure that cannot be survived is ever added.
pub async fn serve(
    listener: TcpListener,
    core: Arc<Core>,
    opts: ServeOptions,
    shutdown: impl Future<Output = ()> + Send,
) -> io::Result<()> {
    let slots = Arc::new(Semaphore::new(opts.max_connections));
    let streams = Arc::new(AtomicUsize::new(0));
    let per_ip = IpLimiter::new(opts.max_per_ip);
    tokio::pin!(shutdown);
    loop {
        let (stream, peer) = tokio::select! {
            accepted = crate::limit::accept_retrying("http", crate::limit::ACCEPT_RETRY, || listener.accept()) => accepted,
            () = &mut shutdown => return Ok(()),
        };
        // Shed load instead of queueing it: a flood must not grow memory (REQ-NFR-110), and one
        // address must not be able to take every slot by opening sockets and saying nothing.
        let Some(ip_guard) = per_ip.try_acquire(peer.ip()) else {
            drop(stream);
            continue;
        };
        let Ok(permit) = Arc::clone(&slots).try_acquire_owned() else {
            drop(stream);
            continue;
        };
        let core = Arc::clone(&core);
        let streams = Arc::clone(&streams);
        let peer = peer.ip();
        tokio::spawn(async move {
            let _permit = permit;
            let _ip_guard = ip_guard;
            let service = service_fn(move |req| {
                route(Arc::clone(&core), Arc::clone(&streams), opts, peer, req)
            });
            let conn = http1::Builder::new()
                .timer(TokioTimer::new())
                .header_read_timeout(opts.header_timeout)
                .serve_connection(TokioIo::new(stream), service);
            let _ = conn.await;
        });
    }
}

fn json(status: StatusCode, body: &'static str) -> Resp {
    let mut r = Response::new(Full::new(Bytes::from_static(body.as_bytes())).boxed());
    *r.status_mut() = status;
    r.headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    r
}

pub(crate) fn token_from(headers: &HeaderMap) -> Option<String> {
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

/// What a route in [`ROUTES`] does. The dispatch in `route` matches on this with no catch-all arm.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Id {
    Page,
    Health,
    Hook,
    Announce,
    Mcp,
    Replies,
    Ack,
}

use Auth::{AnyEndpointToken, EndpointOrAdmin, EndpointToken, Page, Public};

/// Every route the server answers except the portal's `/api` rows, which are in `ui::ROUTES` (see `routes.rs` for why
/// there are two tables). The 404 for an unknown path, the 405 for a wrong method, the class that decides which
/// credential a route needs, the OpenAPI check (`tests/openapi.rs`, through [`documented_routes`]) and the tests of
/// credentials before bodies are all read from the two tables. A path with an `{endpoint}` segment that is not a valid
/// endpoint name is not in the table at all, so it is a 404.
const ROUTES: &[Row<Id>] = &[
    Row::new("GET", "/", Page, Id::Page),
    Row::new("HEAD", "/", Page, Id::Page),
    Row::new("GET", "/ui", Page, Id::Page),
    Row::new("HEAD", "/ui", Page, Id::Page),
    Row::new("GET", "/healthz", Public, Id::Health),
    Row::new("POST", "/hook/{endpoint}", EndpointToken, Id::Hook),
    Row::new("POST", "/api/announce", EndpointOrAdmin, Id::Announce),
    Row::new("POST", "/mcp", AnyEndpointToken, Id::Mcp),
    Row::new(
        "GET",
        "/bridge/{endpoint}/replies",
        EndpointToken,
        Id::Replies,
    ),
    Row::new(
        "POST",
        "/bridge/{endpoint}/replies/{id}/ack",
        EndpointToken,
        Id::Ack,
    ),
];

/// Every route the OpenAPI document must describe, as `(METHOD, "/path/{param}")`: both tables, without the page
/// (which is HTML, not API). For `tests/openapi.rs`.
#[doc(hidden)]
#[must_use]
pub fn documented_routes() -> Vec<(&'static str, &'static str)> {
    ROUTES
        .iter()
        .filter(|r| r.auth != Page)
        .map(|r| (r.method, r.path))
        .chain(crate::ui::api_routes())
        .collect()
}

/// Whether this is a request that changes something under `/api`, sent by a web page from another site.
///
/// In open mode (REQ-CFG-005) the only credential is the caller's address, and a browser on the private network
/// lends its address to any page it shows. Such a page can send a "simple" cross-site POST with no preflight, and
/// the body is parsed whatever its content type. A browser always names the page's origin on a request that
/// changes something, so a request whose `Origin` is not the authority it was sent to is refused. A script sends no
/// `Origin` and is not affected; reads are left to the browser's same-origin rule.
fn cross_site_write(method: &hyper::Method, segs: &[&str], headers: &HeaderMap) -> bool {
    if matches!(
        *method,
        hyper::Method::GET | hyper::Method::HEAD | hyper::Method::OPTIONS
    ) || segs.first() != Some(&"api")
    {
        return false;
    }
    let Some(origin) = headers.get(ORIGIN) else {
        return false;
    };
    let origin_authority = origin
        .to_str()
        .ok()
        .and_then(|o| o.split_once("://"))
        .map(|(_, rest)| rest);
    let host = headers.get(HOST).and_then(|h| h.to_str().ok());
    match (origin_authority, host) {
        (Some(o), Some(h)) => !o.eq_ignore_ascii_case(h),
        _ => true,
    }
}

async fn route(
    core: Arc<Core>,
    streams: Arc<AtomicUsize>,
    opts: ServeOptions,
    peer: IpAddr,
    req: Request<Incoming>,
) -> Result<Resp, Infallible> {
    let (parts, body) = req.into_parts();
    let path = parts.uri.path().to_owned();
    let segs: Vec<&str> = path.trim_matches('/').split('/').collect();
    let headers = &parts.headers;
    if cross_site_write(&parts.method, &segs, headers) {
        return Ok(json(
            StatusCode::FORBIDDEN,
            r#"{"error":"cross-site request refused"}"#,
        ));
    }
    // First decide whether the path names a real resource (invalid endpoint names are simply 404),
    // then whether the method is allowed on it (405).
    let resp = match find(ROUTES, parts.method.as_str(), &segs) {
        Some((row, [name, id])) => match row.id {
            Id::Page => crate::ui::serve_page(headers),
            Id::Health => json(StatusCode::OK, r#"{"status":"ok"}"#),
            Id::Hook => hook(&core, headers, name, body, opts.body_timeout).await,
            Id::Announce => {
                crate::announce_api::handle(&core, headers, peer, body, opts.body_timeout).await
            }
            Id::Mcp => crate::mcp_server::handle(&core, headers, body, opts.body_timeout).await,
            Id::Replies => replies_stream(core, streams, opts, headers, name),
            Id::Ack => ack(&core, headers, name, id),
        },
        None => match path_row(ROUTES, &segs) {
            Some(row) if row.auth == Page => crate::ui::method_not_allowed(),
            Some(_) => method_not_allowed(),
            None => crate::ui::handle(
                &core,
                &parts.method,
                &segs,
                parts.uri.query(),
                headers,
                peer,
                body,
            )
            .await
            .unwrap_or_else(|| json(StatusCode::NOT_FOUND, r#"{"error":"not found"}"#)),
        },
    };
    Ok(resp)
}

async fn hook(
    core: &Core,
    headers: &HeaderMap,
    name: &str,
    body: Incoming,
    body_timeout: Duration,
) -> Resp {
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
    let Some(bytes) = read_body(body, max, body_timeout).await else {
        return json(
            StatusCode::PAYLOAD_TOO_LARGE,
            r#"{"error":"body too large, too slow or unreadable"}"#,
        );
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
            .send_timeout(Bytes::from_static(b"retry: 3000\n\n"), opts.write_timeout)
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
                if tx
                    .send_timeout(Bytes::from(event), opts.write_timeout)
                    .await
                    .is_err()
                {
                    return;
                }
                last_sent = last_sent.max(r.id);
            }
            tokio::select! {
                () = &mut notified => {}
                () = tokio::time::sleep(opts.heartbeat) => {
                    // Nothing else is queued when the heartbeat fires, so a channel that is still full means
                    // the reader has taken nothing for a whole interval: end the stream.
                    if tx.try_send(Bytes::from_static(b": keepalive\n\n")).is_err() {
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
mod tests;
