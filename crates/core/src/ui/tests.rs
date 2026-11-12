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
    start_in(None, with_main_loop).await
}

/// Like [`start`], with the core's files in `dir` when given (so a pasted key has somewhere to be kept).
async fn start_in(dir: Option<&std::path::Path>, with_main_loop: bool) -> Portal {
    let core = Arc::new(match dir {
        None => Core::in_memory(Arc::new(Noon), Adapters::builtin(), Limits::default()),
        Some(d) => Core::open(
            Arc::new(Noon),
            Adapters::builtin(),
            Limits::default(),
            crate::app::Stores::in_dir(d),
        )
        .unwrap(),
    });
    let defaults = [
        ("device_name", json!("Test Dot")),
        ("zone", json!("UTC")),
        ("listen", json!("127.0.0.1:3335")),
        ("llm_key_env", json!("OPENROUTER_API_KEY")),
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
                    Op::Rekey => Ok(json!({ "applied": true })),
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
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Length: {}\r\n",
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
fn the_page_uses_plain_names() {
    // Music and radio appear only on the Media page, which says nothing can play yet.
    for word in ["inbox", "scheduled"] {
        assert!(
            !PAGE.to_lowercase().contains(word),
            "the page still says `{word}`"
        );
    }
    // Home first, then Clock, Media and Settings; everything configurable, the Features switches included, is under Settings.
    for tab in ["Home", "Clock", "Media", "Settings"] {
        assert!(
            PAGE.contains(&format!("\"{tab}\"")),
            "missing destination {tab}"
        );
    }
    for gone in ["\"Agents\"", "#/agents"] {
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
    // Every route the table marks as needing a sign-in, read from the table so a new route cannot be forgotten here.
    let guarded: Vec<_> = ROUTES.iter().filter(|r| r.auth == SignedIn).collect();
    assert!(guarded.len() > 40, "only {} guarded routes", guarded.len());
    for route in guarded {
        let path = crate::routes::sample_path(route.path);
        // Refused either way; after about 20 failures the rate limit answers 429 instead of 401.
        let s = call(&p, route.method, &path, "", "{}").await.0;
        assert!(s == 401 || s == 429, "{} {path}: {s}", route.method);
    }
    // The extra skill actions are one row in the table (`/skills/{action}`); each is guarded the same way.
    for action in ["import", "edit", "delete", "enabled"] {
        let s = call(&p, "POST", &format!("/api/skills/{action}"), "", "{}")
            .await
            .0;
        assert!(s == 401 || s == 429, "skills/{action}: {s}");
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
    // REQ-SKL-001, REQ-SKL-002: a skill folder sent as files, then accepted
    let folder = r#"{"format":"skill-folder","files":[{"path":"tea/SKILL.md","text":"---\nname: tea\ndescription: Brew tea\n---\nSteep it."},{"path":"tea/a.md","text":"notes"}]}"#;
    let (s, v) = call(&p, "POST", "/api/skills/import", "", folder).await;
    assert_eq!(s, 200, "{v}");
    let tea = v["skills"]
        .as_array()
        .unwrap()
        .iter()
        .find(|k| k["name"] == "tea")
        .unwrap();
    assert_eq!(
        (
            tea["description"].as_str(),
            tea["needs_review"].as_bool(),
            tea["source"].as_str()
        ),
        (Some("Brew tea"), Some(true), Some("tea/SKILL.md"))
    );
    assert_eq!(v["import"]["added"], 1);
    let on = r#"{"name":"tea","enabled":true}"#;
    assert_eq!(call(&p, "POST", "/api/skills/enabled", "", on).await.0, 400);
    let (s, v) = call(&p, "POST", "/api/skills/accept", "", r#"{"name":"tea"}"#).await;
    assert_eq!(s, 200);
    assert!(
        v["skills"]
            .as_array()
            .unwrap()
            .iter()
            .any(|k| k["name"] == "tea" && k["needs_review"] == false)
    );
    assert_eq!(call(&p, "POST", "/api/skills/enabled", "", on).await.0, 200);
    call(&p, "POST", "/api/skills/delete", "", r#"{"name":"tea"}"#).await;
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
// Uses named time zones, which only a build with the time zone database accepts.
#[cfg(feature = "tz")]
#[tokio::test]
async fn settings_are_saved_validated_and_need_the_administrator() {
    let p = start(true).await;
    let setup = r#"{"mode":"password","password":"correct horse"}"#;
    let (_, v) = call(&p, "POST", "/api/setup", "", setup).await;
    let session = format!("Bearer {}", v["token"].as_str().unwrap());
    let change = r#"{"zone":"Europe/Berlin","daily_spend_usd":5}"#;
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
        let req = format!("GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n{accept}\r\n");
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
    // One decoder for every query: a broken escape is refused here too, not read as written.
    let (_, v) = get("?tool=docker%5F%5Frestart").await;
    assert_eq!(
        v["records"].as_array().unwrap().len(),
        3,
        "an escape is decoded"
    );
    assert_eq!(get("?tool=docker%zz").await.0, 400);
    assert_eq!(get("?tool=%ff%fe").await.0, 400, "not text");
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
    assert_eq!(v["limits"]["max_notes"], crate::notes::MAX_NOTES);
    // REQ-MEM-004: search, best first, with the place voice commands use
    let (s, v) = call(&p, "GET", "/api/notes?q=spare+KEY", &session, "").await;
    assert_eq!(s, 200);
    assert_eq!(v["query"], "spare KEY");
    assert_eq!(v["notes"].as_array().unwrap().len(), 1);
    assert_eq!(v["notes"][0]["position"], 2);
    assert_eq!(v["notes"][0]["id"], second.id);
    let (s, v) = call(&p, "GET", "/api/notes?q=zzzz", &session, "").await;
    assert_eq!((s, v["notes"].as_array().unwrap().len()), (200, 0));
    assert_eq!(
        call(&p, "GET", "/api/notes?q=key", "", "").await.0,
        401,
        "search needs the administrator"
    );
    // change one in place: the list comes back, a duplicate or a missing one is refused
    let edit_path = format!("/api/notes/{}", second.id);
    let (s, v) = call(
        &p,
        "PUT",
        &edit_path,
        &session,
        r#"{"text":"the spare key is in the shed"}"#,
    )
    .await;
    assert_eq!(s, 200);
    assert_eq!(v["notes"][1]["text"], "the spare key is in the shed");
    assert_eq!(
        call(
            &p,
            "PUT",
            &edit_path,
            &session,
            r#"{"text":"my printer is called pixel"}"#
        )
        .await
        .0,
        409
    );
    assert_eq!(
        call(&p, "PUT", "/api/notes/999", &session, r#"{"text":"x"}"#)
            .await
            .0,
        404
    );
    assert_eq!(
        call(&p, "PUT", &edit_path, "", r#"{"text":"x"}"#).await.0,
        401
    );
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
    assert_eq!(call(&p, "PATCH", "/api/notes", &session, "{}").await.0, 405);
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
        (add("my printer is pixel"), 409, "already remembered"),
        (add("   "), 400, "Write what"),
        (
            add(&"a".repeat(crate::notes::MAX_NOTE_CHARS + 1)),
            400,
            "too long",
        ),
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

    // A reminder the user set: its label is their own words, so the file says only that one is scheduled.
    p.core.set_ui_status(json!({
        "scheduled": [{ "kind": "reminder", "label": "call the clinic about results", "seconds_left": 90 }],
    }));

    let (s, d) = call(&p, "GET", "/api/diagnostics", &session, "").await;
    assert_eq!(s, 200, "{d}");
    assert_eq!(d["format"], "clawdot-diagnostics");
    assert_eq!(d["status"]["scheduled"][0]["kind"], "reminder");
    assert!(d["status"]["scheduled"][0].get("label").is_none(), "{d}");
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
        "call the clinic",
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
    // The release version is a separate field from the commit, so the updater's comparison is untouched.
    assert_eq!(a["version"], env!("CARGO_PKG_VERSION"));
    assert!(a["build"]["commit"].as_str().is_some_and(|c| !c.is_empty()));
    assert!(a["build"]["dirty"].is_boolean());
    assert!(
        a["build"]["id"]
            .as_str()
            .is_some_and(|i| i.starts_with(env!("CARGO_PKG_VERSION")))
    );
}

// ----- forgotten password: local reset window (REQ-CFG-069); there is no recovery code -----

// REQ-INB-041
#[tokio::test]
async fn only_the_administrator_sets_the_spoken_name_and_it_is_validated() {
    let p = start(false).await;
    let setup = r#"{"mode":"password","password":"correct horse"}"#;
    let (_, v) = call(&p, "POST", "/api/setup", "", setup).await;
    let session = format!("Bearer {}", v["token"].as_str().unwrap());

    // Created with one; a bad one is refused and nothing is created.
    let bad = r#"{"name":"kitchen","adapter":"generic","spoken_name":"<b>Mum</b>"}"#;
    assert_eq!(
        call(&p, "POST", "/api/endpoints", &session, bad).await.0,
        400
    );
    assert!(p.core.endpoints().is_empty());
    let good = r#"{"name":"kitchen","adapter":"generic","spoken_name":"the kitchen"}"#;
    let (s, e) = call(&p, "POST", "/api/endpoints", &session, good).await;
    assert_eq!((s, e["spoken_name"].as_str()), (201, Some("the kitchen")));
    let token = e["token"].as_str().unwrap().to_owned();

    // The listing carries it, and a blank clears it.
    let (_, o) = call(&p, "GET", "/api/overview", &session, "").await;
    assert_eq!(o["endpoints"][0]["spoken_name"], "the kitchen");
    let (s, e) = call(
        &p,
        "PUT",
        "/api/endpoints/kitchen",
        &session,
        r#"{"spoken_name":"Hall"}"#,
    )
    .await;
    assert_eq!((s, e["spoken_name"].as_str()), (200, Some("Hall")));
    for (body, status) in [
        (r#"{"spoken_name":"[x](http://e.test)"}"#, 400),
        (r#"{"spoken_name":"line\nbreak"}"#, 400),
        (r#"{"spoken_name":"ClawDot","x":1}"#, 400),
        (r#"{}"#, 400),
    ] {
        let s = call(&p, "PUT", "/api/endpoints/kitchen", &session, body)
            .await
            .0;
        assert_eq!(s, status, "{body}");
    }
    assert_eq!(
        call(
            &p,
            "PUT",
            "/api/endpoints/nope",
            &session,
            r#"{"spoken_name":"x"}"#
        )
        .await
        .0,
        404
    );
    let (s, e) = call(
        &p,
        "PUT",
        "/api/endpoints/kitchen",
        &session,
        r#"{"spoken_name":null}"#,
    )
    .await;
    assert_eq!((s, e["spoken_name"].is_null()), (200, true));

    // Neither anonymous callers nor the endpoint's own token can change it.
    let anon = call(
        &p,
        "PUT",
        "/api/endpoints/kitchen",
        "",
        r#"{"spoken_name":"x"}"#,
    )
    .await
    .0;
    assert!(anon == 401 || anon == 429, "{anon}");
    let by_token = call(
        &p,
        "PUT",
        "/api/endpoints/kitchen",
        &format!("Bearer {token}"),
        r#"{"spoken_name":"ClawDot"}"#,
    )
    .await
    .0;
    assert!(by_token == 401 || by_token == 429, "{by_token}");
    assert!(p.core.endpoints()[0].spoken_name.is_none());
}

// REQ-EXT-019
#[tokio::test]
async fn accepting_a_connections_tools_needs_the_administrator_and_a_real_connection() {
    let p = start(true).await;
    let setup = r#"{"mode":"password","password":"correct horse"}"#;
    let (_, v) = call(&p, "POST", "/api/setup", "", setup).await;
    let session = format!("Bearer {}", v["token"].as_str().unwrap());
    let path = "/api/connections/ghost/accept";
    let anon = call(&p, "POST", path, "", "").await.0;
    assert!(anon == 401 || anon == 429, "{anon}");
    // No such connection, with an empty body or a named tool.
    assert_eq!(call(&p, "POST", path, &session, "").await.0, 404);
    assert_eq!(
        call(&p, "POST", path, &session, r#"{"tool":"ghost__x"}"#)
            .await
            .0,
        404
    );
    // A body that is not {} or {"tool": text}.
    assert_eq!(
        call(&p, "POST", path, &session, r#"{"all":true}"#).await.0,
        400
    );
    assert_eq!(call(&p, "POST", path, &session, "not json").await.0, 400);
    // Only POST.
    assert_eq!(call(&p, "GET", path, &session, "").await.0, 405);
}

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
        r#"{"password":"changed password","current":"correct horse"}"#,
    )
    .await;
    assert_eq!(s, 200);
    assert!(v.get("recovery_code").is_none());
}

// REQ-CFG-024: a session alone cannot change the password; the current one must be given and be right, and a
// wrong guess costs an attempt from the caller's budget like a sign-in does.
#[tokio::test]
async fn changing_the_password_needs_the_current_one() {
    let p = start(false).await;
    let session = set_up_with_password(&p).await;
    for body in [
        r#"{"password":"changed password"}"#,
        r#"{"password":"changed password","current":""}"#,
        r#"{"password":"changed password","current":"wrong horse"}"#,
    ] {
        let (s, v) = call(&p, "POST", "/api/admin/password", &session, body).await;
        assert_eq!(s, 403, "{body}");
        assert_eq!(v["error"], "the current password is missing or wrong");
    }
    // Nothing changed: the same session works and the old password signs in.
    assert_eq!(call(&p, "GET", "/api/about", &session, "").await.0, 200);
    let (s, _) = call(&p, "POST", "/api/login", "", &login_body("correct horse")).await;
    assert_eq!(s, 200);
    // The right one changes it, and the new one signs in.
    let ok = r#"{"password":"changed password","current":"correct horse"}"#;
    assert_eq!(
        call(&p, "POST", "/api/admin/password", &session, ok)
            .await
            .0,
        200
    );
    let (s, _) = call(
        &p,
        "POST",
        "/api/login",
        "",
        &login_body("changed password"),
    )
    .await;
    assert_eq!(s, 200);
}

// A wrong current password is a guess like any other: it is throttled per address, and a good change gives its
// attempt back.
#[tokio::test]
async fn guessing_the_current_password_is_throttled() {
    let p = start(false).await;
    let session = set_up_with_password(&p).await;
    let wrong = r#"{"password":"changed password","current":"wrong horse"}"#;
    let mut limited = false;
    for _ in 0..40 {
        let (s, _) = call(&p, "POST", "/api/admin/password", &session, wrong).await;
        if s == 429 {
            limited = true;
            break;
        }
        assert_eq!(s, 403);
    }
    assert!(limited, "guessing the current password was never throttled");
}

// In open mode there is no password to give; setting one stays possible from a private address, and is recorded.
#[tokio::test]
async fn setting_a_password_in_open_mode_is_recorded() {
    use crate::audit::{AuditConfig, AuditLog, Query};
    let p = start(false).await;
    let log = Arc::new(AuditLog::in_memory(AuditConfig::default()));
    p.core.set_audit(Arc::clone(&log));
    let (s, _) = call(&p, "POST", "/api/setup", "", r#"{"mode":"open"}"#).await;
    assert_eq!(s, 200);
    // A `current` in open mode is ignored.
    let body = r#"{"password":"first password","current":"whatever"}"#;
    let (s, _) = call(&p, "POST", "/api/admin/password", "", body).await;
    assert_eq!(s, 200);
    let page = log.query(&Query::default());
    let rec = page
        .records
        .iter()
        .find(|r| r.tool == "admin_password")
        .expect("the change is in the audit log");
    assert_eq!(rec.note.as_deref(), Some("password set in open mode"));
    assert!(
        !serde_json::to_string(&page.records)
            .unwrap()
            .contains("first password")
    );
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

// REQ-CFG-081
#[tokio::test]
async fn model_and_voice_lists_need_the_administrator_and_explain_what_is_missing() {
    let p = start(true).await;
    let setup = r#"{"mode":"password","password":"correct horse"}"#;
    let (_, v) = call(&p, "POST", "/api/setup", "", setup).await;
    let session = format!("Bearer {}", v["token"].as_str().unwrap());
    assert_eq!(call(&p, "GET", "/api/models", "", "").await.0, 401);
    assert_eq!(call(&p, "GET", "/api/voices", "", "").await.0, 401);
    // No service chosen yet: say what to do, not a connection error.
    let (s, v) = call(&p, "GET", "/api/models", &session, "").await;
    assert_eq!(s, 409, "{v}");
    assert!(
        v["error"].as_str().unwrap().contains("Choose who answers"),
        "{v}"
    );
    // No speech provider in this test portal.
    let (s, v) = call(&p, "GET", "/api/voices", &session, "").await;
    assert_eq!(s, 503, "{v}");
    assert!(
        v["error"].as_str().unwrap().contains("speech provider"),
        "{v}"
    );
}

// REQ-HIV-002. The hive state is process-wide, so this is the only test that starts it.
#[tokio::test]
async fn hive_peers_is_administrator_only_and_holds_no_secret() {
    let p = start(true).await;
    let setup = r#"{"mode":"password","password":"correct horse"}"#;
    let (_, v) = call(&p, "POST", "/api/setup", "", setup).await;
    let session = format!("Bearer {}", v["token"].as_str().unwrap());
    assert_eq!(call(&p, "GET", "/api/hive/peers", "", "").await.0, 401);
    // Not started (or not built): a plain 404, not a crash.
    assert_eq!(
        call(&p, "GET", "/api/hive/peers", &session, "").await.0,
        404
    );
    #[cfg(feature = "hive")]
    {
        let dir = crate::store::testutil::TempDir::new("hive-api");
        let opts = crate::hive::Options {
            mode: crate::hive::Mode::Off,
            port: 3335,
            addr: None,
            publish_name: false,
            name: String::new(),
        };
        let started = crate::hive::start(&dir.0, &opts);
        assert_eq!(started.problem, None);
        p.core.set_hive(started.hive.unwrap());
        let (s, v) = call(&p, "GET", "/api/hive/peers", &session, "").await;
        assert_eq!(s, 200, "{v}");
        assert_eq!(v["self"]["name"], "Test Dot");
        assert_eq!(v["self"]["id"].as_str().unwrap().len(), 64);
        assert_eq!(v["self"]["port"], 3335);
        assert_eq!(v["discovery"], "off");
        assert_eq!(v["peers"], json!([]));
        let text = v.to_string();
        assert!(
            !text.contains("pkcs8") && !text.contains("private"),
            "{text}"
        );
        assert_ne!(v["self"]["id"], v["self"]["public_key"]);
    }
}

// REQ-CFG-081
#[test]
fn a_query_value_is_decoded_and_a_missing_or_broken_one_is_none() {
    use super::query_param;
    assert_eq!(
        query_param(
            Some("url=https%3A%2F%2Fopenrouter.ai%2Fapi%2Fv1&key_env=KEY_1"),
            "url"
        )
        .as_deref(),
        Some("https://openrouter.ai/api/v1")
    );
    assert_eq!(
        query_param(Some("url=a&key_env=KEY_1"), "key_env").as_deref(),
        Some("KEY_1")
    );
    assert_eq!(
        query_param(Some("a+b=1&x=a+b"), "x").as_deref(),
        Some("a b")
    );
    assert_eq!(query_param(Some("x=1"), "url"), None);
    assert_eq!(query_param(None, "url"), None);
    assert_eq!(
        query_param(Some("x=%zz"), "x"),
        None,
        "a broken escape is refused, not guessed"
    );
    assert_eq!(
        query_param(Some("x=%4"), "x").as_deref(),
        Some("%4"),
        "a lone percent sign stays as typed"
    );
}

// REQ-CFG-082
#[tokio::test]
async fn a_pasted_key_is_kept_masked_and_never_returned_whole() {
    let dir = std::env::temp_dir().join(format!("clawdot-key-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let p = start_in(Some(&dir), true).await;
    let setup = r#"{"mode":"password","password":"correct horse"}"#;
    let (_, v) = call(&p, "POST", "/api/setup", "", setup).await;
    let session = format!("Bearer {}", v["token"].as_str().unwrap());
    // Not signed in: refused.
    let put = r#"{"key":"sk-test-0123456789abcdef0123"}"#;
    assert_eq!(call(&p, "PUT", "/api/llm/key", "", put).await.0, 401);
    // Signed in: kept, and only the ends come back.
    let (s, v) = call(&p, "PUT", "/api/llm/key", &session, put).await;
    assert_eq!(s, 200);
    assert_eq!(v["hint"], "sk-tes…0123");
    assert!(!v.to_string().contains("0123456789abcdef"));
    // The page can ask whether a key is in use, still only as a hint.
    let (_, v) = call(&p, "GET", "/api/settings", &session, "").await;
    assert_eq!(v["llm"]["key_hint"], "sk-tes…0123");
    assert!(!v.to_string().contains("0123456789abcdef"));
    // A bad key is refused with a reason; an empty one too.
    assert_eq!(
        call(
            &p,
            "PUT",
            "/api/llm/key",
            &session,
            r#"{"key":"two words"}"#
        )
        .await
        .0,
        400
    );
    assert_eq!(
        call(&p, "PUT", "/api/llm/key", &session, r#"{"key":""}"#)
            .await
            .0,
        400
    );
    // Asking for models needs a provider.
    assert_eq!(
        call(&p, "POST", "/api/models", &session, r#"{"url":""}"#)
            .await
            .0,
        409
    );
    assert_eq!(
        call(
            &p,
            "POST",
            "/api/models",
            "",
            r#"{"url":"https://x.example/v1"}"#
        )
        .await
        .0,
        401
    );
    // Forgotten again.
    let (s, v) = call(&p, "DELETE", "/api/llm/key", &session, "").await;
    assert_eq!((s, v["hint"].is_null()), (200, true));
    // REQ-VOI-023: the speech key is kept apart from the language-model key, with the same rules.
    let put = r#"{"key":"xi-speech-0123456789abcdef01"}"#;
    assert_eq!(call(&p, "PUT", "/api/speech/key", "", put).await.0, 401);
    assert_eq!(call(&p, "DELETE", "/api/speech/key", "", "").await.0, 401);
    let (s, v) = call(&p, "PUT", "/api/speech/key", &session, put).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["hint"], "xi-spe…ef01");
    assert!(!v.to_string().contains("0123456789abcdef"));
    let (_, v) = call(&p, "GET", "/api/settings", &session, "").await;
    assert_eq!(v["speech"]["key_hint"], "xi-spe…ef01");
    assert_eq!(v["speech"]["key_present"], true);
    assert!(v["llm"]["key_hint"].is_null(), "the two keys are separate");
    assert!(!v.to_string().contains("0123456789abcdef"));
    assert_eq!(p.core.secrets().get(crate::secrets::LLM_KEY), None);
    for bad in [r#"{"key":"two words"}"#, r#"{"key":""}"#] {
        assert_eq!(
            call(&p, "PUT", "/api/speech/key", &session, bad).await.0,
            400
        );
    }
    let (s, v) = call(&p, "DELETE", "/api/speech/key", &session, "").await;
    assert_eq!((s, v["hint"].is_null()), (200, true));
    let (_, v) = call(&p, "GET", "/api/settings", &session, "").await;
    assert_eq!(v["speech"]["key_present"], false);
    let _ = std::fs::remove_dir_all(&dir);
}

// The table is the one list of routes (architecture review C4).
#[test]
fn the_route_table_is_well_formed_and_no_row_hides_another() {
    crate::routes::assert_well_formed(ROUTES);
    for route in ROUTES {
        assert!(route.path.starts_with("/api/"), "{}", route.path);
        assert!(
            matches!(route.auth, Auth::Open | Auth::SignedIn),
            "{}: the portal's table has only the sign-in classes",
            route.path
        );
    }
}

#[test]
fn only_the_sign_in_routes_are_open() {
    let open: Vec<_> = ROUTES
        .iter()
        .filter(|r| r.auth == Auth::Open)
        .map(|r| format!("{} {}", r.method, r.path))
        .collect();
    assert_eq!(
        open,
        [
            "GET /api/auth",
            "POST /api/setup",
            "POST /api/login",
            "POST /api/recover/window",
            "POST /api/logout"
        ]
    );
}

// REQ-CFG-005: a path that exists answers 405 for a wrong method, an unknown path 404, and the open routes 404.
#[tokio::test]
async fn unknown_paths_are_404_and_wrong_methods_405_after_the_sign_in() {
    let p = start(true).await;
    let setup = r#"{"mode":"password","password":"correct horse"}"#;
    let (_, v) = call(&p, "POST", "/api/setup", "", setup).await;
    let session = format!("Bearer {}", v["token"].as_str().unwrap());
    assert_eq!(call(&p, "GET", "/api/nope", "", "").await.0, 404);
    assert_eq!(call(&p, "GET", "/api/nope", &session, "").await.0, 404);
    assert_eq!(call(&p, "GET", "/api/login", &session, "").await.0, 404);
    assert_eq!(call(&p, "POST", "/api/auth", &session, "{}").await.0, 404);
    assert_eq!(call(&p, "PATCH", "/api/overview", "", "").await.0, 401);
    assert_eq!(
        call(&p, "PATCH", "/api/overview", &session, "").await.0,
        405
    );
    assert_eq!(
        call(&p, "GET", "/api/connections", &session, "").await.0,
        405
    );
    assert_eq!(
        call(&p, "GET", "/api/connections/x", &session, "").await.0,
        405
    );
    assert_eq!(
        call(&p, "POST", "/api/overview/extra", &session, "{}")
            .await
            .0,
        404
    );
}

/// A server that answers one request with `status` and a JSON `body`.
async fn answer_once(status: &'static str, body: &'static str) -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (mut c, _) = l.accept().await.unwrap();
        let mut buf = [0u8; 4096];
        let _ = c.read(&mut buf).await;
        let resp = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = c.write_all(resp.as_bytes()).await;
    });
    port
}

// REQ-CFG-081: the status follows the KIND of failure, not digits that happen to appear in its text.
#[tokio::test]
async fn model_list_status_follows_the_kind_of_failure() {
    for (status, body, want) in [
        (
            "401 Unauthorized",
            r#"{"error":{"message":"bad key"}}"#,
            401,
        ),
        ("403 Forbidden", r#"{"error":{"message":"no"}}"#, 401),
        (
            "500 Internal Server Error",
            r#"{"error":{"message":"request 401 failed"}}"#,
            502,
        ),
        ("429 Too Many Requests", "{}", 502),
    ] {
        let port = answer_once(status, body).await;
        let r = fetch_models(&format!("http://127.0.0.1:{port}/v1"), "k").await;
        assert_eq!(r.status().as_u16(), want, "{status}");
    }
}
