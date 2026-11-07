//! Runs the real `clawdot` binary: it must serve webhooks and announce what arrives.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const STOP: &str = r#"{"session_id":"s1","cwd":"/home/dev/clawdot","hook_event_name":"Stop","last_assistant_message":"Done."}"#;

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn post_hook(port: u16, token: &str, body: &str) -> u16 {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let req = format!(
        "POST /hook/claude-code HTTP/1.1\r\nHost: t\r\nAuthorization: Bearer {token}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).unwrap();
    let mut raw = String::new();
    let _ = s.read_to_string(&mut raw);
    raw.split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0)
}

#[test]
fn the_binary_serves_hooks_and_announces_them() {
    let port = free_port();
    let dir = std::env::temp_dir().join(format!("clawdot-bin-test-{}", std::process::id()));
    let mut child = Command::new(env!("CARGO_BIN_EXE_clawdot"))
        .args([
            "--no-console",
            "--listen",
            &format!("127.0.0.1:{port}"),
            "--data-dir",
            dir.to_str().unwrap(),
            "--create-endpoint",
            "claude-code",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let _ = tx.send(line);
        }
    });

    let mut token = None;
    let mut listening = false;
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline && !(token.is_some() && listening) {
        if let Ok(line) = rx.recv_timeout(Duration::from_millis(200)) {
            if line.trim_start().starts_with("cdt_") {
                token = Some(line.trim().to_owned());
            }
            if line.contains("listening on") {
                listening = true;
            }
        }
    }
    let token = token.expect("the binary should print the endpoint token once");
    assert!(listening, "the binary should report that it is listening");

    assert_eq!(post_hook(port, &token, STOP), 202);

    let mut spoken = None;
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline && spoken.is_none() {
        if let Ok(line) = rx.recv_timeout(Duration::from_millis(250)) {
            if line.starts_with("speaker> ") && line.contains("clawdot finished") {
                spoken = Some(line);
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&dir);
    let spoken = spoken.expect("the event should be announced within a few seconds");
    // Without an LLM the announcement is the headline only, never the agent's raw text.
    assert!(!spoken.contains("Done."), "{spoken}");
}
