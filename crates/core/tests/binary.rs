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
fn fake_speech_provider() -> u16 {
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

    let speech_port = fake_speech_provider();
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
