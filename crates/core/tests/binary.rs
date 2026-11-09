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

/// Run `clawdot --create-endpoint claude-code` against `data_dir`: it must print the token and exit
/// by itself (REQ-CFG-015). Returns the token.
fn create_endpoint(data_dir: &std::path::Path) -> String {
    let mut child = Command::new(env!("CARGO_BIN_EXE_clawdot"))
        .args([
            "--no-console",
            "--echo-speech",
            "--data-dir",
            data_dir.to_str().unwrap(),
            "--create-endpoint",
            "claude-code",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        assert!(
            Instant::now() < deadline,
            "--create-endpoint should exit by itself"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(status.success());
    let mut out = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut out)
        .unwrap();
    out.lines()
        .map(str::trim)
        .find(|l| l.starts_with("cdt_"))
        .expect("the endpoint token is printed once")
        .to_owned()
}

#[test]
fn the_binary_serves_hooks_and_announces_them() {
    let port = free_port();
    let dir = std::env::temp_dir().join(format!("clawdot-bin-test-{}", std::process::id()));
    let token = create_endpoint(&dir);
    let mut child = Command::new(env!("CARGO_BIN_EXE_clawdot"))
        .args([
            "--no-console",
            "--echo-speech",
            "--listen",
            &format!("127.0.0.1:{port}"),
            "--data-dir",
            dir.to_str().unwrap(),
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

    let mut listening = false;
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline && !listening {
        if let Ok(line) = rx.recv_timeout(Duration::from_millis(200)) {
            if line.contains("listening on") {
                listening = true;
            }
        }
    }
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

/// The next line the console speaks, or `None` after 20 seconds.
fn next_speech(rx: &mpsc::Receiver<String>) -> Option<String> {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if let Ok(line) = rx.recv_timeout(Duration::from_millis(250)) {
            if line.starts_with("speaker> ") {
                return Some(line);
            }
        }
    }
    None
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
        .args([
            "--no-console",
            "--echo-speech",
            "--config",
            path.to_str().unwrap(),
        ])
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
        // The model even claims confirmation itself; the host ignores that and asks the user.
        call("c1", r#"{"room":"den","confirmed":true}"#),
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
    let asked = next_speech(&rx).expect("the host should ask");
    assert!(
        asked.contains("set_light") && asked.contains("Yes or no?"),
        "got: {asked}"
    );
    stdin.write_all(b"yes\n").unwrap();
    let spoken = next_speech(&rx);
    let _ = child.kill();
    let _ = child.wait();
    let spoken = spoken.expect("the assistant should answer");
    assert!(spoken.contains("The den light is on."), "got: {spoken}");
    let bodies = llm.join().unwrap();
    assert_eq!(
        bodies.len(),
        2,
        "the host asked without calling the model again"
    );
    let request = device_thread.join().unwrap();
    assert!(request.starts_with("POST /light "), "{request}");
    assert!(request.ends_with(r#"{"room":"den"}"#), "{request}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_recorded_utterance_is_transcribed_answered_and_spoken_back() {
    // A fake speech provider: transcription returns a timer request, speech returns 0.1 s of 24 kHz audio.
    let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = server.local_addr().unwrap().port();
    let provider = std::thread::spawn(move || {
        let mut paths = Vec::new();
        for _ in 0..2 {
            let (mut c, _) = server.accept().unwrap();
            c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            let mut raw = Vec::new();
            let mut buf = [0u8; 8192];
            let head_end = loop {
                let n = c.read(&mut buf).unwrap();
                assert!(n > 0);
                raw.extend_from_slice(&buf[..n]);
                if let Some(p) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&raw[..p]).to_ascii_lowercase();
                    let want: usize = head
                        .lines()
                        .find_map(|l| {
                            l.strip_prefix("content-length:")
                                .map(|v| v.trim().parse().unwrap())
                        })
                        .unwrap_or(0);
                    if raw.len() >= p + 4 + want {
                        break p;
                    }
                }
            };
            let path = String::from_utf8_lossy(&raw[..head_end])
                .split_whitespace()
                .nth(1)
                .unwrap()
                .to_owned();
            let (ctype, body): (&str, Vec<u8>) = if path.ends_with("/audio/transcriptions") {
                (
                    "application/json",
                    br#"{"text":"set a timer for ten minutes"}"#.to_vec(),
                )
            } else {
                (
                    "audio/pcm",
                    (0..2400i16).flat_map(|i| (i % 50).to_le_bytes()).collect(),
                )
            };
            let mut resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .into_bytes();
            resp.extend_from_slice(&body);
            c.write_all(&resp).unwrap();
            paths.push(path);
        }
        paths
    });

    let dir = std::env::temp_dir().join(format!("clawdot-voice-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = dir.join("config.json");
    std::fs::write(
        &cfg,
        format!(
            r#"{{"speech": {{"url": "http://127.0.0.1:{port}/v1", "stt_model": "w", "tts_model": "t", "voice": "v"}}}}"#
        ),
    )
    .unwrap();
    let input = dir.join("in.wav");
    std::fs::write(&input, clawdot_core::speech::wav(&vec![1000i16; 8000])).unwrap();
    let output = dir.join("out.wav");

    let out = Command::new(env!("CARGO_BIN_EXE_clawdot"))
        .args([
            "--config",
            cfg.to_str().unwrap(),
            "--data-dir",
            dir.join("state").to_str().unwrap(),
            "--voice-in",
            input.to_str().unwrap(),
            "--voice-out",
            output.to_str().unwrap(),
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "stdout: {stdout}\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("heard> set a timer for ten minutes"),
        "{stdout}"
    );
    assert!(
        stdout.contains("speaker> ") && stdout.to_lowercase().contains("timer"),
        "{stdout}"
    );

    let paths = provider.join().unwrap();
    assert_eq!(paths, vec!["/v1/audio/transcriptions", "/v1/audio/speech"]);
    let spoken = clawdot_core::speech::parse_wav(&std::fs::read(&output).unwrap()).unwrap();
    assert_eq!(
        spoken.len(),
        1600,
        "0.1 s of 24 kHz audio becomes 1600 samples at 16 kHz"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn voice_mode_explains_a_missing_speech_setup() {
    let dir = std::env::temp_dir().join(format!("clawdot-voice-missing-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("in.wav");
    std::fs::write(&input, clawdot_core::speech::wav(&[0i16; 100])).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_clawdot"))
        .args([
            "--data-dir",
            dir.join("state").to_str().unwrap(),
            "--voice-in",
            input.to_str().unwrap(),
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("speech.url is not configured"));
}

/// A fake speech provider that answers any number of requests until the test ends.
fn fake_speech_provider(heard: &'static str) -> u16 {
    let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = server.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for conn in server.incoming() {
            let Ok(mut c) = conn else { break };
            std::thread::spawn(move || {
                c.set_read_timeout(Some(Duration::from_secs(10))).ok();
                let mut raw = Vec::new();
                let mut buf = [0u8; 8192];
                loop {
                    let Ok(n) = c.read(&mut buf) else { return };
                    if n == 0 {
                        return;
                    }
                    raw.extend_from_slice(&buf[..n]);
                    if let Some(p) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&raw[..p]).to_ascii_lowercase();
                        let want: usize = head
                            .lines()
                            .find_map(|l| {
                                l.strip_prefix("content-length:")
                                    .map(|v| v.trim().parse().unwrap())
                            })
                            .unwrap_or(0);
                        if raw.len() >= p + 4 + want {
                            let path = head.split_whitespace().nth(1).unwrap_or("").to_owned();
                            let (ctype, body): (&str, Vec<u8>) =
                                if path.ends_with("/audio/transcriptions") {
                                    (
                                        "application/json",
                                        format!(r#"{{"text":"{heard}"}}"#).into_bytes(),
                                    )
                                } else {
                                    (
                                        "audio/pcm",
                                        (0..2400i16).flat_map(|i| (i % 50).to_le_bytes()).collect(),
                                    )
                                };
                            let mut resp = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                body.len()
                            )
                            .into_bytes();
                            resp.extend_from_slice(&body);
                            let _ = c.write_all(&resp);
                            return;
                        }
                    }
                }
            });
        }
    });
    port
}

#[test]
fn a_satellite_can_hold_a_whole_conversation_over_tcp() {
    use clawdot_protocol::message::{
        AudioFormat, Capabilities, Hello, Indicator, Message, PROTOCOL_VERSION, Wake,
    };
    use clawdot_protocol::{Decoder, Limits, encode};

    let speech_port = fake_speech_provider("set a timer for ten minutes");
    let sat_port = free_port();
    let dir = std::env::temp_dir().join(format!("clawdot-sat-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = dir.join("config.json");
    std::fs::write(
        &cfg,
        format!(
            r#"{{"speech": {{"url": "http://127.0.0.1:{speech_port}/v1", "stt_model": "w", "tts_model": "t", "voice": "v"}},
                "satellite": {{"listen": "127.0.0.1:{sat_port}"}}}}"#
        ),
    )
    .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_clawdot"))
        .args([
            "--no-console",
            "--echo-speech",
            "--config",
            cfg.to_str().unwrap(),
            "--listen",
            &format!("127.0.0.1:{}", free_port()),
            "--data-dir",
            dir.join("state").to_str().unwrap(),
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
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut ready = false;
    while Instant::now() < deadline && !ready {
        if let Ok(line) = rx.recv_timeout(Duration::from_millis(200)) {
            ready = line.contains("satellites connect to");
        }
    }
    assert!(ready, "the satellite server should start");

    let mut sock = TcpStream::connect(("127.0.0.1", sat_port)).unwrap();
    sock.set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    let mut dec = Decoder::new(Limits::default());
    let send =
        |sock: &mut TcpStream, m: Message| sock.write_all(&encode(&m.to_frame()).unwrap()).unwrap();
    let recv = |sock: &mut TcpStream, dec: &mut Decoder| -> Option<Message> {
        let end = Instant::now() + Duration::from_secs(10);
        let mut buf = [0u8; 8192];
        while Instant::now() < end {
            if let Some(f) = dec.next_frame().unwrap() {
                return Some(Message::from_frame(f).unwrap());
            }
            match sock.read(&mut buf) {
                Ok(0) => return None,
                Ok(n) => dec.push(&buf[..n]),
                Err(_) => {}
            }
        }
        None
    };

    send(
        &mut sock,
        Message::Hello(Hello {
            protocol: PROTOCOL_VERSION,
            device_id: "test".into(),
            name: None,
            token: None,
            capabilities: Capabilities::default(),
        }),
    );
    assert!(matches!(
        recv(&mut sock, &mut dec),
        Some(Message::Welcome(_))
    ));
    send(
        &mut sock,
        Message::Wake(Wake {
            name: "hey".into(),
            timestamp_ms: None,
        }),
    );
    assert_eq!(
        recv(&mut sock, &mut dec),
        Some(Message::SetIndicator(Indicator::Listening))
    );
    assert_eq!(recv(&mut sock, &mut dec), Some(Message::StreamStart));

    // 0.2 s of silence, 0.6 s of tone, 1 s of silence, in 20 ms frames.
    let mut samples = vec![0i16; 3200];
    samples.extend((0..9600).map(|i| (3000.0 * (f64::from(i) * 0.35).sin()) as i16));
    samples.extend(vec![0i16; 16000]);
    for chunk in samples.chunks(320) {
        let payload: Vec<u8> = chunk.iter().flat_map(|s| s.to_le_bytes()).collect();
        send(
            &mut sock,
            Message::AudioChunk {
                format: AudioFormat::PCM_16K_MONO,
                timestamp_ms: None,
                payload,
            },
        );
    }

    let mut got_audio = 0usize;
    let mut speaking = false;
    loop {
        match recv(&mut sock, &mut dec).expect("the core should reply") {
            Message::Ping => send(&mut sock, Message::Pong),
            Message::SetIndicator(Indicator::Speaking) => speaking = true,
            Message::AudioChunk { payload, .. } => got_audio += payload.len() / 2,
            Message::AudioStop => break,
            _ => {}
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(speaking, "the satellite is told to show the speaking state");
    assert_eq!(
        got_audio, 1600,
        "0.1 s of 24 kHz speech arrives as 1600 samples at 16 kHz"
    );
}

fn sat_send(sock: &mut TcpStream, m: &clawdot_protocol::message::Message) {
    sock.write_all(&clawdot_protocol::encode(&m.to_frame()).unwrap())
        .unwrap();
}

/// Next message from the core, answering keepalive pings along the way; `None` on timeout or close.
fn sat_recv(
    sock: &mut TcpStream,
    dec: &mut clawdot_protocol::Decoder,
    secs: u64,
) -> Option<clawdot_protocol::message::Message> {
    use clawdot_protocol::message::Message;
    let end = Instant::now() + Duration::from_secs(secs);
    let mut buf = [0u8; 8192];
    while Instant::now() < end {
        if let Some(f) = dec.next_frame().unwrap() {
            match Message::from_frame(f).unwrap() {
                Message::Ping => sat_send(sock, &Message::Pong),
                m => return Some(m),
            }
            continue;
        }
        match sock.read(&mut buf) {
            Ok(0) => return None,
            Ok(n) => dec.push(&buf[..n]),
            Err(_) => {}
        }
    }
    None
}

#[test]
fn an_agent_event_is_spoken_on_the_satellite_and_a_spoken_reply_reaches_the_bridge() {
    use clawdot_protocol::message::{
        AudioFormat, Capabilities, Hello, Indicator, Message, PROTOCOL_VERSION,
    };
    use clawdot_protocol::{Decoder, Limits};

    let speech_port = fake_speech_provider("keep going and add tests");
    let (http_port, sat_port) = (free_port(), free_port());
    let dir = std::env::temp_dir().join(format!("clawdot-headline-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = dir.join("config.json");
    std::fs::write(
        &cfg,
        format!(
            r#"{{"speech": {{"url": "http://127.0.0.1:{speech_port}/v1", "stt_model": "w", "tts_model": "t", "voice": "v"}},
                "satellite": {{"listen": "127.0.0.1:{sat_port}"}}}}"#
        ),
    )
    .unwrap();
    let token = create_endpoint(&dir.join("state"));
    let mut child = Command::new(env!("CARGO_BIN_EXE_clawdot"))
        .args([
            "--no-console",
            "--echo-speech",
            "--config",
            cfg.to_str().unwrap(),
            "--listen",
            &format!("127.0.0.1:{http_port}"),
            "--data-dir",
            dir.join("state").to_str().unwrap(),
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
    let mut ready = false;
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline && !ready {
        if let Ok(line) = rx.recv_timeout(Duration::from_millis(200)) {
            ready |= line.contains("satellites connect to");
        }
    }
    assert!(ready);

    // The satellite connects and waits.
    let mut sock = TcpStream::connect(("127.0.0.1", sat_port)).unwrap();
    sock.set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    let mut dec = Decoder::new(Limits::default());
    sat_send(
        &mut sock,
        &Message::Hello(Hello {
            protocol: PROTOCOL_VERSION,
            device_id: "kitchen".into(),
            name: None,
            token: None,
            capabilities: Capabilities::default(),
        }),
    );
    assert!(matches!(
        sat_recv(&mut sock, &mut dec, 10),
        Some(Message::Welcome(_))
    ));

    // An agent finishes; the speaker announces it out loud.
    assert_eq!(post_hook(http_port, &token, STOP), 202);
    let mut announced = 0usize;
    loop {
        match sat_recv(&mut sock, &mut dec, 15).expect("the announcement should be spoken") {
            Message::AudioChunk { payload, .. } => announced += payload.len() / 2,
            Message::AudioStop => break,
            _ => {}
        }
    }
    assert!(announced > 0, "the announcement has audio");

    // The satellite finished playing: listening reopens without the wake word.
    sat_send(&mut sock, &Message::PlaybackDone { id: None });
    assert_eq!(
        sat_recv(&mut sock, &mut dec, 10),
        Some(Message::SetIndicator(Indicator::Listening))
    );
    assert_eq!(
        sat_recv(&mut sock, &mut dec, 10),
        Some(Message::StreamStart)
    );

    // The user answers aloud.
    let mut samples = vec![0i16; 3200];
    samples.extend((0..9600).map(|i| (3000.0 * (f64::from(i) * 0.35).sin()) as i16));
    samples.extend(vec![0i16; 16000]);
    for chunk in samples.chunks(320) {
        let payload: Vec<u8> = chunk.iter().flat_map(|s| s.to_le_bytes()).collect();
        sat_send(
            &mut sock,
            &Message::AudioChunk {
                format: AudioFormat::PCM_16K_MONO,
                timestamp_ms: None,
                payload,
            },
        );
    }
    loop {
        if sat_recv(&mut sock, &mut dec, 15).expect("a spoken confirmation") == Message::AudioStop {
            break;
        }
    }

    // The instruction is waiting for the agent bridge.
    let mut stream = TcpStream::connect(("127.0.0.1", http_port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    write!(
        stream,
        "GET /bridge/claude-code/replies HTTP/1.1\r\nHost: t\r\nAuthorization: Bearer {token}\r\nAccept: text/event-stream\r\n\r\n"
    )
    .unwrap();
    let mut got = String::new();
    let mut buf = [0u8; 4096];
    let end = Instant::now() + Duration::from_secs(10);
    while Instant::now() < end && !got.contains("add tests") {
        if let Ok(n) = stream.read(&mut buf) {
            if n == 0 {
                break;
            }
            got.push_str(&String::from_utf8_lossy(&buf[..n]));
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        got.contains("add tests"),
        "the spoken instruction should reach the bridge: {got}"
    );
    assert!(got.contains("instruct"), "{got}");
}

/// A tiny MCP server (Streamable HTTP, plain JSON replies) that records the `tools/call` requests it receives.
fn fake_mcp_server() -> (u16, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = server.local_addr().unwrap().port();
    let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = std::sync::Arc::clone(&calls);
    std::thread::spawn(move || {
        for conn in server.incoming() {
            let Ok(mut c) = conn else { break };
            let log = std::sync::Arc::clone(&log);
            std::thread::spawn(move || {
                c.set_read_timeout(Some(Duration::from_secs(10))).ok();
                let mut raw = Vec::new();
                let mut buf = [0u8; 8192];
                loop {
                    let Ok(n) = c.read(&mut buf) else { return };
                    if n == 0 {
                        return;
                    }
                    raw.extend_from_slice(&buf[..n]);
                    let Some(p) = raw.windows(4).position(|w| w == b"\r\n\r\n") else {
                        continue;
                    };
                    let head = String::from_utf8_lossy(&raw[..p]).to_ascii_lowercase();
                    let want: usize = head
                        .lines()
                        .find_map(|l| {
                            l.strip_prefix("content-length:")
                                .map(|v| v.trim().parse().unwrap())
                        })
                        .unwrap_or(0);
                    if raw.len() < p + 4 + want {
                        continue;
                    }
                    let msg: serde_json::Value =
                        serde_json::from_slice(&raw[p + 4..p + 4 + want]).unwrap();
                    let id = msg["id"].clone();
                    let (status, body) = match msg["method"].as_str().unwrap_or("") {
                        "initialize" => ("200 OK", serde_json::json!({"jsonrpc":"2.0","id":id,"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"f","version":"1"}}}).to_string()),
                        "notifications/initialized" => ("202 Accepted", String::new()),
                        "tools/list" => ("200 OK", serde_json::json!({"jsonrpc":"2.0","id":id,"result":{"tools":[{"name":"turn_on","description":"Turn a light on","inputSchema":{"type":"object","properties":{"room":{"type":"string"}},"required":["room"]}}]}}).to_string()),
                        "tools/call" => {
                            log.lock().unwrap().push(msg["params"].to_string());
                            ("200 OK", serde_json::json!({"jsonrpc":"2.0","id":id,"result":{"content":[{"type":"text","text":"the den light is on"}],"isError":false}}).to_string())
                        }
                        _ => ("400 Bad Request", String::new()),
                    };
                    let resp = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = c.write_all(resp.as_bytes());
                    return;
                }
            });
        }
    });
    (port, calls)
}

#[test]
fn an_mcp_tool_runs_only_after_the_user_confirms() {
    let (mcp_port, calls) = fake_mcp_server();
    let call = |id: &str, args: &str| {
        sse(&[
            &format!(
                r#"{{"choices":[{{"delta":{{"tool_calls":[{{"index":0,"id":"{id}","function":{{"name":"home__turn_on","arguments":{}}}}}]}}}}]}}"#,
                serde_json::to_string(args).unwrap()
            ),
            r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
        ])
    };
    let (llm_port, llm) = fake_llm(vec![
        call("c1", r#"{"room":"den","confirmed":true}"#),
        sse(&[
            r#"{"choices":[{"delta":{"content":"Done, the den light is on."},"finish_reason":"stop"}]}"#,
        ]),
    ]);
    let dir = std::env::temp_dir().join(format!("clawdot-mcp-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = dir.join("config.json");
    std::fs::write(
        &cfg,
        format!(
            r#"{{"llm": {{"url": "http://127.0.0.1:{llm_port}", "model": "m"}},
                "mcp": [{{"name": "home", "url": "http://127.0.0.1:{mcp_port}/mcp"}}]}}"#
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
    stdin.write_all(b"light up the den for me\n").unwrap();
    let asked = next_speech(&rx).expect("the host should ask");
    assert!(
        asked.contains("home__turn_on") && asked.contains("Yes or no?"),
        "got: {asked}"
    );
    stdin.write_all(b"yes\n").unwrap();
    let spoken = next_speech(&rx);
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(spoken.expect("an answer").contains("the den light is on"));
    let bodies = llm.join().unwrap();
    assert!(
        bodies[0].contains("home__turn_on"),
        "the MCP tool is offered to the model"
    );
    assert_eq!(
        bodies.len(),
        2,
        "the host asked without calling the model again"
    );
    let calls = calls.lock().unwrap();
    assert_eq!(
        calls.len(),
        1,
        "only the confirmed call reached the server: {calls:?}"
    );
    assert!(calls[0].contains(r#""name":"turn_on""#) && calls[0].contains(r#""room":"den""#));
    assert!(!calls[0].contains("confirmed"), "{}", calls[0]);
}

/// One HTTP request to the running binary; returns (status, body).
fn http(port: u16, method: &str, path: &str, token: &str, body: &str) -> (u16, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(
        s,
        "{method} {path} HTTP/1.1\r\nHost: t\r\nAuthorization: Bearer {token}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut raw = String::new();
    let _ = s.read_to_string(&mut raw);
    let status = raw
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    (
        status,
        raw.split_once("\r\n\r\n")
            .map(|(_, b)| b.to_owned())
            .unwrap_or_default(),
    )
}

#[test]
fn the_web_ui_api_shows_status_and_makes_the_speaker_talk() {
    let port = free_port();
    let dir = std::env::temp_dir().join(format!("clawdot-ui-test-{}", std::process::id()));
    let mut child = Command::new(env!("CARGO_BIN_EXE_clawdot"))
        .args([
            "--no-console",
            "--echo-speech",
            "--listen",
            &format!("127.0.0.1:{port}"),
            "--data-dir",
            dir.to_str().unwrap(),
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
    let (mut printed_token, mut listening) = (false, false);
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline && !listening {
        if let Ok(line) = rx.recv_timeout(Duration::from_millis(200)) {
            printed_token |= line.contains("cdt_");
            listening |= line.contains("listening on");
        }
    }
    assert!(
        listening && !printed_token,
        "no administrator token is printed (REQ-CFG-018)"
    );
    // First run: the API is closed until setup (REQ-CFG-018), then a password gives a session (REQ-CFG-019).
    assert_eq!(http(port, "GET", "/api/status", "", "").0, 403);
    let (status, b) = http(
        port,
        "POST",
        "/api/setup",
        "",
        r#"{"mode":"password","password":"correct horse battery"}"#,
    );
    assert_eq!(status, 200, "{b}");
    let admin = b
        .split("\"token\":\"")
        .nth(1)
        .and_then(|t| t.split('"').next())
        .expect("a session token")
        .to_owned();
    let saved = std::fs::read_to_string(dir.join("admin.json")).unwrap();
    assert!(saved.contains("pbkdf2-sha256$") && !saved.contains("correct horse"));

    // Status appears within a tick or two of starting.
    let mut body = String::new();
    let end = Instant::now() + Duration::from_secs(10);
    while Instant::now() < end {
        let (status, b) = http(port, "GET", "/api/status", &admin, "");
        assert_eq!(status, 200);
        if b.contains("\"llm\"") {
            body = b;
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(
        body.contains(r#""configured":false"#) && body.contains(r#""satellites":0"#),
        "{body}"
    );
    assert!(body.contains(r#""scheduled":[]"#), "{body}");

    assert_eq!(
        http(
            port,
            "POST",
            "/api/speak",
            &admin,
            r#"{"text":"testing one two"}"#
        )
        .0,
        202
    );
    let mut spoken = false;
    let end = Instant::now() + Duration::from_secs(10);
    while Instant::now() < end && !spoken {
        if let Ok(line) = rx.recv_timeout(Duration::from_millis(250)) {
            spoken = line == "speaker> testing one two";
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(spoken, "the test phrase should be spoken");
}

/// Saved settings sit between the configuration file and flags, and a live change reaches the running
/// assistant without a restart (REQ-CFG-038, REQ-CFG-039).
#[test]
fn saved_settings_layer_under_flags_and_apply_live() {
    let port = free_port();
    let dir = std::env::temp_dir().join(format!("clawdot-settings-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("settings.json"),
        r#"{"schema":1,"data":{"zone":"America/Phoenix","device_name":"Den"}}"#,
    )
    .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_clawdot"))
        .args([
            "--no-console",
            "--listen",
            &format!("127.0.0.1:{port}"),
            "--zone",
            "America/Chicago",
            "--data-dir",
            dir.to_str().unwrap(),
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
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut listening = false;
    while Instant::now() < deadline && !listening {
        if let Ok(line) = rx.recv_timeout(Duration::from_millis(200)) {
            listening |= line.contains("listening on");
        }
    }
    assert!(listening, "the server should start");
    let (_, b) = http(
        port,
        "POST",
        "/api/setup",
        "",
        r#"{"mode":"password","password":"correct horse battery"}"#,
    );
    let admin = b
        .split("\"token\":\"")
        .nth(1)
        .and_then(|t| t.split('"').next())
        .expect("a session token")
        .to_owned();
    // Not signed in: a change is refused and the file is untouched.
    let before = std::fs::read_to_string(dir.join("settings.json")).unwrap();
    assert_eq!(
        http(
            port,
            "PUT",
            "/api/settings",
            "wrong",
            r#"{"device_name":"X"}"#
        )
        .0,
        401
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("settings.json")).unwrap(),
        before
    );

    let (s, b) = http(port, "GET", "/api/settings", &admin, "");
    assert_eq!(s, 200, "{b}");
    let v: serde_json::Value = serde_json::from_str(&b).unwrap();
    assert_eq!(
        v["fields"]["zone"]["value"], "America/Chicago",
        "a flag beats a saved value"
    );
    assert_eq!(v["fields"]["zone"]["source"], "flag");
    assert_eq!(v["fields"]["zone"]["ignored"], true);
    assert_eq!(v["fields"]["device_name"]["value"], "Den");
    assert_eq!(v["fields"]["device_name"]["source"], "saved");

    // A live change shows up in the running assistant's status.
    let (s, b) = http(
        port,
        "PUT",
        "/api/settings",
        &admin,
        r#"{"max_requests_per_day":7}"#,
    );
    assert_eq!(s, 200, "{b}");
    let mut seen = false;
    let end = Instant::now() + Duration::from_secs(10);
    while Instant::now() < end && !seen {
        let (_, b) = http(port, "GET", "/api/status", &admin, "");
        seen = b.contains(r#""max_requests_per_day":7"#);
        std::thread::sleep(Duration::from_millis(200));
    }
    let saved = std::fs::read_to_string(dir.join("settings.json")).unwrap();
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(seen, "the new limit should reach the running assistant");
    assert!(
        saved.contains("max_requests_per_day") && saved.contains("Den"),
        "{saved}"
    );
}

/// A taken port stops startup with a message that names `--listen`, and no administrator token is
/// created or printed for a server that never started (REQ-CFG-014, REQ-CFG-015).
#[test]
fn a_taken_port_fails_without_revealing_a_token() {
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = taken.local_addr().unwrap().port();
    let dir = std::env::temp_dir().join(format!("clawdot-bind-{}", std::process::id()));
    let out = Command::new(env!("CARGO_BIN_EXE_clawdot"))
        .args([
            "--no-console",
            "--echo-speech",
            "--listen",
            &format!("127.0.0.1:{port}"),
            "--data-dir",
            dir.to_str().unwrap(),
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!out.status.success());
    let (stdout, stderr) = (
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(stderr.contains("--listen"), "{stderr}");
    assert!(
        !stdout.contains("cdt_"),
        "no token may be printed: {stdout}"
    );
    assert!(!dir.join("admin-token.hash").exists());
    assert!(!dir.join("admin.json").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Start the binary, wait for the "listening" line, and return it with everything printed so far.
fn start_and_read(args: &[&str]) -> (std::process::Child, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_clawdot"))
        .args(["--no-console", "--echo-speech"])
        .args(args)
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
    let mut seen = String::new();
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline && !seen.contains("listening on") {
        if let Ok(line) = rx.recv_timeout(Duration::from_millis(200)) {
            seen.push_str(&line);
            seen.push('\n');
        }
    }
    (child, seen)
}

/// No setup code is printed, whatever the listen address: the device may have no visible console
/// (REQ-CFG-021). The URL and the first-visitor warning are, and setup completes without a code.
#[test]
fn setup_prints_the_url_and_a_warning_but_no_code() {
    let dir = std::env::temp_dir().join(format!("clawdot-code-{}", std::process::id()));
    let port = free_port();
    let (mut child, seen) = start_and_read(&[
        "--listen",
        &format!("0.0.0.0:{port}"),
        "--data-dir",
        dir.to_str().unwrap(),
    ]);
    assert!(
        seen.contains("Web UI: http://") && seen.contains("first-run setup"),
        "{seen}"
    );
    assert!(seen.contains("first person on a private network"), "{seen}");
    assert!(!seen.to_lowercase().contains("setup code"), "{seen}");
    assert!(!seen.contains("cdt_"), "{seen}");
    let body = r#"{"mode":"password","password":"correct horse battery"}"#;
    assert_eq!(http(port, "POST", "/api/setup", "", body).0, 200);
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// An old admin-token.hash is converted and its token still signs in (REQ-CFG-023); --reset-admin clears
/// only the administrator setting, then setup shows again (REQ-CFG-022).
#[test]
fn legacy_token_migration_and_reset_admin() {
    let dir = std::env::temp_dir().join(format!("clawdot-reset-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let old = "cdt_oldtokenoldtokenoldtokenoldtokenoldtoken1";
    std::fs::write(
        dir.join("admin-token.hash"),
        clawdot_core::inbox::endpoint::hash_token(old),
    )
    .unwrap();
    let port = free_port();
    let args = [
        "--listen".to_owned(),
        format!("127.0.0.1:{port}"),
        "--data-dir".to_owned(),
        dir.to_str().unwrap().to_owned(),
    ];
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let (mut child, seen) = start_and_read(&args);
    assert!(
        seen.contains("old administrator token still works"),
        "{seen}"
    );
    assert!(!dir.join("admin-token.hash").exists() && dir.join("admin.json").exists());
    let login = format!(r#"{{"password":"{old}"}}"#);
    let (status, b) = http(port, "POST", "/api/login", "", &login);
    assert_eq!(status, 200, "{b}");
    let _ = child.kill();
    let _ = child.wait();

    // Reset (server stopped): the setting goes, other state stays.
    std::fs::write(dir.join("other.json"), "keep").unwrap();
    let reset = || {
        let out = Command::new(env!("CARGO_BIN_EXE_clawdot"))
            .args(["--reset-admin", "--data-dir", dir.to_str().unwrap()])
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(out.status.success());
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    assert!(reset().contains("cleared"));
    assert!(!dir.join("admin.json").exists());
    assert!(dir.join("other.json").exists());
    assert!(reset().contains("no administrator setting"));
    let (mut child, _) = start_and_read(&args);
    assert_eq!(
        http(port, "GET", "/api/status", "", "").0,
        403,
        "setup shows again"
    );
    let _ = child.kill();
    let _ = child.wait();

    // The removed flag explains itself.
    let out = Command::new(env!("CARGO_BIN_EXE_clawdot"))
        .arg("--rotate-admin-token")
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("--reset-admin"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Printed snippets use the listen address when `--public-url` is absent (REQ-CFG-016), and the
/// command exits after printing (REQ-CFG-015).
#[test]
fn snippets_follow_the_listen_address() {
    let dir = std::env::temp_dir().join(format!("clawdot-snippet-{}", std::process::id()));
    let run = |n: u8, listen: &str, extra: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_clawdot"))
            .args([
                "--no-console",
                "--echo-speech",
                "--listen",
                listen,
                "--data-dir",
                dir.join(n.to_string()).to_str().unwrap(),
                "--create-endpoint",
                "claude-code",
            ])
            .args(extra)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(out.status.success());
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let lan = run(1, "192.168.1.20:9000", &[]);
    assert!(
        lan.contains("http://192.168.1.20:9000/hook/claude-code"),
        "{lan}"
    );
    assert!(!lan.contains("127.0.0.1:8080"));
    let any = run(2, "0.0.0.0:9000", &[]);
    assert!(
        any.contains("http://clawdot.local:9000/hook/claude-code"),
        "{any}"
    );
    let explicit = run(3, "0.0.0.0:9000", &["--public-url", "http://dot:1"]);
    assert!(
        explicit.contains("http://dot:1/hook/claude-code"),
        "{explicit}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Typing `quit` leaves the console (REQ-CFG-017).
#[test]
fn quit_leaves_the_console() {
    let port = free_port();
    let dir = std::env::temp_dir().join(format!("clawdot-quit-{}", std::process::id()));
    let mut child = Command::new(env!("CARGO_BIN_EXE_clawdot"))
        .args([
            "--listen",
            &format!("127.0.0.1:{port}"),
            "--data-dir",
            dir.to_str().unwrap(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(500));
    child.stdin.as_mut().unwrap().write_all(b"quit\n").unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break Some(s);
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            break None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let _ = std::fs::remove_dir_all(&dir);
    assert!(status.expect("`quit` should end the program").success());
}
