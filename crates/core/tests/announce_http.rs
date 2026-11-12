//! `POST /api/announce` over a real socket (REQ-INB-021, 022, 025). The inbox rules themselves are tested
//! in `app::announce`; this checks the HTTP surface: credentials before the body, statuses and bounds.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use clawdot_core::app::{Adapters, Core, Limits};
use clawdot_core::http::{ServeOptions, serve};
use clawdot_core::sched::SystemClock;
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::time::timeout;

struct Harness {
    core: Arc<Core>,
    addr: SocketAddr,
    token: String,
    _stop: oneshot::Sender<()>,
}

async fn start() -> Harness {
    let core = Arc::new(Core::in_memory(
        Arc::new(SystemClock),
        Adapters::builtin(),
        Limits::default(),
    ));
    let (_, token) = core.create_endpoint("house", "generic").unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = oneshot::channel::<()>();
    let served = Arc::clone(&core);
    tokio::spawn(async move {
        let _ = serve(listener, served, ServeOptions::default(), async {
            let _ = rx.await;
        })
        .await;
    });
    Harness {
        core,
        addr,
        token,
        _stop: tx,
    }
}

async fn call(
    addr: SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> (u16, String) {
    let mut s = TcpStream::connect(addr).await.unwrap();
    let mut req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Length: {}\r\n",
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
    (status, body.to_owned())
}

async fn announce(h: &Harness, auth: &[(&str, &str)], body: &Value) -> (u16, Value) {
    let (status, text) = call(h.addr, "POST", "/api/announce", auth, &body.to_string()).await;
    (status, serde_json::from_str(&text).unwrap_or(Value::Null))
}

fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

// REQ-INB-021, REQ-INB-025
#[tokio::test(flavor = "current_thread")]
async fn an_endpoint_token_queues_a_message_and_the_reply_never_echoes_the_text() {
    let h = start().await;
    let auth = bearer(&h.token);
    let (status, v) = announce(
        &h,
        &[("Authorization", &auth)],
        &json!({"text": "Dinner is ready", "source": "Kitchen", "priority": "urgent"}),
    )
    .await;
    assert_eq!(status, 202);
    assert_eq!(v, json!({"queued": true, "id": 1}));
    let ev = h.core.next_announcement(true, 0).unwrap().event;
    assert_eq!(
        (ev.source.as_str(), ev.text.as_str()),
        ("Kitchen", "Dinner is ready")
    );

    // The alternative header works too.
    let (status, _) = announce(
        &h,
        &[("X-ClawDot-Token", &h.token)],
        &json!({"text": "again"}),
    )
    .await;
    assert_eq!(status, 202);
}

// REQ-INB-021, REQ-SEC-008
#[tokio::test(flavor = "current_thread")]
async fn missing_and_wrong_credentials_get_the_same_401_before_the_body_is_read() {
    let h = start().await;
    let wrong = bearer("cdt_wrong");
    let session_like = bearer("not-a-session");
    for headers in [
        vec![],
        vec![("Authorization", wrong.as_str())],
        vec![("Authorization", session_like.as_str())],
    ] {
        let (status, _) = announce(&h, &headers, &json!({"text": "x"})).await;
        // Even before an administrator is set up, a caller learns nothing beyond "no".
        assert_eq!(status, 401, "{headers:?}");
    }
    // A huge declared body is refused without being read, with or without credentials.
    let big = "x".repeat(40_000);
    let auth = bearer(&h.token);
    let (status, _) = call(
        h.addr,
        "POST",
        "/api/announce",
        &[("Authorization", &auth)],
        &big,
    )
    .await;
    assert_eq!(status, 413);
    assert!(h.core.events().is_empty());
}

// REQ-INB-025
#[tokio::test(flavor = "current_thread")]
async fn bad_requests_get_a_400_naming_the_field_and_other_methods_are_refused() {
    let h = start().await;
    let auth = bearer(&h.token);
    let hdr = [("Authorization", auth.as_str())];
    for (body, field) in [
        (json!({}), "text"),
        (json!({"text": "x".repeat(2001)}), "text"),
        (json!({"text": "x", "priority": "shout"}), "priority"),
        (json!({"text": "x", "ttl_secs": 999_999}), "ttl_secs"),
        (json!({"text": "x", "tool": "set_timer"}), "unknown field"),
    ] {
        let (status, v) = announce(&h, &hdr, &body).await;
        assert_eq!(status, 400, "{body}");
        let msg = v["error"].as_str().unwrap();
        assert!(msg.contains(field), "{msg}");
        assert!(!msg.contains("xxxx"), "the text is never echoed");
    }
    let (status, _) = call(h.addr, "POST", "/api/announce", &hdr, "{nope").await;
    assert_eq!(status, 400);
    assert_eq!(call(h.addr, "GET", "/api/announce", &hdr, "").await.0, 405);
    assert!(h.core.events().is_empty());
}

// REQ-INB-022, REQ-INB-025
#[tokio::test(flavor = "current_thread")]
async fn the_endpoint_filter_applies_and_is_reported_without_queueing() {
    let h = start().await;
    h.core.update_filter("house", |f| f.muted = true);
    let auth = bearer(&h.token);
    let (status, v) = announce(&h, &[("Authorization", &auth)], &json!({"text": "psst"})).await;
    assert_eq!(status, 202);
    assert_eq!(v, json!({"queued": false, "filtered": "muted"}));
    assert!(h.core.events().is_empty());
}

// REQ-INB-021
#[tokio::test(flavor = "current_thread")]
async fn an_administrator_session_names_the_endpoint() {
    let h = start().await;
    let hash = clawdot_core::admin::hash_password("adm-secret").unwrap();
    let session = h
        .core
        .admin_setup_password([127, 0, 0, 1].into(), hash)
        .unwrap();
    let auth = bearer(&session);
    let hdr = [("Authorization", auth.as_str())];
    let (status, v) = announce(&h, &hdr, &json!({"text": "hello"})).await;
    assert_eq!(status, 400, "{v}");
    let (status, v) = announce(&h, &hdr, &json!({"text": "hello", "endpoint": "house"})).await;
    assert_eq!((status, v), (202, json!({"queued": true, "id": 1})));
}

// REQ-INB-026
#[tokio::test(flavor = "current_thread")]
async fn the_say_command_reaches_the_server_and_explains_a_refusal() {
    let h = start().await;
    let url = format!("http://{}", h.addr);
    let args: Vec<String> = ["Dinner is ready", "--source", "Kitchen", "--url", &url]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    let say = clawdot_core::say::parse(&args, |_| None).unwrap();
    let line = clawdot_core::say::run(&say, Some(&h.token)).await.unwrap();
    assert_eq!(line, "queued");
    assert_eq!(h.core.events()[0].source, "Kitchen");

    let e = clawdot_core::say::run(&say, Some("cdt_wrong"))
        .await
        .unwrap_err();
    assert!(e.contains("token"), "{e}");
    h.core.update_filter("house", |f| f.muted = true);
    let line = clawdot_core::say::run(&say, Some(&h.token)).await.unwrap();
    assert!(line.contains("muted"), "{line}");
}
