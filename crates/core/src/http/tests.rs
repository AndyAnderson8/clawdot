use super::*;
use crate::app::{Adapters, LOCALHOST_PEER, Limits};
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
    start_with_clock(Arc::new(SystemClock), limits, opts).await
}

/// A clock that never moves, so token-bucket refill cannot depend on how fast the machine is.
struct FrozenClock;
impl crate::sched::Clock for FrozenClock {
    fn monotonic_secs(&self) -> i64 {
        1_800_000_000
    }
    fn now(&self) -> crate::sched::Ts {
        1_800_000_000
    }
}

async fn start_with_clock(
    clock: Arc<dyn crate::sched::Clock + Send + Sync>,
    limits: Limits,
    opts: ServeOptions,
) -> Harness {
    let core = Arc::new(Core::in_memory(clock, Adapters::builtin(), limits));
    start_core(core, opts).await
}

async fn start_core(core: Arc<Core>, opts: ServeOptions) -> Harness {
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
    call_for(addr, "127.0.0.1", method, path, headers, body).await
}

/// Like [`call`], with the `Host` header the request is addressed to.
async fn call_for(
    addr: SocketAddr,
    host: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> (u16, String, String) {
    call_from("127.0.0.1", addr, host, method, path, headers, body).await
}

/// Like [`call_for`], sent from the local address `source` (any 127.x.y.z address is this machine), so a test can
/// be several different clients.
async fn call_from(
    source: &str,
    addr: SocketAddr,
    host: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> (u16, String, String) {
    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    socket.bind(format!("{source}:0").parse().unwrap()).unwrap();
    let mut s = socket.connect(addr).await.unwrap();
    let mut req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\nContent-Length: {}\r\n",
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
async fn an_accepted_event_is_on_disk_before_the_response_arrives() {
    let dir = crate::store::testutil::TempDir::new("http-durable");
    let core = Arc::new(
        Core::open(
            Arc::new(SystemClock),
            Adapters::builtin(),
            Limits::default(),
            crate::app::Stores::in_dir(&dir.0),
        )
        .unwrap(),
    );
    let h = start_core(core, ServeOptions::default()).await;
    let auth = bearer(&h.token);
    let (status, _, _) = call(
        h.addr,
        "POST",
        "/hook/claude-code",
        &[("Authorization", &auth)],
        STOP,
    )
    .await;
    assert_eq!(status, 202);
    // Read at once, with no flush: the response itself must have waited for the write (REQ-INB-004).
    let saved = std::fs::read_to_string(dir.0.join("inbox.json")).unwrap();
    assert!(saved.contains("clawdot finished"), "{saved}");
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
        "POST /hook/claude-code HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: {auth}\r\nContent-Length: 10485760\r\nConnection: close\r\n\r\n{{}}"
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
        "GET /bridge/claude-code/replies HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {token}\r\nAccept: text/event-stream\r\n\r\n"
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
    assert!(buf.starts_with("HTTP/1.1 200") && buf.to_lowercase().contains("text/event-stream"));
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
            .bridge_pending("claude-code", &h.token, LOCALHOST_PEER, 0)
            .unwrap()
            .is_empty()
    );
}

/// True when the server closes `s` (end of stream or reset) within `ms` milliseconds.
async fn closed_within(s: &mut TcpStream, ms: u64) -> bool {
    let mut buf = [0_u8; 512];
    matches!(
        timeout(Duration::from_millis(ms), async {
            loop {
                match s.read(&mut buf).await {
                    Ok(0) | Err(_) => return,
                    Ok(_) => {}
                }
            }
        })
        .await,
        Ok(())
    )
}

#[tokio::test(flavor = "current_thread")]
async fn idle_sockets_cannot_hold_slots_one_address_cannot_take_them_all_and_they_time_out() {
    let h = start(
        Limits::default(),
        ServeOptions {
            max_per_ip: 2,
            header_timeout: Duration::from_millis(300),
            ..ServeOptions::default()
        },
    )
    .await;
    // Two idle sockets use this address's share; the third is refused at once.
    let mut a = TcpStream::connect(h.addr).await.unwrap();
    let mut b = TcpStream::connect(h.addr).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let mut c = TcpStream::connect(h.addr).await.unwrap();
    assert!(closed_within(&mut c, 200).await, "over the per-address cap");
    // The idle ones are dropped by the header timeout, freeing the share again.
    assert!(closed_within(&mut a, 2_000).await && closed_within(&mut b, 2_000).await);
    let (status, _, _) = call(h.addr, "GET", "/healthz", &[], "").await;
    assert_eq!(status, 200);
}

#[tokio::test(flavor = "current_thread")]
async fn a_stalled_request_body_is_cut_off() {
    let h = start(
        Limits::default(),
        ServeOptions {
            body_timeout: Duration::from_millis(300),
            ..ServeOptions::default()
        },
    )
    .await;
    let mut s = TcpStream::connect(h.addr).await.unwrap();
    let head = format!(
        "POST /hook/claude-code HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {}\r\nContent-Length: 1000\r\n\r\n{{\"a\":",
        h.token
    );
    s.write_all(head.as_bytes()).await.unwrap();
    // The server answers (413) and closes instead of waiting for the other 994 bytes.
    let mut got = Vec::new();
    let done = timeout(Duration::from_secs(3), s.read_to_end(&mut got)).await;
    assert!(done.is_ok(), "the stalled body kept the connection open");
    assert!(String::from_utf8_lossy(&got).contains("413"));
    assert!(
        h.core
            .bridge_pending("claude-code", &h.token, LOCALHOST_PEER, 0)
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
async fn a_stream_whose_reader_stopped_reading_ends_and_frees_its_slot() {
    let core = Arc::new(Core::in_memory(
        Arc::new(SystemClock),
        Adapters::builtin(),
        Limits::default(),
    ));
    let (_, token) = core.create_endpoint("claude-code", "claude-code").unwrap();
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&bearer(&token)).unwrap(),
    );
    let streams = Arc::new(AtomicUsize::new(0));
    let opts = ServeOptions {
        heartbeat: Duration::from_millis(5),
        write_timeout: Duration::from_millis(50),
        ..ServeOptions::default()
    };
    // The response body is held but never polled: a client that has stopped reading.
    let response = replies_stream(
        core,
        Arc::clone(&streams),
        opts,
        &headers,
        LOCALHOST_PEER,
        "claude-code",
    );
    assert_eq!(response.status(), 200);
    assert_eq!(streams.load(Ordering::SeqCst), 1);
    let freed = timeout(Duration::from_secs(5), async {
        while streams.load(Ordering::SeqCst) != 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(freed.is_ok(), "the stalled stream must release its slot");
    drop(response);
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

/// Complete setup with a password and return the signed-in session as an Authorization value.
fn admin(h: &Harness) -> String {
    let hash = crate::admin::hash_password("adm-secret").unwrap();
    let token = h
        .core
        .admin_setup_password([127, 0, 0, 1].into(), hash)
        .unwrap();
    bearer(&token)
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
        // (The W3C namespace names that SVG elements are created with are identifiers, not requests.)
        // (Addresses are also named as plain text, such as the example service addresses in Settings; what matters is
        // that the page loads nothing: no script, style, image or font from anywhere, and no CSS url().)
        let no_namespaces = body.replace("http://www.w3.org/", "");
        for load in [
            "src=\"http",
            "src='http",
            "href=\"http://",
            "url(http",
            "url(\"http",
            "@import",
        ] {
            assert!(
                !no_namespaces.contains(load),
                "external reference in page: {load}"
            );
        }
        // The tokens are inlined: light by default, dark only by explicit choice or match-system, reduced motion handled.
        assert!(!body.contains("/*@theme*/"), "theme marker left in page");
        let squeezed = body.replace(": ", ":");
        assert!(
            squeezed.contains("--color-text-primary:")
                && squeezed.contains("[data-theme=dark]")
                && squeezed.contains("prefers-color-scheme:dark")
        );
        assert!(squeezed.contains("prefers-reduced-motion:reduce"));
        assert!(!body.contains("url("), "the page loads nothing by URL");
        // Agent text must only be rendered as text. The bundled Preact runtime mentions `innerHTML`
        // (it supports a raw-HTML prop), so this is enforced on the portal sources instead
        // (portal/test/unit/rules.test.ts forbids every HTML injection API).
        assert!(
            !body.contains("dangerouslySetInnerHTML:"),
            "the page must not pass raw HTML"
        );
    }
}

#[tokio::test]
async fn the_api_needs_a_signed_in_administrator() {
    let h = start(Limits::default(), ServeOptions::default()).await;
    // Setup not done yet: everyone is refused (REQ-CFG-018).
    assert_eq!(call(h.addr, "GET", "/api/overview", &[], "").await.0, 403);
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
    // A voice can be tried for one phrase before it is saved (REQ-CFG-081); a nonsense name is refused.
    let with_voice = |v: &str| format!(r#"{{"text":"hi","voice":"{v}"}}"#);
    assert_eq!(
        call(h.addr, "POST", "/api/speak", &auth, &with_voice("nova"))
            .await
            .0,
        202
    );
    assert_eq!(
        h.core.take_speech_as(),
        Some(("hi".to_owned(), Some("nova".to_owned())))
    );
    assert_eq!(
        call(
            h.addr,
            "POST",
            "/api/speak",
            &auth,
            &with_voice(&"v".repeat(129))
        )
        .await
        .0,
        400
    );
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
            == 409
        {
            refused += 1;
        }
    }
    assert_eq!(refused, 10 - crate::app::MAX_PENDING_SPEECH);
}

/// REQ-CFG-070: a refused test phrase says why and how to fix it; only a queued one answers `ok: true`.
#[tokio::test]
async fn test_speak_says_why_it_stayed_silent() {
    let h = start(Limits::default(), ServeOptions::default()).await;
    let a = admin(&h);
    let auth = [("Authorization", a.as_str())];
    let say = r#"{"text":"hello"}"#;
    let ready = || {
        serde_json::json!({
            "speaker": {"connected": true, "available": true},
            "voice": {"configured": true}, "muted": false,
        })
    };
    let cases = [
        (
            "no_voice",
            503,
            serde_json::json!({"speaker": {"connected": true}, "voice": {"configured": false}}),
        ),
        (
            "no_speaker",
            409,
            serde_json::json!({"speaker": {"connected": false}, "voice": {"configured": true}}),
        ),
        (
            "muted",
            409,
            serde_json::json!({"speaker": {"connected": true, "available": true}, "muted": true}),
        ),
        (
            "busy",
            409,
            serde_json::json!({"speaker": {"connected": true, "available": false}}),
        ),
    ];
    for (reason, code, status) in cases {
        h.core.set_ui_status(status);
        let (s, _, body) = call(h.addr, "POST", "/api/speak", &auth, say).await;
        let v = json_of(&body);
        assert_eq!(s, code, "{reason}: {body}");
        assert_eq!(v["ok"], false, "{reason}");
        assert_eq!(v["reason"], reason, "{body}");
        assert!(
            v["message"].as_str().is_some_and(|m| m.len() > 20),
            "{reason} must explain itself: {body}"
        );
        assert!(
            h.core.take_speech().is_none(),
            "{reason} must queue nothing"
        );
    }
    // A speaker that can speak takes it; so does a status that does not say (not published yet).
    h.core.set_ui_status(ready());
    let (s, _, body) = call(h.addr, "POST", "/api/speak", &auth, say).await;
    assert_eq!(s, 202, "{body}");
    assert_eq!(json_of(&body)["ok"], true);
    h.core.set_ui_status(serde_json::json!({"version": "9"}));
    assert_eq!(call(h.addr, "POST", "/api/speak", &auth, say).await.0, 202);
    // Speech printed to the terminal needs neither a satellite nor a voice.
    h.core.set_ui_status(serde_json::json!({
        "speaker": {"connected": true, "available": true, "name": "console"},
        "voice": {"configured": false},
    }));
    assert_eq!(call(h.addr, "POST", "/api/speak", &auth, say).await.0, 202);
    // The administrator is still required.
    assert_eq!(call(h.addr, "POST", "/api/speak", &[], say).await.0, 401);
}

// ----- first-run setup, sign-in, open mode (REQ-CFG-018..024) -----

fn json_of(body: &str) -> serde_json::Value {
    serde_json::from_str(body).unwrap()
}

async fn post(h: &Harness, path: &str, auth: &str, body: &str) -> (u16, serde_json::Value) {
    let hdrs: Vec<(&str, &str)> = if auth.is_empty() {
        Vec::new()
    } else {
        vec![("Authorization", auth)]
    };
    let (status, _, b) = call(h.addr, "POST", path, &hdrs, body).await;
    (status, serde_json::from_str(&b).unwrap_or_default())
}

async fn get_status(h: &Harness, path: &str, auth: &str) -> u16 {
    let hdrs: Vec<(&str, &str)> = if auth.is_empty() {
        Vec::new()
    } else {
        vec![("Authorization", auth)]
    };
    call(h.addr, "GET", path, &hdrs, "").await.0
}

fn session_of(v: &serde_json::Value) -> String {
    format!("Bearer {}", v["token"].as_str().unwrap())
}

// REQ-CFG-018
#[tokio::test]
async fn first_run_offers_setup_and_refuses_everything_else() {
    let h = start(Limits::default(), ServeOptions::default()).await;
    let (status, _, body) = call(h.addr, "GET", "/api/auth", &[], "").await;
    assert_eq!(status, 200);
    let v = json_of(&body);
    assert_eq!(v["mode"], "setup");
    assert_eq!(v["open_allowed"], true);
    assert_eq!(v["peer_private"], true);
    // Nothing but setup works yet, with or without credentials.
    for (m, p) in [
        ("GET", "/api/overview"),
        ("GET", "/api/status"),
        ("POST", "/api/speak"),
        ("POST", "/api/admin/password"),
    ] {
        assert_eq!(call(h.addr, m, p, &[], "{}").await.0, 403, "{p}");
    }
    assert_eq!(
        post(&h, "/api/login", "", r#"{"password":"x"}"#).await.0,
        403
    );
    // No token wording is left in the page, and it can show the banner (REQ-CFG-018, REQ-CFG-020).
    let (_, _, page) = call(h.addr, "GET", "/", &[], "").await;
    assert!(!page.contains("Administrator token"), "old wording is gone");
    assert!(page.contains("\"banner\"") && page.contains("Unprotected"));
}

// REQ-CFG-018, REQ-CFG-019, REQ-CFG-024
#[tokio::test]
async fn a_password_signs_in_for_a_session_and_can_be_changed() {
    let h = start(Limits::default(), ServeOptions::default()).await;
    for bad in [
        r#"{"mode":"password"}"#,
        r#"{"mode":"password","password":"short"}"#,
        r#"{"mode":"nope"}"#,
        r#"{"mode":"password","password":"long enough","extra":1}"#,
    ] {
        assert_eq!(post(&h, "/api/setup", "", bad).await.0, 400, "{bad}");
    }
    assert_eq!(h.core.admin_state()["mode"], "setup");
    let (status, v) = post(
        &h,
        "/api/setup",
        "",
        r#"{"mode":"password","password":"first password"}"#,
    )
    .await;
    assert_eq!(status, 200);
    let s1 = session_of(&v);
    // Setup cannot be run again by anyone.
    assert_eq!(
        post(&h, "/api/setup", "", r#"{"mode":"open"}"#).await.0,
        409
    );
    assert_eq!(h.core.admin_state()["mode"], "password");
    // The session works; the password itself is not a credential for the API.
    assert_eq!(get_status(&h, "/api/overview", &s1).await, 200);
    assert_eq!(
        get_status(&h, "/api/overview", &bearer("first password")).await,
        401
    );
    // Sign in again: wrong, then right.
    assert_eq!(
        post(&h, "/api/login", "", r#"{"password":"first passwore"}"#)
            .await
            .0,
        401
    );
    let (status, v) = post(&h, "/api/login", "", r#"{"password":"first password"}"#).await;
    assert_eq!(status, 200);
    let s2 = session_of(&v);
    // Sign out ends just that session.
    assert_eq!(post(&h, "/api/logout", &s2, "").await.0, 200);
    assert_eq!(get_status(&h, "/api/overview", &s2).await, 401);
    assert_eq!(get_status(&h, "/api/overview", &s1).await, 200);
    // Changing the password needs a session, checks length, and ends the others.
    assert_eq!(
        post(
            &h,
            "/api/admin/password",
            "",
            r#"{"password":"another one"}"#
        )
        .await
        .0,
        401
    );
    assert_eq!(
        post(&h, "/api/admin/password", &s1, r#"{"password":"tiny"}"#)
            .await
            .0,
        400
    );
    // The current password must be given, and be right (REQ-CFG-024).
    for body in [
        r#"{"password":"another one"}"#,
        r#"{"password":"another one","current":"not the password"}"#,
    ] {
        let (status, v) = post(&h, "/api/admin/password", &s1, body).await;
        assert_eq!(status, 403, "{body}");
        assert!(v["error"].as_str().unwrap().contains("current password"));
    }
    assert_eq!(
        get_status(&h, "/api/overview", &s1).await,
        200,
        "a wrong current password does not end the session"
    );
    let (status, v) = post(
        &h,
        "/api/admin/password",
        &s1,
        r#"{"password":"another one","current":"first password"}"#,
    )
    .await;
    assert_eq!(status, 200);
    let s3 = session_of(&v);
    assert_eq!(get_status(&h, "/api/overview", &s1).await, 401);
    assert_eq!(get_status(&h, "/api/overview", &s3).await, 200);
    assert_eq!(
        post(&h, "/api/login", "", r#"{"password":"first password"}"#)
            .await
            .0,
        401
    );
    assert_eq!(
        post(&h, "/api/login", "", r#"{"password":"another one"}"#)
            .await
            .0,
        200
    );
}

// REQ-CFG-020, REQ-CFG-024, REQ-INB-002
#[tokio::test]
async fn open_mode_needs_no_sign_in_but_webhooks_still_need_tokens() {
    let h = start(Limits::default(), ServeOptions::default()).await;
    assert_eq!(
        post(&h, "/api/setup", "", r#"{"mode":"open"}"#).await.0,
        200
    );
    let (_, _, body) = call(h.addr, "GET", "/api/auth", &[], "").await;
    assert_eq!(json_of(&body)["mode"], "open");
    assert_eq!(get_status(&h, "/api/overview", "").await, 200);
    assert_eq!(
        post(&h, "/api/login", "", r#"{"password":"anything"}"#)
            .await
            .0,
        409,
        "there is no password to sign in with"
    );
    // Endpoint tokens stay required (REQ-INB-002).
    assert_eq!(
        call(h.addr, "POST", "/hook/claude-code", &[], STOP).await.0,
        401
    );
    let bad = bearer("cdt_wrong");
    let hdr = [("Authorization", bad.as_str())];
    assert_eq!(
        call(h.addr, "POST", "/hook/claude-code", &hdr, STOP)
            .await
            .0,
        401
    );
    let good = bearer(&h.token);
    let hdr = [("Authorization", good.as_str())];
    assert_eq!(
        call(h.addr, "POST", "/hook/claude-code", &hdr, STOP)
            .await
            .0,
        202
    );
    // From open mode, a password can be set without signing in first.
    let (status, v) = post(
        &h,
        "/api/admin/password",
        "",
        r#"{"password":"now protected"}"#,
    )
    .await;
    assert_eq!(status, 200);
    assert!(v["token"].is_string());
    assert_eq!(get_status(&h, "/api/overview", "").await, 401);
}

// REQ-CFG-021
#[tokio::test]
async fn setup_needs_no_code_when_listening_beyond_loopback() {
    let h = start(Limits::default(), ServeOptions::default()).await;
    h.core.set_listen_ip([192, 168, 1, 20].into());
    let (_, _, body) = call(h.addr, "GET", "/api/auth", &[], "").await;
    assert!(json_of(&body).get("code_needed").is_none());
    // A stale client that still sends a code is refused as a malformed request.
    let old = r#"{"mode":"open","code":"0000-0000"}"#;
    assert_eq!(post(&h, "/api/setup", "", old).await.0, 400);
    assert_eq!(h.core.admin_state()["mode"], "setup");
    let ok = r#"{"mode":"password","password":"no code needed"}"#;
    assert_eq!(post(&h, "/api/setup", "", ok).await.0, 200);
    assert_eq!(h.core.admin_state()["mode"], "password");
}

// REQ-CFG-020
#[tokio::test]
async fn open_mode_is_refused_on_a_public_listen_address() {
    let h = start(Limits::default(), ServeOptions::default()).await;
    h.core.set_listen_ip([203, 0, 113, 9].into());
    let (_, _, body) = call(h.addr, "GET", "/api/auth", &[], "").await;
    assert_eq!(json_of(&body)["open_allowed"], false);
    let open = r#"{"mode":"open"}"#;
    assert_eq!(post(&h, "/api/setup", "", open).await.0, 403);
    let pw = r#"{"mode":"password","password":"still fine"}"#;
    assert_eq!(post(&h, "/api/setup", "", pw).await.0, 200);
}

// REQ-CFG-019
#[tokio::test]
async fn sign_in_attempts_are_rate_limited() {
    // A frozen clock: the bucket cannot refill however slow each (PBKDF2) attempt is, so the burst is
    // spent after exactly `auth_fail_burst` attempts on any machine.
    let limits = Limits::default();
    let burst = limits.auth_fail_burst as usize;
    let h = start_with_clock(Arc::new(FrozenClock), limits, ServeOptions::default()).await;
    admin(&h);
    for i in 0..burst {
        let (status, ..) = post(&h, "/api/login", "", r#"{"password":"guess guess"}"#).await;
        assert_eq!(status, 401, "attempt {i} is within the burst");
    }
    let (status, ..) = post(&h, "/api/login", "", r#"{"password":"guess guess"}"#).await;
    assert_eq!(status, 429, "password guessing must be throttled");
}

// REQ-CFG-023
#[tokio::test]
async fn an_old_administrator_token_signs_in_like_a_password() {
    let h = start(Limits::default(), ServeOptions::default()).await;
    let old = crate::inbox::endpoint::generate_token().unwrap();
    let hash = crate::inbox::endpoint::hash_token(&old);
    h.core
        .admin_setup_password([127, 0, 0, 1].into(), hash)
        .unwrap();
    let (_, _, body) = call(h.addr, "GET", "/api/auth", &[], "").await;
    let v = json_of(&body);
    assert_eq!(v["mode"], "password");
    assert_eq!(v["legacy"], true);
    let login = format!(r#"{{"password":"{old}"}}"#);
    let (status, v) = post(&h, "/api/login", "", &login).await;
    assert_eq!(status, 200);
    let s = session_of(&v);
    let new = format!(r#"{{"password":"a real password","current":"{old}"}}"#);
    assert_eq!(post(&h, "/api/admin/password", &s, &new).await.0, 200);
    let (_, _, body) = call(h.addr, "GET", "/api/auth", &[], "").await;
    assert_eq!(json_of(&body)["legacy"], false);
    assert_eq!(
        post(&h, "/api/login", "", &login).await.0,
        401,
        "the old token stops working once replaced"
    );
}

// The table is the one list of routes (architecture review C4), here for everything but the portal's `/api`.
#[test]
fn the_server_table_is_well_formed_and_its_classes_are_the_ones_expected() {
    crate::routes::assert_well_formed(ROUTES);
    let by = |a: Auth| -> Vec<String> {
        ROUTES
            .iter()
            .filter(|r| r.auth == a)
            .map(|r| format!("{} {}", r.method, r.path))
            .collect()
    };
    assert_eq!(by(Page), ["GET /", "HEAD /", "GET /ui", "HEAD /ui"]);
    assert_eq!(by(Public), ["GET /healthz"]);
    assert_eq!(
        by(EndpointToken),
        [
            "POST /hook/{endpoint}",
            "GET /bridge/{endpoint}/replies",
            "POST /bridge/{endpoint}/replies/{id}/ack"
        ]
    );
    assert_eq!(by(EndpointOrAdmin), ["POST /api/announce"]);
    assert_eq!(by(AnyEndpointToken), ["POST /mcp"]);
    // No class the portal's own table owns.
    assert!(by(Auth::Open).is_empty() && by(Auth::SignedIn).is_empty());
    // The two tables share no route, so the order in which they are tried cannot matter.
    for (m, p) in crate::ui::api_routes() {
        assert!(
            !ROUTES.iter().any(|r| r.method == m && r.path == p),
            "{m} {p} is in both tables"
        );
    }
}

/// Send the head of a request that promises a body, never send the body, and return the status line if the
/// server answers before its body timeout (a server that read the body first would wait the five seconds out).
async fn answered_without_the_body(
    addr: SocketAddr,
    method: &str,
    path: &str,
    header: Option<(&str, &str)>,
) -> Option<u16> {
    let mut s = TcpStream::connect(addr).await.unwrap();
    let mut req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Length: 64\r\n"
    );
    if let Some((k, v)) = header {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    s.write_all(req.as_bytes()).await.unwrap();
    let mut buf = [0_u8; 64];
    let n = timeout(Duration::from_millis(1500), s.read(&mut buf))
        .await
        .ok()?
        .unwrap();
    String::from_utf8_lossy(&buf[..n])
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
}

// REQ-SEC-004, REQ-INB-002: every route that takes a token refuses a missing or wrong one before reading a body.
#[tokio::test]
async fn every_token_route_refuses_a_missing_or_wrong_token_before_reading_the_body() {
    let h = start(Limits::default(), ServeOptions::default()).await;
    // Sign the portal in, so a request without a session is refused as 401 and not as "set up first".
    let _ = admin(&h);
    let guarded: Vec<_> = ROUTES
        .iter()
        .filter(|r| {
            matches!(
                r.auth,
                Auth::EndpointToken | Auth::AnyEndpointToken | Auth::EndpointOrAdmin
            )
        })
        .collect();
    assert!(guarded.len() >= 5, "only {} token routes", guarded.len());
    for route in guarded {
        // The endpoint in the path exists, so the refusal is about the token, not about the name.
        let path = crate::routes::sample_path(&route.path.replace("{endpoint}", "claude-code"));
        let wrong = bearer("cdt_not-the-token");
        for header in [None, Some(("Authorization", wrong.as_str()))] {
            let s = answered_without_the_body(h.addr, route.method, &path, header).await;
            assert!(
                matches!(s, Some(401 | 429)),
                "{} {path} with {header:?}: {s:?}",
                route.method
            );
        }
    }
}

// The ways the routes differ today. These are pinned, not endorsed: a change here is a behaviour change.
#[tokio::test]
async fn the_known_differences_between_routes_are_pinned() {
    let h = start(Limits::default(), ServeOptions::default()).await;
    let _ = admin(&h);
    let big = ("Content-Length", "10485760");
    // 1. Body routes check the declared size before the credentials, so an oversized request is 413 even with
    //    no token; the stream and the ack take no body and never answer 413.
    for path in ["/hook/claude-code", "/api/announce", "/mcp"] {
        let mut s = TcpStream::connect(h.addr).await.unwrap();
        let req = format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n{}: {}\r\n\r\n",
            big.0, big.1
        );
        s.write_all(req.as_bytes()).await.unwrap();
        let mut raw = Vec::new();
        timeout(Duration::from_secs(5), s.read_to_end(&mut raw))
            .await
            .unwrap()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&raw).starts_with("HTTP/1.1 413"),
            "{path}"
        );
    }
    // 2. The ack checks that the id is a number before it checks the token: a bad id is 400 with a wrong token
    //    but 401 with none.
    let wrong = bearer("cdt_wrong");
    let bad_id = "/bridge/claude-code/replies/x/ack";
    let (s, ..) = call(h.addr, "POST", bad_id, &[("Authorization", &wrong)], "").await;
    assert_eq!(s, 400);
    assert_eq!(call(h.addr, "POST", bad_id, &[], "").await.0, 401);
    // 3. A wrong method is 405 with no credential on the server's own routes, while the portal's routes ask
    //    for the sign-in first (401) and its open routes answer 404.
    assert_eq!(
        call(h.addr, "GET", "/hook/claude-code", &[], "").await.0,
        405
    );
    assert_eq!(call(h.addr, "GET", "/api/announce", &[], "").await.0, 405);
    assert_eq!(call(h.addr, "GET", "/mcp", &[], "").await.0, 405);
    assert_eq!(call(h.addr, "HEAD", "/healthz", &[], "").await.0, 405);
    assert_eq!(call(h.addr, "GET", "/api/overview", &[], "").await.0, 401);
    assert_eq!(call(h.addr, "PUT", "/api/overview", &[], "").await.0, 401);
    assert_eq!(call(h.addr, "GET", "/api/login", &[], "").await.0, 404);
    // 4. The 405 for the page carries the portal's cache headers; the 405 elsewhere does not.
    let (s, head, _) = call(h.addr, "POST", "/", &[], "").await;
    assert_eq!(s, 405);
    assert!(head.to_lowercase().contains("cache-control: no-store"));
    let (s, head, _) = call(h.addr, "POST", "/healthz", &[], "").await;
    assert_eq!(s, 405);
    assert!(!head.to_lowercase().contains("cache-control"));
    // 5. An endpoint name that is not valid is not a route (404); a valid one that does not exist is a 401.
    let hdr = [("Authorization", wrong.as_str())];
    assert_eq!(call(h.addr, "POST", "/hook/UPPER", &hdr, "{}").await.0, 404);
    assert_eq!(
        call(h.addr, "GET", "/bridge/UPPER/replies", &hdr, "")
            .await
            .0,
        404
    );
    assert_eq!(
        call(h.addr, "POST", "/hook/nobody", &hdr, "{}").await.0,
        401
    );
}

// REQ-CFG-005, threat T5: in open mode the only credential is the caller's address, which a web page in a browser on
// the same network borrows. A "simple" cross-site POST needs no preflight and the body is parsed whatever its type, so
// a state-changing request that names a different origin than the one it was sent to must be refused.
#[tokio::test]
async fn a_web_page_on_another_site_cannot_drive_the_portal_in_open_mode() {
    let h = start(Limits::default(), ServeOptions::default()).await;
    let evil = [("Origin", "http://evil.example")];
    // Before setup: a page must not be able to choose the mode for the owner.
    let (s, _, _) = call(h.addr, "POST", "/api/setup", &evil, r#"{"mode":"open"}"#).await;
    assert_eq!(s, 403);
    let (_, _, body) = call(h.addr, "GET", "/api/auth", &[], "").await;
    assert_eq!(json_of(&body)["mode"], "setup", "nothing was set up");
    assert_eq!(
        post(&h, "/api/setup", "", r#"{"mode":"open"}"#).await.0,
        200
    );

    let make = r#"{"name":"csrf","adapter":"generic"}"#;
    for origin in [
        "http://evil.example",
        "null",
        "http://127.0.0.1.evil.example",
        "http://127.0.0.1:1",
    ] {
        let hdrs = [("Origin", origin)];
        let (s, _, _) = call(h.addr, "POST", "/api/endpoints", &hdrs, make).await;
        assert_eq!(s, 403, "{origin}");
    }
    assert!(
        h.core.endpoints().iter().all(|e| e.name != "csrf"),
        "no cross-site request changed anything"
    );
    // The announce route takes the administrator path in open mode, so it is covered too.
    let say = r#"{"text":"hello","endpoint":"claude-code"}"#;
    assert_eq!(
        call(h.addr, "POST", "/api/announce", &evil, say).await.0,
        403
    );
    // A browser labels a request made by a page on another site, whatever its method (an image or a script tag is
    // a GET with no Origin). Such a request is refused before it can have any effect.
    for path in [
        "/api/overview",
        "/api/models?url=https://evil.example&key_env=PATH",
    ] {
        let labelled = [("Sec-Fetch-Site", "cross-site")];
        assert_eq!(
            call(h.addr, "GET", path, &labelled, "").await.0,
            403,
            "{path}"
        );
    }
    for site in ["same-origin", "none"] {
        let labelled = [("Sec-Fetch-Site", site)];
        let (s, _, _) = call(h.addr, "GET", "/api/overview", &labelled, "").await;
        assert_eq!(s, 200, "{site}");
    }
    // An older browser sends no label. The model list then still takes nothing from the address: with no saved
    // provider it asks nobody, whatever address and variable the request names.
    let (s, _, body) = call(
        h.addr,
        "GET",
        "/api/models?url=https://evil.example&key_env=PATH",
        &[],
        "",
    )
    .await;
    assert_eq!(s, 409, "{body}");
    // Reading without that label is left to the browser's same-origin rule, and the portal itself (origin = host)
    // works, as does a script that sends no Origin at all.
    assert_eq!(call(h.addr, "GET", "/api/overview", &evil, "").await.0, 200);
    let same = [("Origin", "http://127.0.0.1")];
    assert_eq!(
        call(h.addr, "POST", "/api/endpoints", &same, make).await.0,
        201
    );
    let (s, _, _) = call(h.addr, "POST", "/api/announce", &[], say).await;
    assert_eq!(s, 202);
}

// REQ-CFG-005, threat T5: DNS rebinding. The attacker's own name is re-resolved to the device, so the page, the
// device and the Origin all agree; only the Host header still names the attacker. In open mode the address is the
// credential, so the portal answers only to names that are the device's.
#[tokio::test]
async fn a_rebound_name_cannot_reach_the_portal_but_the_devices_own_names_can() {
    // The machine name is injected: the request path answers for the kept value and reads no file or variable.
    let core = Core::in_memory(
        Arc::new(SystemClock),
        Adapters::builtin(),
        Limits::default(),
    )
    .with_machine_name(Some("Speaker.Example.Lan"));
    let h = start_core(Arc::new(core), ServeOptions::default()).await;
    assert_eq!(
        post(&h, "/api/setup", "", r#"{"mode":"open"}"#).await.0,
        200
    );
    let port = h.addr.port();
    let ours = vec![
        format!("speaker.example.lan:{port}"),
        format!("speaker:{port}"),
        format!("speaker.local:{port}"),
        format!("127.0.0.1:{port}"),
        format!("192.168.1.20:{port}"),
        format!("[::1]:{port}"),
        format!("localhost:{port}"),
        "10.0.0.7".to_owned(),
    ];
    for host in &ours {
        for (method, path) in [("GET", "/"), ("GET", "/api/overview"), ("GET", "/api/auth")] {
            let (s, _, _) = call_for(h.addr, host, method, path, &[], "").await;
            assert_eq!(s, 200, "{host} {method} {path}");
        }
    }
    let make = r#"{"name":"rebind","adapter":"generic"}"#;
    for host in [
        format!("evil.example:{port}"),
        "evil.example".to_owned(),
        format!("localhost.evil.example:{port}"),
        format!("192.168.1.20.evil.example:{port}"),
        format!("{port}"),
    ] {
        for (method, path, body) in [
            ("GET", "/", ""),
            ("HEAD", "/", ""),
            ("GET", "/ui", ""),
            ("GET", "/api/overview", ""),
            ("GET", "/api/auth", ""),
            ("POST", "/api/endpoints", make),
            ("DELETE", "/api/endpoints/claude-code", ""),
            ("GET", "/api/nothing-here", ""),
            // An announcement with no token takes the administrator path, which open mode leaves to the address.
            (
                "POST",
                "/api/announce",
                r#"{"text":"hi","endpoint":"claude-code"}"#,
            ),
        ] {
            let (s, _, text) = call_for(h.addr, &host, method, path, &[], body).await;
            assert_eq!(s, 403, "{host} {method} {path}");
            if method != "HEAD" {
                assert!(text.contains("public address"), "{text}");
            }
        }
    }
    assert!(
        h.core.endpoints().iter().all(|e| e.name != "rebind"),
        "nothing was changed"
    );
}

// A core answers for its own kept name only: a core built with no name (or another one) refuses "speaker".
#[tokio::test]
async fn the_kept_machine_name_is_per_core() {
    let unnamed = start(Limits::default(), ServeOptions::default()).await;
    let other = Core::in_memory(
        Arc::new(SystemClock),
        Adapters::builtin(),
        Limits::default(),
    )
    .with_machine_name(Some("other"));
    let other = start_core(Arc::new(other), ServeOptions::default()).await;
    for h in [&unnamed, &other] {
        assert_eq!(post(h, "/api/setup", "", r#"{"mode":"open"}"#).await.0, 200);
        let host = format!("speaker:{}", h.addr.port());
        assert_eq!(
            call_for(h.addr, &host, "GET", "/api/auth", &[], "").await.0,
            403
        );
    }
    let host = format!("other:{}", other.addr.port());
    assert_eq!(
        call_for(other.addr, &host, "GET", "/api/auth", &[], "")
            .await
            .0,
        200
    );
}

// Routes that carry their own token cannot be driven by a rebound page, so the Host check leaves them alone.
#[tokio::test]
async fn token_routes_and_the_health_check_work_under_any_host_name() {
    let h = start(Limits::default(), ServeOptions::default()).await;
    assert_eq!(
        post(&h, "/api/setup", "", r#"{"mode":"open"}"#).await.0,
        200
    );
    let auth = bearer(&h.token);
    let hdr = [("Authorization", auth.as_str())];
    let any = "agent.example.net:3335";
    assert_eq!(
        call_for(h.addr, any, "GET", "/healthz", &[], "").await.0,
        200
    );
    assert_eq!(
        call_for(h.addr, any, "POST", "/hook/claude-code", &hdr, STOP)
            .await
            .0,
        202
    );
    assert_eq!(
        call_for(
            h.addr,
            any,
            "POST",
            "/api/announce",
            &hdr,
            r#"{"text":"hello"}"#
        )
        .await
        .0,
        202,
        "an announcement with a token"
    );
    assert_eq!(
        call_for(
            h.addr,
            any,
            "POST",
            "/bridge/claude-code/replies/999/ack",
            &hdr,
            ""
        )
        .await
        .0,
        404,
        "the bridge answers (no such reply); it is not refused for its host"
    );
    assert_eq!(
        call_for(h.addr, any, "POST", "/mcp", &[], "{}").await.0,
        401,
        "the MCP server asks for its token"
    );
    let mut stream = TcpStream::connect(h.addr).await.unwrap();
    let req = format!(
        "GET /bridge/claude-code/replies HTTP/1.1\r\nHost: {any}\r\nAuthorization: Bearer {}\r\n\r\n",
        h.token
    );
    stream.write_all(req.as_bytes()).await.unwrap();
    let mut buf = String::new();
    read_until(&mut stream, &mut buf, "retry:").await;
}

// REQ-SEC-004: failed attempts are counted per source address. Address A guessing until it is refused leaves
// address B able to present the right token and the administrator able to sign in with the right password.
#[tokio::test(flavor = "current_thread")]
async fn one_clients_failures_do_not_lock_out_another_client() {
    let h = start_with_clock(
        Arc::new(FrozenClock),
        Limits::default(),
        ServeOptions::default(),
    )
    .await;
    admin(&h);
    let (a, b) = ("127.0.0.2", "127.0.0.3");
    let wrong = bearer("cdt_wrong");
    let right = bearer(&h.token);
    let burst = Limits::default().auth_fail_burst as usize;
    async fn from(h: &Harness, who: &str, path: &str, auth: &str, body: &str) -> u16 {
        let hdrs = [("Authorization", auth)];
        let hdrs: &[(&str, &str)] = if auth.is_empty() { &[] } else { &hdrs };
        call_from(who, h.addr, "127.0.0.1", "POST", path, hdrs, body)
            .await
            .0
    }
    for i in 0..burst {
        let status = from(&h, a, "/hook/claude-code", &wrong, STOP).await;
        assert_eq!(status, 401, "attempt {i}");
    }
    let password = r#"{"password":"adm-secret"}"#;
    assert_eq!(from(&h, a, "/hook/claude-code", &wrong, STOP).await, 429);
    assert_eq!(
        from(&h, a, "/api/login", "", password).await,
        429,
        "A is throttled"
    );
    assert_eq!(
        from(&h, b, "/hook/claude-code", &right, STOP).await,
        202,
        "B's token works"
    );
    assert_eq!(
        from(&h, b, "/api/login", "", password).await,
        200,
        "B signs in"
    );
    assert_eq!(
        from(&h, b, "/hook/claude-code", &wrong, STOP).await,
        401,
        "B is still within its own budget"
    );
}

/// A core with its files in `dir`, served, with one endpoint (`claude-code`).
async fn start_stored(dir: &crate::store::testutil::TempDir) -> Harness {
    let core = Arc::new(
        Core::open(
            Arc::new(SystemClock),
            Adapters::builtin(),
            Limits::default(),
            crate::app::Stores::in_dir(&dir.0),
        )
        .unwrap(),
    );
    start_core(core, ServeOptions::default()).await
}

/// Make every later save of `name` in `dir` fail: a directory sits where its temporary file would be written.
fn break_saves(dir: &crate::store::testutil::TempDir, name: &str) -> std::path::PathBuf {
    let blocker = crate::store::temp_path(&dir.0.join(name));
    std::fs::create_dir(&blocker).unwrap();
    blocker
}

// REQ-INB-004: a webhook is answered 202 only if it was saved. When it could not be, the answer is a 5xx with a
// plain message (the event is still held in memory and announced), and the next one that can be saved is a 202.
#[tokio::test(flavor = "current_thread")]
async fn a_webhook_that_could_not_be_saved_is_not_answered_202() {
    let dir = crate::store::testutil::TempDir::new("hook-unsaved");
    let h = start_stored(&dir).await;
    let auth = bearer(&h.token);
    let hdr = [("Authorization", auth.as_str())];
    h.core.flush_blocking();
    let blocker = break_saves(&dir, "inbox.json");
    let (status, _, body) = call(h.addr, "POST", "/hook/claude-code", &hdr, STOP).await;
    assert_eq!(status, 500, "{body}");
    assert!(
        json_of(&body)["error"]
            .as_str()
            .unwrap()
            .contains("could not be saved")
    );
    assert_eq!(
        h.core.events().len(),
        1,
        "the event is kept and will be announced"
    );
    std::fs::remove_dir(&blocker).unwrap();
    let other = STOP.replace("s1", "s2");
    let (status, ..) = call(h.addr, "POST", "/hook/claude-code", &hdr, &other).await;
    assert_eq!(status, 202, "saving works again");
    let (status, ..) = call(h.addr, "GET", "/healthz", &[], "").await;
    assert_eq!(status, 200, "requests that save nothing are unaffected");
}

// The same for an announcement, a bridge acknowledgement and an endpoint made in the portal.
#[tokio::test(flavor = "current_thread")]
async fn announcements_acks_and_portal_endpoints_that_could_not_be_saved_are_5xx() {
    let dir = crate::store::testutil::TempDir::new("others-unsaved");
    let h = start_stored(&dir).await;
    let auth = bearer(&h.token);
    let hdr = [("Authorization", auth.as_str())];
    let session = admin(&h);
    let portal = [("Authorization", session.as_str())];

    // Announcement.
    let blocker = break_saves(&dir, "inbox.json");
    let say = r#"{"text":"hello"}"#;
    let (status, ..) = call(h.addr, "POST", "/api/announce", &hdr, say).await;
    assert_eq!(status, 500);
    std::fs::remove_dir(&blocker).unwrap();
    let again = r#"{"text":"hello again"}"#;
    let (status, ..) = call(h.addr, "POST", "/api/announce", &hdr, again).await;
    assert_eq!(status, 202);

    // Acknowledgement of a reply.
    h.core
        .ingest_hook("claude-code", &h.token, LOCALHOST_PEER, STOP.as_bytes())
        .unwrap();
    let id = h.core.events().last().unwrap().id;
    h.core.mark_announced(&[id]);
    h.core
        .reply(crate::inbox::reply::ReplyKind::Instruct, "go on")
        .unwrap();
    h.core.flush_blocking();
    let reply = h
        .core
        .bridge_pending("claude-code", &h.token, LOCALHOST_PEER, 0)
        .unwrap()[0]
        .id;
    let blocker = break_saves(&dir, "replies.json");
    let path = format!("/bridge/claude-code/replies/{reply}/ack");
    let (status, ..) = call(h.addr, "POST", &path, &hdr, "").await;
    assert_eq!(status, 500, "an acknowledgement that was not saved");
    std::fs::remove_dir(&blocker).unwrap();

    // An endpoint made in the portal is taken back, so that trying again works.
    let blocker = break_saves(&dir, "endpoints.json");
    let new = r#"{"name":"second","adapter":"generic"}"#;
    let (status, _, body) = call(h.addr, "POST", "/api/endpoints", &portal, new).await;
    assert_eq!(status, 500, "{body}");
    assert!(h.core.endpoints().iter().all(|e| e.name != "second"));
    std::fs::remove_dir(&blocker).unwrap();
    let (status, ..) = call(h.addr, "POST", "/api/endpoints", &portal, new).await;
    assert_eq!(status, 201);
}
