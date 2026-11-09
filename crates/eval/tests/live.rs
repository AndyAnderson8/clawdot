//! Live mode (REQ-AST-017), exercised against a fake provider on this machine. No real model is called and
//! nothing is spent: the "provider" is a local socket that answers every request with the same text and a
//! reported cost, which is enough to show the three runs, the pass rule, the cap and the CLI guard.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use clawdot_core::transport::HttpBackend;
use clawdot_eval::live::{LiveOptions, render, run_live};
use clawdot_eval::scenario::load_dir;
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn sse(text: &str, cost: f64) -> String {
    let chunk = json!({"choices": [{"delta": {"content": text}}]});
    let end = json!({"choices": [{"delta": {}, "finish_reason": "stop"}],
                     "usage": {"prompt_tokens": 4, "completion_tokens": 2, "cost": cost}});
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: {chunk}\n\ndata: {end}\n\ndata: [DONE]\n\n"
    )
}

/// A provider that answers every request with `reply`; returns its address and a request counter.
async fn provider(reply: String) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let count = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&count);
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = listener.accept().await else {
                return;
            };
            let reply = reply.clone();
            let seen = Arc::clone(&seen);
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0_u8; 4096];
                loop {
                    let n = s.read(&mut chunk).await.unwrap_or(0);
                    buf.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&buf).into_owned();
                    if let Some((head, body)) = text.split_once("\r\n\r\n") {
                        let want = head
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .and_then(|v| v.trim().parse::<usize>().ok())
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
                seen.fetch_add(1, Ordering::Relaxed);
                let _ = s.write_all(reply.as_bytes()).await;
                let _ = s.shutdown().await;
            });
        }
    });
    (url, count)
}

fn scenarios(n: usize, say_contains: &str) -> (PathBuf, Vec<clawdot_eval::scenario::Loaded>) {
    static UNIQUE: AtomicUsize = AtomicUsize::new(0);
    let u = UNIQUE.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("clawdot-eval-live-{}-{u}", std::process::id()));
    std::fs::create_dir_all(dir.join("scenarios")).unwrap();
    for i in 0..n {
        let id = format!("greeting-{i}");
        let s = json!({
            "version": 1, "id": id, "use_case": if i == 0 { "INJ" } else { "UC9" },
            "turns": [{"say": "hello", "expect": {"speech_contains": [say_contains]}}]
        });
        std::fs::write(
            dir.join("scenarios").join(format!("{id}.json")),
            s.to_string(),
        )
        .unwrap();
    }
    let loaded = load_dir(&dir.join("scenarios")).unwrap();
    (dir, loaded)
}

fn backend(url: &str) -> HttpBackend {
    HttpBackend::new(url, "", Duration::from_secs(10)).unwrap()
}

// REQ-AST-017
#[tokio::test(flavor = "current_thread")]
async fn each_scenario_runs_three_times_and_pass_rates_are_reported() {
    let (url, count) = provider(sse("Hello there.", 0.0)).await;
    let (dir, set) = scenarios(2, "hello");
    let make = || backend(&url);
    let (summary, recordings) = run_live(&set, LiveOptions::default(), &make, |_, _, _| {}).await;
    assert_eq!(count.load(Ordering::Relaxed), 6, "2 scenarios x 3 runs");
    assert!(!summary.partial);
    assert_eq!(summary.results.len(), 2);
    for r in &summary.results {
        assert_eq!((r.runs.len(), r.passes()), (3, 3));
        assert!(r.scenario_passed(3));
    }
    assert_eq!(
        recordings.len(),
        2,
        "the first run of each is kept for a transcript"
    );
    assert_eq!(recordings[0].1[0][0].text.as_deref(), Some("Hello there."));
    let text = render(&summary, "fake-model", "abc123");
    assert!(
        text.contains("100%") && text.contains("required 100%: met"),
        "{text}"
    );
    assert!(text.contains("fake-model") && text.contains("set abc123"));
    std::fs::remove_dir_all(dir).unwrap();
}

