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
            let builtin = vec!["set_timer".to_owned()];
            let mut ext = crate::extensions::Extensions::in_memory();
            let mut http =
                crate::httptools::HttpTools::new(Vec::new(), &builtin, |_| None).unwrap();
            let (mut mcp, _) = crate::mcp::McpClient::connect(Vec::new(), &builtin, |_| None)
                .await
                .unwrap();
            while let Some(job) = jobs.recv().await {
                let r = match job.op {
                    Op::Sched(op) => assistant.admin_sched(op),
                    Op::Ext(op) => ext
                        .apply(op, &mut http, &mut mcp, &builtin, |n| {
                            (n == "HA_TOKEN").then(|| "x".to_owned())
                        })
                        .await
                        .map(|()| ext.listing(&[], &http, &mcp)),
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
    for word in ["inbox", "scheduled", "radio", "music", "station"] {
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
        ("POST", "/api/tools"),
        ("DELETE", "/api/tools/x"),
        ("POST", "/api/tools/x/enabled"),
        ("POST", "/api/connections"),
        ("POST", "/api/connections/test"),
        ("DELETE", "/api/connections/x"),
        ("POST", "/api/connections/x/enabled"),
        ("GET", "/api/skills"),
        ("POST", "/api/skills"),
        ("POST", "/api/skills/import"),
        ("POST", "/api/skills/edit"),
        ("POST", "/api/skills/delete"),
        ("POST", "/api/skills/enabled"),
    ] {
        // Refused either way; after about 20 failures the rate limit answers 429 instead of 401.
        let s = call(&p, m, path, "", "{}").await.0;
        assert!(s == 401 || s == 429, "{m} {path}: {s}");
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

// REQ-CFG-032, REQ-CFG-033, REQ-CFG-034, REQ-CFG-035, REQ-CFG-036
#[tokio::test]
async fn the_portal_manages_tools_connections_and_skills() {
    let p = start(true).await;
    call(&p, "POST", "/api/setup", "", r#"{"mode":"open"}"#).await;
    let tool = r#"{"name":"set_light","description":"Turn a light on or off","method":"POST","url":"http://ha.local/light/{service}","auth_env":"HA_TOKEN","params":{"service":{"type":"string","required":true}},"confirm":true}"#;
    let (s, v) = call(&p, "POST", "/api/tools", "", tool).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["tools"][0]["name"], "set_light");
    assert_eq!(v["tools"][0]["deletable"], true);
    // A secret value is refused; a token variable that is not set says which one to set.
    let with_secret = tool.replace(r#""confirm":true"#, r#""confirm":true,"token":"abc""#);
    assert_eq!(
        call(&p, "POST", "/api/tools", "", &with_secret).await.0,
        400
    );
    let unset = tool
        .replace("HA_TOKEN", "MISSING_VAR")
        .replace("set_light", "other");
    let (s, v) = call(&p, "POST", "/api/tools", "", &unset).await;
    assert_eq!(s, 400);
    assert!(v["error"].as_str().unwrap().contains("MISSING_VAR"), "{v}");

    let off = r#"{"enabled":false}"#;
    let (s, v) = call(&p, "POST", "/api/tools/set_light/enabled", "", off).await;
    assert_eq!((s, &v["tools"][0]["enabled"]), (200, &json!(false)));
    assert_eq!(
        call(&p, "POST", "/api/tools/nope/enabled", "", off).await.0,
        404
    );
    assert_eq!(
        call(&p, "POST", "/api/tools/set_light/enabled", "", "{}")
            .await
            .0,
        400
    );
    let (s, v) = call(&p, "DELETE", "/api/tools/set_light", "", "").await;
    assert_eq!((s, v["tools"].as_array().unwrap().len()), (200, 0));

    // A connection that cannot be reached is not saved, and the message says what to check.
    let dead = r#"{"name":"home","url":"http://127.0.0.1:9/mcp"}"#;
    let (s, v) = call(&p, "POST", "/api/connections/test", "", dead).await;
    assert_eq!(s, 400);
    assert!(
        v["error"].as_str().unwrap().contains("did not answer"),
        "{v}"
    );
    assert_eq!(call(&p, "POST", "/api/connections", "", dead).await.0, 400);
    assert_eq!(
        call(&p, "DELETE", "/api/connections/home", "", "").await.0,
        404
    );

    // Skills are addressed in the body.
    let skill =
        r#"{"name":"good night","instructions":"Turn off the lights.","tools":[],"enabled":true}"#;
    let (s, v) = call(&p, "POST", "/api/skills", "", skill).await;
    assert_eq!(
        (s, v["skills"][0]["name"].as_str()),
        (200, Some("good night"))
    );
    let (s, v) = call(
        &p,
        "POST",
        "/api/skills/enabled",
        "",
        r#"{"name":"good night","enabled":false}"#,
    )
    .await;
    assert_eq!((s, &v["skills"][0]["enabled"]), (200, &json!(false)));
    let edit = r#"{"name":"good night","skill":{"name":"night","instructions":"Say good night.","enabled":false}}"#;
    let (s, v) = call(&p, "POST", "/api/skills/edit", "", edit).await;
    assert_eq!((s, v["skills"][0]["name"].as_str()), (200, Some("night")));
    let file = r#"{"format":"clawdot-skills","version":1,"skills":[{"name":"morning","instructions":"Read the news.","enabled":true}]}"#;
    let (s, v) = call(&p, "POST", "/api/skills/import", "", file).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        v["skills"][1]["enabled"], false,
        "imported skills start off"
    );
    assert_eq!(
        call(&p, "POST", "/api/skills/import", "", r#"{"hello":1}"#)
            .await
            .0,
        400
    );
    assert_eq!(
        call(&p, "POST", "/api/skills/unknown", "", "{}").await.0,
        404
    );
    let (s, v) = call(&p, "POST", "/api/skills/delete", "", r#"{"name":"night"}"#).await;
    assert_eq!((s, v["skills"].as_array().unwrap().len()), (200, 1));
    let huge = format!(r#"{{"name":"x","instructions":"{}"}}"#, "a".repeat(70_000));
    assert_eq!(call(&p, "POST", "/api/skills", "", &huge).await.0, 413);
}
