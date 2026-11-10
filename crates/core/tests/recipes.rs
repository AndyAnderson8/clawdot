//! The shipped recipe configurations must keep working (REQ-EXT-008, REQ-EXT-009): each file in
//! `examples/recipes/` is a valid configuration, every tool names its tier, and no secret is written in it.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use clawdot_core::config::Config;
use clawdot_core::httptools::HttpTools;
use clawdot_core::session::Tier;
use std::path::PathBuf;

fn recipe_files() -> Vec<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/recipes");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    files
}

#[test]
fn every_recipe_is_a_valid_configuration() {
    let files = recipe_files();
    assert!(files.len() >= 8, "the recipes went missing: {files:?}");
    for f in files {
        let text = std::fs::read_to_string(&f).unwrap();
        let cfg = Config::parse(&text).unwrap_or_else(|e| panic!("{}: {e}", f.display()));
        assert!(
            !cfg.tools.is_empty() || !cfg.mcp.is_empty(),
            "{}: no tools or connections",
            f.display()
        );
        // The core's own start-up checks, with every credential variable "set".
        HttpTools::new(cfg.tools.clone(), &[], |_| Some("x".into()))
            .unwrap_or_else(|e| panic!("{}: {e}", f.display()));
        for s in &cfg.mcp {
            s.validate()
                .unwrap_or_else(|e| panic!("{}: {e}", f.display()));
        }
    }
}

#[test]
fn every_tool_states_its_tier_and_names_its_credential_by_variable() {
    for f in recipe_files() {
        let text = std::fs::read_to_string(&f).unwrap();
        let cfg = Config::parse(&text).unwrap();
        for t in &cfg.tools {
            assert!(
                t.tier.is_some(),
                "{}: `{}` has no tier",
                f.display(),
                t.name
            );
            // A tool that changes something must not be marked read-only.
            if t.method != "GET" && t.tier == Some(Tier::ReadOnly) {
                assert!(
                    t.name.contains("search"),
                    "{}: `{}` is a {} marked read-only",
                    f.display(),
                    t.name,
                    t.method
                );
            }
            if let Some(var) = &t.auth_env {
                assert!(
                    var.bytes()
                        .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_'),
                    "{}: `{var}` is not an environment variable name",
                    f.display()
                );
            }
        }
        for s in &cfg.mcp {
            assert!(
                !s.tiers.is_empty() || s.confirm,
                "{}: `{}` lowers confirmation without naming tiers",
                f.display(),
                s.name
            );
        }
    }
}

#[test]
fn no_recipe_contains_a_secret() {
    for f in recipe_files() {
        let text = std::fs::read_to_string(&f).unwrap();
        for needle in [
            "Bearer ",
            "tvly-",
            "sk-",
            "ghp_",
            "github_pat_",
            "eyJ",
            "cdt_",
        ] {
            assert!(
                !text.contains(needle),
                "{}: looks like it holds a secret (`{needle}`)",
                f.display()
            );
        }
    }
}

// ----- the tool shapes against a fake server (REQ-EXT-009) -----

use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// A server that records each request (head and body) and answers `200 {"fake":true}`.
async fn fake_server() -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = listener.accept().await else {
                return;
            };
            let log = Arc::clone(&log);
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    let n = s.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&buf).into_owned();
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
                }
                log.lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&buf).into_owned());
                let body = r#"{"fake":true}"#;
                let _ = s
                    .write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        )
                        .as_bytes(),
                    )
                    .await;
            });
        }
    });
    (format!("http://{addr}"), seen)
}

/// The recipe's tools with every host replaced by the fake server.
fn tools_of(file: &str, base: &str) -> HttpTools {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/recipes")
        .join(file);
    let mut cfg = Config::parse(&std::fs::read_to_string(path).unwrap()).unwrap();
    for t in &mut cfg.tools {
        let rest = t.url.split_once("://").unwrap().1;
        let path = rest.find('/').map_or("", |i| &rest[i..]);
        t.url = format!("{base}{path}");
    }
    HttpTools::new(cfg.tools, &[], |var| Some(format!("secret-of-{var}"))).unwrap()
}

fn last(seen: &Arc<Mutex<Vec<String>>>) -> String {
    seen.lock().unwrap().last().cloned().unwrap_or_default()
}