// REQ-AST-017: two of three runs passing is enough; fewer is not; a safety break always fails.
#[tokio::test(flavor = "current_thread")]
async fn the_pass_rule_is_two_of_three_with_no_safety_failure() {
    use clawdot_eval::live::ScenarioResult;
    use clawdot_eval::run::{Check, RunReport};
    let run = |ok: bool, safety: bool| RunReport {
        checks: vec![Check {
            turn: Some(1),
            name: "x".into(),
            ok,
            detail: String::new(),
            safety,
        }],
        ..RunReport::default()
    };
    let result = |runs: Vec<RunReport>| ScenarioResult {
        id: "s".into(),
        use_case: "UC1".into(),
        runs,
    };
    assert!(result(vec![run(true, false), run(true, false), run(false, false)]).scenario_passed(3));
    assert!(
        !result(vec![run(true, false), run(false, false), run(false, false)]).scenario_passed(3)
    );
    // two clean passes do not excuse a forbidden action in the third
    assert!(!result(vec![run(true, false), run(true, false), run(false, true)]).scenario_passed(3));
    // an incomplete scenario (stopped by the cap) does not pass
    assert!(!result(vec![run(true, false), run(true, false)]).scenario_passed(3));
}

// REQ-AST-017: the spend cap stops the run and the partial results say so.
#[tokio::test(flavor = "current_thread")]
async fn the_spend_cap_stops_the_run_and_reports_partial_results() {
    let (url, count) = provider(sse("Hello there.", 0.4)).await;
    let (dir, set) = scenarios(4, "hello");
    let make = || backend(&url);
    let options = LiveOptions {
        runs: 3,
        spend_cap_usd: 1.0,
        max_requests: 100,
    };
    let (summary, _) = run_live(&set, options, &make, |_, _, _| {}).await;
    assert!(summary.partial, "stopped early");
    assert_eq!(
        count.load(Ordering::Relaxed),
        3,
        "no request after the cap was reached"
    );
    assert!(
        (summary.spend_usd - 1.2).abs() < 1e-6,
        "{}",
        summary.spend_usd
    );
    assert!(summary.results.len() < 4);
    let text = render(&summary, "fake-model", "abc123");
    assert!(text.contains("PARTIAL"), "{text}");
    std::fs::remove_dir_all(dir).unwrap();
}

// REQ-AST-017: a provider that reports no cost is still bounded by the request limit.
#[tokio::test(flavor = "current_thread")]
async fn the_request_limit_bounds_a_provider_that_reports_no_cost() {
    let (url, count) = provider(sse("Hello there.", 0.0)).await;
    let (dir, set) = scenarios(4, "hello");
    let make = || backend(&url);
    let options = LiveOptions {
        runs: 3,
        spend_cap_usd: 1.0,
        max_requests: 5,
    };
    let (summary, _) = run_live(&set, options, &make, |_, _, _| {}).await;
    assert!(summary.partial);
    assert_eq!(count.load(Ordering::Relaxed), 5);
    std::fs::remove_dir_all(dir).unwrap();
}

// REQ-AST-017: a model that fails a check fails its runs, and the table says so.
#[tokio::test(flavor = "current_thread")]
async fn failing_runs_are_counted() {
    let (url, _) = provider(sse("Goodbye.", 0.0)).await;
    let (dir, set) = scenarios(1, "hello");
    let make = || backend(&url);
    let (summary, _) = run_live(&set, LiveOptions::default(), &make, |_, _, _| {}).await;
    assert_eq!(summary.results[0].passes(), 0);
    assert!(!summary.results[0].scenario_passed(3));
    let text = render(&summary, "m", "x");
    assert!(text.contains("FAIL") && text.contains("NOT MET"), "{text}");
    std::fs::remove_dir_all(dir).unwrap();
}

// REQ-AST-017: live mode is off unless asked for explicitly, and never calls anything without the flag.
#[test]
fn live_mode_needs_an_explicit_flag_and_never_runs_by_accident() {
    let exe = env!("CARGO_BIN_EXE_clawdot-eval");
    let run = |args: &[&str]| {
        std::process::Command::new(exe)
            .args(args)
            .env_remove("OPENROUTER_API_KEY")
            .output()
            .unwrap()
    };
    let out = run(&[
        "live",
        "--url",
        "https://example.invalid/v1",
        "--model",
        "m",
    ]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("--yes-spend"));
    // with the flag but no key for a remote endpoint it still refuses before any request
    let out = run(&[
        "live",
        "--yes-spend",
        "--url",
        "https://example.invalid/v1",
        "--model",
        "m",
    ]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("OPENROUTER_API_KEY"));
    // an absurd cap is refused
    let out = run(&[
        "live",
        "--yes-spend",
        "--spend-cap-usd",
        "5000",
        "--url",
        "http://127.0.0.1:1",
        "--model",
        "m",
    ]);
    assert_eq!(out.status.code(), Some(2));
    // the default command is replay and needs no flags, no key and no network
    let out = run(&["replay"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("Injection set"));
}
