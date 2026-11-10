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
    let defaults = [
        ("device_name", json!("Test Dot")),
        ("zone", json!("UTC")),
        ("listen", json!("127.0.0.1:3335")),
        ("llm_key_env", json!("OPENROUTER_API_KEY")),
        ("max_requests_per_day", json!(500)),
        ("board", json!("none")),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect();
    core.set_settings(crate::settings::SettingsState::in_memory(
        defaults,
        false,
        Box::new(|n| (n == "MY_KEY").then(|| "x".to_owned())),
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
                    Op::Settings(live) => Ok(Value::Object(live)),
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
    // A radio *input* (the theme choice) is fine; radio stations are not.
    let page = PAGE
        .to_lowercase()
        .replace("type:\"radio\"", "")
        .replace("type=radio", "");
    for word in ["inbox", "scheduled", "radio", "music", "station"] {
        assert!(!page.contains(word), "the page still says `{word}`");
    }
    // Home first, then Clock, Bluetooth and Settings; everything configurable, the Features switches included, is under Settings.
    for tab in ["Home", "Clock", "Bluetooth", "Settings"] {
        assert!(
            PAGE.contains(&format!("\"{tab}\"")),
            "missing destination {tab}"
        );
    }
    for gone in ["\"Agents\"", "\"Webhooks\"", "#/agents"] {
        assert!(!PAGE.contains(gone), "the Agents tab is gone: {gone}");
    }
    // The page is built from TypeScript (portal/), so the markup lives in the script.
    assert!(PAGE.contains("\"foot\""));
}

// REQ-CFG-071: the portal never tells anyone to run a command
#[test]
fn the_sign_in_help_never_tells_anyone_to_run_a_command() {
    assert!(
        !PAGE.contains("reset-admin") && !PAGE.to_lowercase().contains("recovery code"),
        "the page tells people to run a command"
    );
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
        ("PUT", "/api/settings"),
        ("GET", "/api/about"),
        ("POST", "/api/tools"),
        ("DELETE", "/api/tools/x"),
        ("POST", "/api/tools/x/enabled"),
        ("POST", "/api/tools/x/tier"),
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
        ("GET", "/api/update"),
        ("POST", "/api/update/check"),
        ("POST", "/api/update/apply"),
    ] {
        // Refused either way; after about 20 failures the rate limit answers 429 instead of 401.
        let s = call(&p, m, path, "", "{}").await.0;
        assert!(s == 401 || s == 429, "{m} {path}: {s}");
    }
    p.core
        .set_ui_tools(json!({"tools": [{"name": "set_timer"}], "mcp": []}));
    let (s, v) = call(&p, "GET", "/api/tools", &session, "").await;
    assert_eq!(
        (s, v["tools"][0]["name"].as_str()),
        (200, Some("set_timer"))
    );
    let (_, v) = call(&p, "GET", "/api/settings", &session, "").await;
    assert_eq!(v["fields"]["zone"]["value"], "UTC");
    assert_eq!(v["fields"]["zone"]["source"], "default");
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
    // The administrator picks a tier (REQ-SEC-030): lowering, going back, and the refusals.
    let tier = |t: &str| format!(r#"{{"tier":{t}}}"#);
    let (s, v) = call(
        &p,
        "POST",
        "/api/tools/set_light/tier",
        "",
        &tier("\"reversible\""),
    )
    .await;
    assert_eq!(
        (s, &v["tools"][0]["tier"], &v["tools"][0]["tier_set"]),
        (200, &json!("reversible"), &json!(true)),
        "{v}"
    );
    let (s, v) = call(&p, "POST", "/api/tools/set_light/tier", "", &tier("null")).await;
    assert_eq!((s, &v["tools"][0]["tier"]), (200, &json!("costly")));
    for (path, body, want) in [
        ("/api/tools/set_light/tier", tier("\"destructive\""), 400),
        ("/api/tools/set_light/tier", tier("\"harmless\""), 400),
        ("/api/tools/set_light/tier", "{}".to_owned(), 400),
        ("/api/tools/nope/tier", tier("\"costly\""), 404),
    ] {
        assert_eq!(
            call(&p, "POST", path, "", &body).await.0,
            want,
            "{path} {body}"
        );
    }
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

// REQ-CFG-037, REQ-CFG-038, REQ-CFG-039, REQ-CFG-040, REQ-CFG-041
#[tokio::test]
async fn settings_are_saved_validated_and_need_the_administrator() {
    let p = start(true).await;
    let setup = r#"{"mode":"password","password":"correct horse"}"#;
    let (_, v) = call(&p, "POST", "/api/setup", "", setup).await;
    let session = format!("Bearer {}", v["token"].as_str().unwrap());
    let change = r#"{"zone":"Europe/Berlin","max_requests_per_day":20}"#;
    // Not signed in, or signed in with a wrong token: refused, and nothing changes.
    for auth in ["", "Bearer wrong"] {
        let s = call(&p, "PUT", "/api/settings", auth, change).await.0;
        assert!(s == 401 || s == 429, "{auth}: {s}");
    }
    let (_, v) = call(&p, "GET", "/api/settings", &session, "").await;
    assert_eq!(v["fields"]["zone"]["value"], "UTC");
    // A bad value says what is wrong, and the good half is not applied.
    let bad = r#"{"zone":"Europe/Berlin","listen":"everywhere"}"#;
    let (s, v) = call(&p, "PUT", "/api/settings", &session, bad).await;
    assert_eq!(s, 400);
    assert!(
        v["error"].as_str().unwrap().contains("127.0.0.1:3335"),
        "{v}"
    );
    let (_, v) = call(&p, "GET", "/api/settings", &session, "").await;
    assert_eq!(v["fields"]["zone"]["value"], "UTC");
    // A good change is saved; the answer is the new listing, with what needs a restart.
    let (s, v) = call(&p, "PUT", "/api/settings", &session, change).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["fields"]["zone"]["value"], "Europe/Berlin");
    assert_eq!(v["fields"]["zone"]["source"], "saved");
    assert_eq!(v["restart_needed"], json!([]));
    let (s, v) = call(
        &p,
        "PUT",
        "/api/settings",
        &session,
        r#"{"listen":"0.0.0.0:4000"}"#,
    )
    .await;
    assert_eq!(s, 200);
    assert_eq!(v["restart_needed"], json!(["listen", "public_url"]));
    // A secret value in a key field is refused with the rule, and never echoed back.
    let (s, v) = call(
        &p,
        "PUT",
        "/api/settings",
        &session,
        r#"{"llm_key_env":"sk-live-123"}"#,
    )
    .await;
    assert_eq!(s, 400);
    assert!(!v.to_string().contains("sk-live-123"));
    assert!(v["error"].as_str().unwrap().contains("NAME"), "{v}");
    let (s, v) = call(
        &p,
        "PUT",
        "/api/settings",
        &session,
        r#"{"llm_key_env":"MY_KEY"}"#,
    )
    .await;
    assert_eq!((s, v["llm"]["key_present"].as_bool()), (200, Some(true)));
}

// REQ-CFG-043
#[test]
fn the_page_is_compressed_only_for_browsers_that_accept_it() {
    for (h, gz) in [
        (None, false),
        (Some("identity"), false),
        (Some("br, deflate"), false),
        (Some("gzip;q=0"), false),
        (Some("gzip"), true),
        (Some("deflate, GZIP;q=0.5"), true),
        (Some("*"), true),
    ] {
        assert_eq!(accepts_gzip(h), gz, "{h:?}");
    }
    let plain = page(None);
    assert!(plain.headers().get("content-encoding").is_none());
    assert_eq!(plain.headers()["vary"], "Accept-Encoding");
    let zipped = page(Some("gzip"));
    assert_eq!(zipped.headers()["content-encoding"], "gzip");
    assert_eq!(zipped.headers()["vary"], "Accept-Encoding");
    // Both carry the security headers, and the gzip copy is a real gzip stream much smaller than the page.
    for r in [&plain, &zipped] {
        assert!(r.headers().contains_key("content-security-policy"));
    }
    assert_eq!(&PAGE_GZ[..2], &[0x1f, 0x8b]);
    assert!(
        // The page is mostly English copy and a runtime, so it compresses about 2.9 to 1.
        PAGE_GZ.len() * 5 < PAGE.len() * 2,
        "{} vs {}",
        PAGE_GZ.len(),
        PAGE.len()
    );
}

// REQ-CFG-043
#[tokio::test]
async fn the_served_page_negotiates_the_encoding() {
    let p = start(false).await;
    for (accept, encoded) in [("", false), ("Accept-Encoding: gzip\r\n", true)] {
        let mut s = TcpStream::connect(p.addr).await.unwrap();
        let req = format!("GET / HTTP/1.1\r\nHost: t\r\nConnection: close\r\n{accept}\r\n");
        s.write_all(req.as_bytes()).await.unwrap();
        let mut raw = Vec::new();
        s.read_to_end(&mut raw).await.unwrap();
        let split = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        let head = String::from_utf8_lossy(&raw[..split]).to_lowercase();
        assert_eq!(head.contains("content-encoding: gzip"), encoded, "{head}");
        let body = &raw[split + 4..];
        assert_eq!(body.starts_with(&[0x1f, 0x8b]), encoded);
        if !encoded {
            assert!(String::from_utf8_lossy(body).contains("<!doctype html>"));
        }
    }
}

// REQ-SEC-014
#[tokio::test]
async fn the_audit_log_is_read_through_an_authenticated_paged_endpoint() {
    use crate::audit::{Act, AuditConfig, AuditLog, decision};
    use crate::session::Tier;
    let p = start(false).await;
    // nothing to read until the assistant has a log; and never without a sign-in
    let setup = r#"{"mode":"password","password":"correct horse"}"#;
    let (_, v) = call(&p, "POST", "/api/setup", "", setup).await;
    let session = format!("Bearer {}", v["token"].as_str().unwrap());
    let s = call(&p, "GET", "/api/audit", "", "").await.0;
    assert!(s == 401 || s == 429, "{s}");
    let s = call(&p, "GET", "/api/audit", "Bearer not-the-token", "")
        .await
        .0;
    assert!(s == 401 || s == 429, "{s}");
    assert_eq!(call(&p, "GET", "/api/audit", &session, "").await.0, 503);
    let log = Arc::new(AuditLog::in_memory(AuditConfig::default()));
    p.core.set_audit(Arc::clone(&log));
    for i in 0..6_i64 {
        let (tool, tier, d) = if i % 2 == 0 {
            ("set_timer", Tier::Reversible, decision::RAN)
        } else {
            ("docker__restart", Tier::Costly, decision::HELD)
        };
        let args = json!({"n": i, "api_key": "sk-secret-0123456789"});
        let a = Act {
            tool,
            tier: Some(tier),
            args: &args,
            decision: d,
            held_id: None,
            outcome: None,
            note: None,
        };
        log.record(1000 + i, 1, 1, "voice", None, &a);
    }
    async fn audit_get(p: &Portal, session: &str, q: &str) -> (u16, Value) {
        call(p, "GET", &format!("/api/audit{q}"), session, "").await
    }
    let get = |q: &'static str| audit_get(&p, &session, q);
    let (s, v) = get("?limit=2").await;
    assert_eq!(s, 200);
    let ids: Vec<u64> = v["records"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_u64().unwrap())
        .collect();
    assert_eq!(ids, [6, 5]);
    assert_eq!(v["next_before"], 5);
    assert_eq!(v["policy"]["dry_run"], false);
    assert!(!v.to_string().contains("sk-secret"), "{v}");
    let (_, v) = get("?limit=2&before=5").await;
    assert_eq!(v["records"][0]["id"], 4);
    let (_, v) = get("?tool=docker__restart&decision=held").await;
    assert_eq!(v["records"].as_array().unwrap().len(), 3);
    let (_, v) = get("?tier=reversible&since=1004").await;
    assert_eq!(v["records"].as_array().unwrap().len(), 1);
    let (_, v) = get("?trigger=voice&decision=blocked%3A").await;
    assert_eq!(v["records"].as_array().unwrap().len(), 0);
    let (s, v) = get("?limit=lots").await;
    assert_eq!(s, 400);
    assert!(v["error"].as_str().unwrap().contains("limit"));
    // read-only: the log cannot be changed or cleared through the API
    for m in ["POST", "PUT", "DELETE"] {
        assert_eq!(
            call(&p, m, "/api/audit", &session, "{}").await.0,
            405,
            "{m}"
        );
    }
}

// REQ-CFG-044, REQ-AST-023
#[tokio::test]
async fn notes_are_listed_and_forgotten_through_an_authenticated_endpoint() {
    use crate::notes::NoteStore;
    let p = start(false).await;
    let setup = r#"{"mode":"password","password":"correct horse"}"#;
    let (_, v) = call(&p, "POST", "/api/setup", "", setup).await;
    let session = format!("Bearer {}", v["token"].as_str().unwrap());
    // never without a sign-in, and nothing to read until the assistant keeps notes
    let s = call(&p, "GET", "/api/notes", "", "").await.0;
    assert!(s == 401 || s == 429, "{s}");
    assert_eq!(call(&p, "GET", "/api/notes", &session, "").await.0, 503);
    let store = Arc::new(NoteStore::in_memory(true));
    p.core.set_notes(Arc::clone(&store));
    store.add("my printer is called Pixel", 10).unwrap();
    let second = store.add("the spare key is under the pot", 11).unwrap();
    let (s, v) = call(&p, "GET", "/api/notes", &session, "").await;
    assert_eq!(s, 200);
    assert_eq!(v["enabled"], true);
    assert_eq!(v["notes"].as_array().unwrap().len(), 2);
    assert_eq!(v["notes"][0]["text"], "my printer is called Pixel");
    assert_eq!(v["limits"]["max_notes"], 50);
    // forget one by id, then a missing one, then all
    let path = format!("/api/notes/{}", second.id);
    assert_eq!(call(&p, "DELETE", &path, &session, "").await.0, 200);
    assert_eq!(call(&p, "DELETE", &path, &session, "").await.0, 404);
    assert_eq!(
        call(&p, "DELETE", "/api/notes/nope", &session, "").await.0,
        404
    );
    assert_eq!(store.list().len(), 1);
    let (s, v) = call(&p, "DELETE", "/api/notes", &session, "").await;
    assert_eq!((s, v["forgotten"].as_u64()), (200, Some(1)));
    assert!(store.list().is_empty());
    // only the signed-in administrator writes through the page; other methods are refused
    assert_eq!(call(&p, "PUT", "/api/notes", &session, "{}").await.0, 405);
}

// REQ-CFG-061, REQ-SEC-017
#[tokio::test]
async fn the_administrator_adds_a_note_through_the_same_checks_as_a_spoken_one() {
    use crate::notes::NoteStore;
    let p = start(false).await;
    let setup = r#"{"mode":"password","password":"correct horse"}"#;
    let (_, v) = call(&p, "POST", "/api/setup", "", setup).await;
    let session = format!("Bearer {}", v["token"].as_str().unwrap());
    let add = |text: &str| format!(r#"{{"text":{}}}"#, json!(text));
    // never without a sign-in, and before notes exist
    let s = call(&p, "POST", "/api/notes", "", &add("x")).await.0;
    assert!(s == 401 || s == 429, "{s}");
    assert_eq!(
        call(&p, "POST", "/api/notes", &session, &add("x")).await.0,
        503
    );
    // off: refused with the reason, nothing written
    let store = Arc::new(NoteStore::in_memory(false));
    p.core.set_notes(Arc::clone(&store));
    let (s, v) = call(
        &p,
        "POST",
        "/api/notes",
        &session,
        &add("my printer is Pixel"),
    )
    .await;
    assert_eq!(s, 409);
    assert!(v["error"].as_str().unwrap().contains("turned off"), "{v}");
    assert!(store.list().is_empty());
    // on: saved, and the answer is the new list
    store.set_enabled(true);
    let (s, v) = call(
        &p,
        "POST",
        "/api/notes",
        &session,
        &add("  my printer is   Pixel "),
    )
    .await;
    assert_eq!(s, 200);
    assert_eq!(v["notes"][0]["text"], "my printer is Pixel");
    assert_eq!(v["notes"][0]["ts"], 1_794_225_600);
    // the same refusals as the spoken path, each with a plain reason and no write
    for (body, status, needle) in [
        (add("my printer is pixel"), 409, "already saved"),
        (add("   "), 400, "Write what"),
        (add(&"a".repeat(201)), 400, "too long"),
        (
            add("the key is sk-abcdefghijklmnopqrstuvwxyz123456"),
            400,
            "password or a key",
        ),
        (r#"{"text":5}"#.to_owned(), 400, "expected"),
        (r#"{"text":"a","extra":1}"#.to_owned(), 400, "expected"),
    ] {
        let (s, v) = call(&p, "POST", "/api/notes", &session, &body).await;
        assert_eq!(s, status, "{body}");
        assert!(v["error"].as_str().unwrap().contains(needle), "{body}: {v}");
    }
    assert_eq!(store.list().len(), 1);
}

// REQ-CFG-063, REQ-CFG-064, REQ-CFG-065, REQ-CFG-066
#[tokio::test]
async fn diagnostics_logs_export_and_build_info_need_a_sign_in_and_hold_no_secret() {
    let p = start(false).await;
    let setup = r#"{"mode":"password","password":"correct horse"}"#;
    let (_, v) = call(&p, "POST", "/api/setup", "", setup).await;
    let session = format!("Bearer {}", v["token"].as_str().unwrap());
    for path in ["/api/diagnostics", "/api/logs", "/api/settings/export"] {
        let s = call(&p, "GET", path, "", "").await.0;
        assert!(s == 401 || s == 429, "{path}: {s}");
    }
    // An endpoint with a token, a saved setting with a secret-looking name, and a long persona.
    let (_, e) = call(
        &p,
        "POST",
        "/api/endpoints",
        &session,
        r#"{"name":"ci","adapter":"generic"}"#,
    )
    .await;
    let token = e["token"].as_str().unwrap().to_owned();
    let (s, _) = call(
        &p,
        "PUT",
        "/api/settings",
        &session,
        r#"{"device_name":"Den","persona":"Secret persona text"}"#,
    )
    .await;
    assert_eq!(s, 200);
    crate::diag::error(
        "diag-api-test could not do a thing with Bearer abcdefghijklmnopqrstuvwxyz123456",
    );

    let (s, d) = call(&p, "GET", "/api/diagnostics", &session, "").await;
    assert_eq!(s, 200, "{d}");
    assert_eq!(d["format"], "clawdot-diagnostics");
    assert_eq!(d["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(d["settings"]["fields"]["device_name"]["value"], "Den");
    assert_eq!(d["settings"]["fields"]["device_name"]["source"], "saved");
    assert_eq!(d["endpoints"][0]["name"], "ci");
    assert!(d["build"]["os"].is_string());
    let text = d.to_string();
    for forbidden in [
        token.as_str(),
        "token_hash",
        "Secret persona text",
        "abcdefghijklmnopqrstuvwxyz123456",
    ] {
        assert!(
            !text.contains(forbidden),
            "diagnostics must not hold {forbidden}"
        );
    }
    assert!(
        text.contains("diag-api-test"),
        "recent problems are included"
    );

    let (s, l) = call(&p, "GET", "/api/logs?limit=5", &session, "").await;
    assert_eq!(s, 200);
    assert!(l["lines"].as_array().unwrap().len() <= 5);
    assert_eq!(l["max_lines"], crate::diag::MAX_LINES);
    assert!(!l.to_string().contains("abcdefghijklmnopqrstuvwxyz123456"));

    let (s, x) = call(&p, "GET", "/api/settings/export", &session, "").await;
    assert_eq!(s, 200);
    assert_eq!(x["format"], "clawdot-settings");
    assert_eq!(x["settings"]["device_name"], "Den");
    assert!(
        x["settings"].get("zone").is_none(),
        "only what was saved is exported"
    );
    // Importing is the same call as saving, with the same checks.
    let (s, _) = call(
        &p,
        "PUT",
        "/api/settings",
        &session,
        &x["settings"].to_string(),
    )
    .await;
    assert_eq!(s, 200);
    let (s, v) = call(
        &p,
        "PUT",
        "/api/settings",
        &session,
        r#"{"zone":"Mars/Base"}"#,
    )
    .await;
    assert_eq!(s, 400, "{v}");

    let (_, a) = call(&p, "GET", "/api/about", &session, "").await;
    assert_eq!(a["build"]["version"], env!("CARGO_PKG_VERSION"));
    assert!(a["build"]["features"].is_array());
}

// ----- forgotten password: local reset window (REQ-CFG-069); there is no recovery code -----

fn token_of(v: &Value) -> String {
    format!("Bearer {}", v["token"].as_str().unwrap())
}

/// Set a password through the API; returns the session.
async fn set_up_with_password(p: &Portal) -> String {
    let setup = r#"{"mode":"password","password":"correct horse"}"#;
    let (s, v) = call(p, "POST", "/api/setup", "", setup).await;
    assert_eq!(s, 200);
    assert!(v.get("recovery_code").is_none(), "no recovery code exists");
    token_of(&v)
}

fn login_body(password: &str) -> String {
    format!(r#"{{"password":"{password}"}}"#)
}

// The recovery-code endpoints are gone.
#[tokio::test]
async fn the_recovery_code_endpoints_are_gone() {
    let p = start(false).await;
    let session = set_up_with_password(&p).await;
    let (_, auth) = call(&p, "GET", "/api/auth", "", "").await;
    assert!(auth.get("recovery").is_none());
    assert_eq!(auth["reset_window_secs"], 0);
    for path in ["/api/recover", "/api/admin/recovery"] {
        let (s, _) = call(&p, "POST", path, &session, "{}").await;
        assert_eq!(s, 404, "{path}");
    }
    // Changing the password does not issue a code either.
    let (s, v) = call(
        &p,
        "POST",
        "/api/admin/password",
        &session,
        &login_body("changed password"),
    )
    .await;
    assert_eq!(s, 200);
    assert!(v.get("recovery_code").is_none());
}

// An older install that was in open mode can still move to a password.
#[tokio::test]
async fn setting_a_password_in_open_mode_works_and_shows_no_code() {
    let p = start(false).await;
    let (s, _) = call(&p, "POST", "/api/setup", "", r#"{"mode":"open"}"#).await;
    assert_eq!(s, 200);
    let (s, v) = call(
        &p,
        "POST",
        "/api/admin/password",
        "",
        &login_body("first password"),
    )
    .await;
    assert_eq!(s, 200);
    assert!(v.get("recovery_code").is_none());
}

// REQ-CFG-072
#[tokio::test]
async fn open_mode_has_no_reset_window() {
    let p = start(false).await;
    let (s, _) = call(&p, "POST", "/api/setup", "", r#"{"mode":"open"}"#).await;
    assert_eq!(s, 200);
    assert_eq!(
        call(
            &p,
            "POST",
            "/api/recover/window",
            "",
            &login_body("new password 1")
        )
        .await
        .0,
        409
    );
    assert!(p.core.admin_open_reset_window().is_err());
    // Still open, still no password.
    assert_eq!(call(&p, "GET", "/api/auth", "", "").await.1["mode"], "open");
}

// REQ-CFG-069
#[tokio::test]
async fn the_local_reset_window_sets_a_password_once() {
    let p = start(false).await;
    let session = set_up_with_password(&p).await;
    let body = login_body("window password");
    // Closed: refused.
    assert_eq!(
        call(&p, "POST", "/api/recover/window", "", &body).await.0,
        403
    );
    // The device asks (the satellite path calls this); the page learns the window is open.
    assert_eq!(p.core.admin_open_reset_window().unwrap(), 300);
    let (_, auth) = call(&p, "GET", "/api/auth", "", "").await;
    assert_eq!(auth["reset_window_secs"], 300);
    // A weak password does not use the window up.
    assert_eq!(
        call(&p, "POST", "/api/recover/window", "", &login_body("short"))
            .await
            .0,
        400
    );
    let (s, v) = call(&p, "POST", "/api/recover/window", "", &body).await;
    assert_eq!(s, 200, "{v}");
    assert!(v.get("recovery_code").is_none());
    assert_eq!(call(&p, "GET", "/api/about", &session, "").await.0, 401);
    assert_eq!(
        call(&p, "GET", "/api/about", &token_of(&v), "").await.0,
        200
    );
    // One use: the window is shut.
    assert_eq!(
        call(&p, "POST", "/api/recover/window", "", &body).await.0,
        403
    );
    let (_, auth) = call(&p, "GET", "/api/auth", "", "").await;
    assert_eq!(auth["reset_window_secs"], 0);
    // A second request for a window right away is held off by the cooldown.
    assert!(p.core.admin_open_reset_window().is_err());
}

// REQ-UPD-005, REQ-UPD-006
#[tokio::test]
async fn updates_need_the_administrator_and_say_so_when_they_are_not_running() {
    let p = start(true).await;
    let setup = r#"{"mode":"password","password":"correct horse"}"#;
    let (_, v) = call(&p, "POST", "/api/setup", "", setup).await;
    let session = format!("Bearer {}", v["token"].as_str().unwrap());
    // No updater runs in this test, so each call says so plainly instead of failing oddly.
    for (m, path, body) in [
        ("GET", "/api/update", ""),
        ("POST", "/api/update/check", "{}"),
        ("POST", "/api/update/apply", r#"{"version":"9.9.9"}"#),
    ] {
        let (s, v) = call(&p, m, path, &session, body).await;
        assert_eq!(s, 503, "{m} {path}: {v}");
        assert!(
            v["error"]
                .as_str()
                .unwrap()
                .to_lowercase()
                .contains("update"),
            "{v}"
        );
    }
    assert_eq!(call(&p, "GET", "/api/update", "", "").await.0, 401);
}