fn body_of(req: &str) -> Value {
    serde_json::from_str(req.split_once("\r\n\r\n").map_or("", |(_, b)| b)).unwrap_or(Value::Null)
}

fn has_header(req: &str, name: &str, value: &str) -> bool {
    req.lines()
        .any(|l| l.eq_ignore_ascii_case(&format!("{name}: {value}")))
}

#[tokio::test(flavor = "current_thread")]
async fn the_search_recipes_build_the_requests_their_services_expect() {
    let (base, seen) = fake_server().await;
    let searx = tools_of("web-search-searxng.json", &base);
    let out = searx
        .call("web_search", &json!({"q": "rust & more"}))
        .await
        .unwrap();
    assert!(out.contains("fake"));
    let req = last(&seen);
    assert!(
        req.starts_with("GET /search?q=rust%20%26%20more&format=json&language=en&safesearch=1 "),
        "{req}"
    );
    assert!(
        !req.to_lowercase().contains("authorization"),
        "SearXNG needs no key: {req}"
    );

    let tavily = tools_of("web-search-tavily.json", &base);
    tavily
        .call(
            "web_search",
            &json!({"query": "weather in Oslo", "include_answer": "basic", "max_results": 3}),
        )
        .await
        .unwrap();
    let req = last(&seen);
    assert!(req.starts_with("POST /search "), "{req}");
    assert!(
        has_header(&req, "authorization", "Bearer secret-of-TAVILY_API_KEY"),
        "{req}"
    );
    assert_eq!(
        body_of(&req),
        json!({"query": "weather in Oslo", "include_answer": "basic", "max_results": 3})
    );
    // The model cannot ask for anything the recipe does not declare.
    assert!(
        tavily
            .call(
                "web_search",
                &json!({"query": "x", "include_answer": "basic", "max_results": 3, "include_raw_content": true})
            )
            .await
            .is_err()
    );
    assert!(
        tavily
            .call(
                "web_search",
                &json!({"query": "x", "include_answer": "advanced", "max_results": 3})
            )
            .await
            .is_err()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn the_spotify_recipe_sends_what_the_helper_expects_and_only_to_listed_devices() {
    let (base, seen) = fake_server().await;
    let t = tools_of("spotify-web-api.json", &base);
    let kitchen = "REPLACE_WITH_KITCHEN_DEVICE_ID";
    t.call(
        "spotify_play",
        &json!({"device_id": kitchen, "uri": "spotify:album:abc"}),
    )
    .await
    .unwrap();
    let req = last(&seen);
    assert!(
        req.starts_with("PUT /v1/me/player/play?device_id=REPLACE_WITH_KITCHEN_DEVICE_ID "),
        "{req}"
    );
    assert!(
        has_header(&req, "authorization", "Bearer secret-of-SPOTIFY_PROXY_KEY"),
        "{req}"
    );
    assert_eq!(body_of(&req), json!({"uri": "spotify:album:abc"}));

    t.call(
        "spotify_volume",
        &json!({"volume_percent": 35, "device_id": "REPLACE_WITH_LIVING_ROOM_DEVICE_ID"}),
    )
    .await
    .unwrap();
    assert!(last(&seen).starts_with(
        "PUT /v1/me/player/volume?volume_percent=35&device_id=REPLACE_WITH_LIVING_ROOM_DEVICE_ID "
    ));
    t.call("spotify_next", &json!({"device_id": kitchen}))
        .await
        .unwrap();
    assert!(
        last(&seen)
            .starts_with("POST /v1/me/player/next?device_id=REPLACE_WITH_KITCHEN_DEVICE_ID ")
    );
    t.call(
        "spotify_search",
        &json!({"q": "dave brubeck", "type": "album"}),
    )
    .await
    .unwrap();
    assert!(last(&seen).starts_with("GET /v1/search?q=dave%20brubeck&type=album&limit=3 "));
    // A device the administrator did not list is refused before anything is sent.
    let before = seen.lock().unwrap().len();
    assert!(
        t.call(
            "spotify_pause",
            &json!({"device_id": "someone-elses-speaker"})
        )
        .await
        .is_err()
    );
    assert_eq!(seen.lock().unwrap().len(), before);
}

#[tokio::test(flavor = "current_thread")]
async fn the_home_assistant_repository_host_and_calendar_recipes_build_their_requests() {
    let (base, seen) = fake_server().await;
    let ha = tools_of("home-assistant-rest.json", &base);
    ha.call(
        "home_light",
        &json!({"service": "turn_on", "entity_id": "light.den"}),
    )
    .await
    .unwrap();
    let req = last(&seen);
    assert!(
        req.starts_with("POST /api/services/light/turn_on "),
        "{req}"
    );
    assert!(
        has_header(&req, "authorization", "Bearer secret-of-HA_TOKEN"),
        "{req}"
    );
    assert_eq!(body_of(&req), json!({"entity_id": "light.den"}));
    assert!(
        ha.call(
            "home_light",
            &json!({"service": "turn_on", "entity_id": "lock.front_door"})
        )
        .await
        .is_err()
    );
    assert!(
        ha.call(
            "home_light",
            &json!({"service": "../../lock/unlock", "entity_id": "light.den"})
        )
        .await
        .is_err()
    );
    ha.call("home_state", &json!({"entity_id": "sensor.hall_temp"}))
        .await
        .unwrap();
    assert!(last(&seen).starts_with("GET /api/states/sensor.hall_temp "));
    ha.call(
        "home_assist",
        &json!({"text": "is the garage open", "language": "en"}),
    )
    .await
    .unwrap();
    assert_eq!(
        body_of(&last(&seen)),
        json!({"text": "is the garage open", "language": "en"})
    );

    let forge = tools_of("github-rest.json", &base);
    forge
        .call(
            "github_list_runs",
            &json!({"owner": "me", "repo": "clawdot"}),
        )
        .await
        .unwrap();
    let req = last(&seen);
    assert!(
        req.starts_with("GET /repos/me/clawdot/actions/runs?per_page=5 "),
        "{req}"
    );
    assert!(
        has_header(&req, "x-github-api-version", "2022-11-28"),
        "{req}"
    );
    forge
        .call(
            "github_rerun_failed",
            &json!({"owner": "me", "repo": "clawdot", "run_id": 42}),
        )
        .await
        .unwrap();
    assert!(last(&seen).starts_with("POST /repos/me/clawdot/actions/runs/42/rerun-failed-jobs "));
    // A repository name cannot add path segments.
    forge
        .call(
            "github_list_prs",
            &json!({"owner": "me", "repo": "x/../../orgs"}),
        )
        .await
        .unwrap();
    assert!(
        last(&seen).contains("/repos/me/x%2F..%2F..%2Forgs/pulls"),
        "{}",
        last(&seen)
    );

    let cal = tools_of("calendar-home-assistant.json", &base);
    cal.call(
        "calendar_events",
        &json!({"start": "2026-11-10T00:00:00-05:00", "end": "2026-11-11T00:00:00-05:00"}),
    )
    .await
    .unwrap();
    assert!(
        last(&seen).starts_with("GET /api/calendars/calendar.family?start=2026-11-10T00%3A00%3A00-05%3A00&end=2026-11-11T00%3A00%3A00-05%3A00 "),
        "{}",
        last(&seen)
    );
}

#[test]
fn recipe_tiers_are_what_the_recipes_say() {
    let tier_of = |file: &str, name: &str| {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/recipes")
            .join(file);
        let cfg = Config::parse(&std::fs::read_to_string(path).unwrap()).unwrap();
        cfg.tools
            .into_iter()
            .find(|t| t.name == name)
            .unwrap()
            .tier
            .unwrap()
    };
    assert_eq!(
        tier_of("web-search-searxng.json", "web_search"),
        Tier::ReadOnly
    );
    assert_eq!(
        tier_of("spotify-web-api.json", "spotify_now_playing"),
        Tier::ReadOnly
    );
    assert_eq!(
        tier_of("spotify-web-api.json", "spotify_play"),
        Tier::Reversible
    );
    assert_eq!(
        tier_of("home-assistant-rest.json", "home_light"),
        Tier::Reversible
    );
    assert_eq!(
        tier_of("home-assistant-rest.json", "home_assist"),
        Tier::Costly
    );
    assert_eq!(
        tier_of("github-rest.json", "github_rerun_failed"),
        Tier::Costly
    );
    assert_eq!(
        tier_of("calendar-home-assistant.json", "calendar_events"),
        Tier::ReadOnly
    );
}
