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
            if token.is_none() && line.trim_start().starts_with("cdt_") {
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

/// Serve the given canned HTTP responses, one per connection, and return each request body received.
fn fake_llm(responses: Vec<String>) -> (u16, std::thread::JoinHandle<Vec<String>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = std::thread::spawn(move || {
        let mut bodies = Vec::new();
        for response in responses {
            let (mut conn, _) = listener.accept().unwrap();
            conn.set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let mut raw = Vec::new();
            let mut buf = [0u8; 4096];
            let body = loop {
                let n = conn.read(&mut buf).unwrap();
                assert!(n > 0, "the client closed before sending a full request");
                raw.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&raw).into_owned();
                if let Some((head, body)) = text.split_once("\r\n\r\n") {
                    let want: usize = head
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse().unwrap())
                        })
                        .unwrap_or(0);
                    if body.len() >= want {
                        break body.to_owned();
                    }
                }
            };
            bodies.push(body);
            conn.write_all(response.as_bytes()).unwrap();
        }
        bodies
    });
    (port, handle)
}

fn sse(events: &[&str]) -> String {
    let mut s = String::from(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
    );
    for e in events {
        s.push_str(&format!("data: {e}\n\n"));
    }
    s.push_str("data: [DONE]\n\n");
    s
}

#[test]
fn a_question_the_fast_path_cannot_answer_goes_through_the_model_and_its_tools() {
    let tool_call = sse(&[
        r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"inbox_status","arguments":"{}"}}]}}]}"#,
        r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
    ]);
    let answer = sse(&[
        r#"{"choices":[{"delta":{"content":"Nothing is waiting. "}}]}"#,
        r#"{"choices":[{"delta":{"content":"Your agents are quiet."},"finish_reason":"stop"}]}"#,
    ]);
    let (llm_port, llm) = fake_llm(vec![tool_call, answer]);
    let dir = std::env::temp_dir().join(format!("clawdot-llm-test-{}", std::process::id()));
    let mut child = Command::new(env!("CARGO_BIN_EXE_clawdot"))
        .args([
            "--listen",
            &format!("127.0.0.1:{}", free_port()),
            "--data-dir",
            dir.to_str().unwrap(),
            "--llm-url",
            &format!("http://127.0.0.1:{llm_port}"),
            "--llm-model",
            "test-model",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let _ = tx.send(line);
        }
    });
    // Wait for the console before typing.
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut ready = false;
    while Instant::now() < deadline && !ready {
        if let Ok(line) = rx.recv_timeout(Duration::from_millis(200)) {
            ready = line.contains("console ready");
        }
    }
    assert!(ready, "the console should start");

    stdin
        .write_all(b"tell me what you think about owls\n")
        .unwrap();
    let mut spoken = None;
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline && spoken.is_none() {
        if let Ok(line) = rx.recv_timeout(Duration::from_millis(250)) {
            if line.starts_with("speaker> ") {
                spoken = Some(line);
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&dir);

    let spoken = spoken.expect("the assistant should answer");
    assert!(spoken.contains("Your agents are quiet."), "got: {spoken}");
    let bodies = llm.join().unwrap();
    assert!(
        bodies[0].contains("inbox_status"),
        "tools are offered to the model"
    );
    assert!(
        bodies[1].contains("call_1"),
        "the tool result goes back to the model"
    );
}

#[test]
fn a_bad_config_file_stops_startup_and_names_the_problem() {
    let path = std::env::temp_dir().join(format!("clawdot-bad-config-{}.json", std::process::id()));
    std::fs::write(&path, r#"{"listn": "127.0.0.1:1"}"#).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_clawdot"))
        .args(["--no-console", "--config", path.to_str().unwrap()])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let _ = std::fs::remove_file(&path);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("listn"), "got: {err}");
}

#[test]
fn a_configured_http_tool_runs_only_after_the_user_confirms() {
    // The device the tool controls.
    let device = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let device_port = device.local_addr().unwrap().port();
    let device_thread = std::thread::spawn(move || {
        let (mut c, _) = device.accept().unwrap();
        c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut raw = Vec::new();
        let mut buf = [0u8; 2048];
        while !String::from_utf8_lossy(&raw).contains("}") {
            let n = c.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            raw.extend_from_slice(&buf[..n]);
        }
        c.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK")
            .unwrap();
        String::from_utf8_lossy(&raw).into_owned()
    });

    let call = |id: &str, args: &str| {
        sse(&[
            &format!(
                r#"{{"choices":[{{"delta":{{"tool_calls":[{{"index":0,"id":"{id}","function":{{"name":"set_light","arguments":{}}}}}]}}}}]}}"#,
                serde_json::to_string(args).unwrap()
            ),
            r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
        ])
    };
    let (llm_port, llm) = fake_llm(vec![
        call("c1", r#"{"room":"den"}"#),
        call("c2", r#"{"room":"den","confirmed":true}"#),
        sse(&[
            r#"{"choices":[{"delta":{"content":"The den light is on."},"finish_reason":"stop"}]}"#,
        ]),
    ]);

    let dir = std::env::temp_dir().join(format!("clawdot-tool-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = dir.join("config.json");
    std::fs::write(
        &cfg,
        format!(
            r#"{{"llm": {{"url": "http://127.0.0.1:{llm_port}", "model": "m"}},
                "tools": [{{"name": "set_light", "description": "Turn a light on", "method": "POST",
                            "url": "http://127.0.0.1:{device_port}/light",
                            "params": {{"room": {{"type": "string", "enum": ["den", "kitchen"], "required": true}}}}}}]}}"#
        ),
    )
    .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_clawdot"))
        .args([
            "--config",
            cfg.to_str().unwrap(),
            "--listen",
            &format!("127.0.0.1:{}", free_port()),
            "--data-dir",
            dir.join("state").to_str().unwrap(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let _ = tx.send(line);
        }
    });
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut ready = false;
    while Instant::now() < deadline && !ready {
        if let Ok(line) = rx.recv_timeout(Duration::from_millis(200)) {
            ready = line.contains("console ready");
        }
    }
    assert!(ready);
    stdin.write_all(b"make the den bright please\n").unwrap();
    let mut spoken = None;
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline && spoken.is_none() {
        if let Ok(line) = rx.recv_timeout(Duration::from_millis(250)) {
            if line.starts_with("speaker> ") {
                spoken = Some(line);
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    let spoken = spoken.expect("the assistant should answer");
    assert!(spoken.contains("The den light is on."), "got: {spoken}");
    let bodies = llm.join().unwrap();
    assert!(
        bodies[1].contains("needs the user's confirmation"),
        "the first call was refused"
    );
    let request = device_thread.join().unwrap();
    assert!(request.starts_with("POST /light "), "{request}");
    assert!(request.ends_with(r#"{"room":"den"}"#), "{request}");
    let _ = std::fs::remove_dir_all(&dir);
}
