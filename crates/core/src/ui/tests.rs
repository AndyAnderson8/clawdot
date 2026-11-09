//! Tests of the portal's JSON API against a real listener (REQ-CFG-005, 027, 029, 030).
#![allow(clippy::unwrap_used)]

use super::*;
use crate::app::{Adapters, Limits};
use crate::assistant::Assistant;
use crate::http::{ServeOptions, serve};
use crate::sched::{Clock, FixedOffset, Scheduler, Ts};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// 2026-11-09 12:00 UTC.
struct Noon;
impl Clock for Noon {
    fn now(&self) -> Ts {
        1_794_225_600
    }
}

struct Portal {
    addr: SocketAddr,
    core: Arc<Core>,
    _stop: tokio::sync::oneshot::Sender<()>,
}

/// A core plus, optionally, a stand-in for the main loop answering portal requests.
async fn start(with_main_loop: bool) -> Portal {
    let core = Arc::new(Core::in_memory(
        Arc::new(Noon),
        Adapters::builtin(),
        Limits::default(),
    ));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (stop, rx) = tokio::sync::oneshot::channel::<()>();
    let served = Arc::clone(&core);
    tokio::spawn(async move {
        let _ = serve(listener, served, ServeOptions::default(), async {
            let _ = rx.await;
        })
        .await;
    });
    if with_main_loop {
        let mut jobs = core.attach_portal();
        let mut assistant = Assistant::new(
            Arc::new(Noon),
            Box::new(FixedOffset(-6 * 3600)),
            Scheduler::new(8),
            None,
            None,
        );
        tokio::spawn(async move {
            while let Some(job) = jobs.recv().await {
                let r = match job.op {
                    Op::Sched(op) => assistant.admin_sched(op),
                };
                let _ = job.reply.send(r);
            }
        });
    }
    Portal {
        addr,
        core,
        _stop: stop,
    }
}

async fn call(p: &Portal, method: &str, path: &str, auth: &str, body: &str) -> (u16, Value) {
    let mut s = TcpStream::connect(p.addr).await.unwrap();
    let mut req = format!(
        "{method} {path} HTTP/1.1\r\nHost: t\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    );
    if !auth.is_empty() {
        req.push_str(&format!("Authorization: {auth}\r\n"));
    }
    req.push_str("\r\n");
    req.push_str(body);
    s.write_all(req.as_bytes()).await.unwrap();
    let mut raw = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut raw))
        .await
        .unwrap()
        .unwrap();
    let text = String::from_utf8_lossy(&raw).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head.split_whitespace().nth(1).unwrap().parse().unwrap();
    (status, serde_json::from_str(body).unwrap_or(Value::Null))
}

// REQ-CFG-025, REQ-CFG-026
#[test]
fn the_page_uses_plain_names_and_no_radio() {
    let page = PAGE.to_lowercase();
    for word in [
        "inbox",
        "scheduled",
        "radio",
        "music",
        "station",
        "endpoints",
    ] {
        assert!(!page.contains(word), "the page still says `{word}`");
    }
    for tab in [
        "Home",
        "Reminders",
        "Agents",
        "Tools",
        "Connections",
        "Skills",
        "Webhooks",
        "Settings",
    ] {
        assert!(PAGE.contains(&format!("\"{tab}\"")), "missing tab {tab}");
    }
    assert!(PAGE.contains(r#"role="tablist""#) && PAGE.contains(r#"id="foot""#));
}

// REQ-CFG-027
#[tokio::test]
async fn the_portal_manages_timers_alarms_and_reminders() {
    let p = start(true).await;
    let open = r#"{"mode":"open"}"#;
    assert_eq!(call(&p, "POST", "/api/setup", "", open).await.0, 200);
    let (s, v) = call(&p, "GET", "/api/schedule", "", "").await;
    assert_eq!((s, v["items"].as_array().unwrap().len()), (200, 0));
    let timer = r#"{"kind":"timer","seconds":300,"label":"tea"}"#;
    let (s, v) = call(&p, "POST", "/api/schedule", "", timer).await;
    assert_eq!(s, 200, "{v}");
    let id = v["id"].as_u64().unwrap();
    let alarm = r#"{"kind":"alarm","hour":7,"minute":30,"days":[0,1,2,3,4],"label":"work"}"#;
    let (s, v) = call(&p, "PUT", &format!("/api/schedule/{id}"), "", alarm).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["items"][0]["kind"], "alarm");
    assert_eq!(v["items"][0]["id"], id);
    let bad = r#"{"kind":"timer","seconds":0}"#;
    let (s, v) = call(&p, "POST", "/api/schedule", "", bad).await;
    assert_eq!(s, 400);
    assert!(v["error"].as_str().unwrap().contains("Timers run"), "{v}");
    let ok = r#"{"kind":"timer","seconds":5}"#;
    assert_eq!(call(&p, "PUT", "/api/schedule/99", "", ok).await.0, 404);
    assert_eq!(call(&p, "DELETE", "/api/schedule/abc", "", "").await.0, 404);
    let path = format!("/api/schedule/{id}");
    let (s, v) = call(&p, "DELETE", &path, "", "").await;
    assert_eq!((s, v["items"].as_array().unwrap().len()), (200, 0));
    assert_eq!(call(&p, "DELETE", &path, "", "").await.0, 404);
}

// REQ-CFG-027
#[tokio::test]
async fn the_portal_says_so_when_the_assistant_is_not_running() {
    let p = start(false).await;
    call(&p, "POST", "/api/setup", "", r#"{"mode":"open"}"#).await;
    let (s, v) = call(&p, "GET", "/api/schedule", "", "").await;
    assert_eq!(s, 503);
    assert!(v["error"].as_str().unwrap().contains("not running"), "{v}");
}

// REQ-CFG-005, REQ-CFG-027, REQ-CFG-029, REQ-CFG-030
#[tokio::test]
async fn every_portal_listing_needs_a_sign_in() {
    let p = start(true).await;
    let setup = r#"{"mode":"password","password":"correct horse"}"#;
    let (_, v) = call(&p, "POST", "/api/setup", "", setup).await;
    let session = format!("Bearer {}", v["token"].as_str().unwrap());
    for (m, path) in [
        ("GET", "/api/schedule"),
        ("POST", "/api/schedule"),
        ("PUT", "/api/schedule/1"),
        ("DELETE", "/api/schedule/1"),
        ("GET", "/api/tools"),
        ("GET", "/api/settings"),
        ("GET", "/api/about"),
    ] {
        assert_eq!(call(&p, m, path, "", "{}").await.0, 401, "{m} {path}");
    }
    p.core
        .set_ui_tools(json!({"tools": [{"name": "set_timer"}], "mcp": []}));
    p.core.set_ui_settings(json!({"zone": "America/Chicago"}));
    let (s, v) = call(&p, "GET", "/api/tools", &session, "").await;
    assert_eq!(
        (s, v["tools"][0]["name"].as_str()),
        (200, Some("set_timer"))
    );
    let (_, v) = call(&p, "GET", "/api/settings", &session, "").await;
    assert_eq!(v["zone"], "America/Chicago");
    let (_, v) = call(&p, "GET", "/api/about", &session, "").await;
    assert_eq!(v["version"], env!("CARGO_PKG_VERSION"));
    assert!(v["changelog"].as_str().unwrap().contains("Changelog"));
    let (_, v) = call(&p, "GET", "/api/auth", "", "").await;
    assert_eq!(
        v["version"],
        env!("CARGO_PKG_VERSION"),
        "the footer shows it before sign-in"
    );
}
