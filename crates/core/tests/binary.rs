//! Runs the real `clawdot` binary: it must serve webhooks and announce what arrives.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

const STOP: &str = r#"{"session_id":"s1","cwd":"/home/dev/clawdot","hook_event_name":"Stop","last_assistant_message":"Done."}"#;

// The updater runs `--version` on a new program and requires the release version in the text (REQ-UPD-007); the
// commit follows it.
#[test]
fn version_prints_the_release_version_then_the_commit() {
    let out = Command::new(env!("CARGO_BIN_EXE_clawdot"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(out.status.success());
    let said = String::from_utf8(out.stdout).unwrap();
    let prefix = format!("clawdot {} (", env!("CARGO_PKG_VERSION"));
    assert!(said.starts_with(&prefix), "{said}");
    assert!(said.trim_end().ends_with(')'), "{said}");
}

/// A port nothing listens on, never handed out twice in this test process. Tests run in parallel threads, and
/// the operating system may give a port that was just released to the next caller, so two servers could
/// otherwise be told to bind the same one.
fn free_port() -> u16 {
    static GIVEN: std::sync::Mutex<Vec<u16>> = std::sync::Mutex::new(Vec::new());
    loop {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let mut given = GIVEN.lock().unwrap();
        if !given.contains(&port) {
            given.push(port);
            return port;
        }
    }
}

/// A scratch directory with a name no other run can have (process id, a counter, and the clock), removed when
/// dropped, also after a failed assertion. A directory left by a crashed run can therefore never be reused.
/// Declare it before the [`Server`] that uses it: the server is dropped (killed) first.
struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        Self(std::env::temp_dir().join(format!(
            "clawdot-{tag}-{}-{n}-{nanos:x}",
            std::process::id()
        )))
    }
}

impl std::ops::Deref for TempDir {
    type Target = std::path::Path;
    fn deref(&self) -> &std::path::Path {
        &self.0
    }
}

impl AsRef<std::path::Path> for TempDir {
    fn as_ref(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn scratch_directories_are_unique_and_removed_on_drop() {
    let (a, b) = (TempDir::new("t"), TempDir::new("t"));
    assert_ne!(a.0, b.0);
    std::fs::create_dir_all(a.join("x")).unwrap();
    let path = a.0.clone();
    drop(a);
    assert!(!path.exists());
}

// A machine that is compiling something else can take a long time to start a process, so these limits are
// ceilings for a failure, not expected waits: every wait ends the moment its line arrives or the process exits.
/// From spawning the binary to a line it prints once the listeners are up.
const START: Duration = Duration::from_secs(90);
/// For a reply from a running binary or a fake peer.
const REPLY: Duration = Duration::from_secs(60);

/// Connect to a local port, retrying while nothing accepts yet. The binary binds before it prints its first
/// line, so after that line a refusal is not expected; this covers the other cases without a fixed sleep.
fn connect(port: u16) -> TcpStream {
    let end = Instant::now() + REPLY;
    loop {
        match TcpStream::connect(("127.0.0.1", port)) {
            Ok(s) => return s,
            Err(e) => {
                assert!(
                    Instant::now() < end,
                    "nothing accepted a connection on port {port}: {e}"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
}

/// The running binary with its stdout read line by line on another thread. It is killed when dropped, so a
/// failed assertion does not leave a process behind.
struct Server {
    child: Child,
    stdin: Option<ChildStdin>,
    rx: mpsc::Receiver<String>,
    /// What the binary printed on stderr, read on another thread so the pipe never fills.
    err: Arc<Mutex<String>>,
    /// Set once a "listening on" line has been printed.
    listening: Arc<AtomicBool>,
}

impl Server {
    /// Start the binary with `args`. `console` gives it a piped stdin (see [`Server::type_line`]).
    fn start(args: &[&str], console: bool) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_clawdot"))
            .args(args)
            .stdin(if console {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let (tx, rx) = mpsc::channel::<String>();
        let listening = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&listening);
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if line.contains("listening on") {
                    flag.store(true, Ordering::SeqCst);
                }
                let _ = tx.send(line);
            }
        });
        let err = Arc::new(Mutex::new(String::new()));
        let sink = Arc::clone(&err);
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                let mut s = sink.lock().unwrap();
                s.push_str(&line);
                s.push('\n');
            }
        });
        Self {
            child,
            stdin,
            rx,
            err,
            listening,
        }
    }

    /// Start the binary on a free port. Every `{port}` in `args` is replaced by the port. The port is only
    /// known to be free until the child binds it, so when the child exits saying the port is taken, another
    /// port is picked and it is started again (a few times). Returns the server and the port it listens on.
    fn start_free(args: &[&str], console: bool) -> (Self, u16) {
        const TRIES: usize = 5;
        for attempt in 1..=TRIES {
            let port = free_port();
            let args: Vec<String> = args
                .iter()
                .map(|a| a.replace("{port}", &port.to_string()))
                .collect();
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            let mut server = Self::start(&args, console);
            let end = Instant::now() + START;
            loop {
                if server.listening.load(Ordering::SeqCst) {
                    return (server, port);
                }
                if server.child.try_wait().unwrap().is_some() {
                    // Let the stderr thread finish reading what the child printed.
                    std::thread::sleep(Duration::from_millis(100));
                    let taken = server.err.lock().unwrap().contains("is using port");
                    if taken && attempt < TRIES {
                        break;
                    }
                    return (server, port);
                }
                if Instant::now() >= end {
                    return (server, port);
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        unreachable!("the last attempt returns")
    }

    /// Wait for a printed line that `matches` accepts, within `within`, and return it. Fails at once, naming
    /// `what`, if the process exits first.
    fn wait_line(
        &self,
        what: &str,
        within: Duration,
        mut matches: impl FnMut(&str) -> bool,
    ) -> String {
        let end = Instant::now() + within;
        loop {
            let left = end.saturating_duration_since(Instant::now());
            assert!(
                !left.is_zero(),
                "timed out after {within:?} waiting for {what}"
            );
            match self.rx.recv_timeout(left) {
                Ok(line) if matches(&line) => return line,
                Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    panic!("the binary exited before it printed {what}")
                }
            }
        }
    }

    /// Wait until every text in `wanted` has appeared in some printed line, in any order. Returns all the
    /// lines read, one per line.
    fn wait_for(&self, wanted: &[&str], within: Duration) -> String {
        let mut missing: Vec<&str> = wanted.to_vec();
        let mut seen = String::new();
        self.wait_line(&format!("{wanted:?}"), within, |line| {
            seen.push_str(line);
            seen.push('\n');
            missing.retain(|w| !line.contains(w));
            missing.is_empty()
        });
        seen
    }

    /// Wait for the first start-up line (the HTTP listener is bound before it is printed).
    fn wait_listening(&self) -> String {
        self.wait_for(&["listening on"], START)
    }

    /// The next line the speaker says, if one comes within [`REPLY`].
    fn next_speech(&self) -> Option<String> {
        let end = Instant::now() + REPLY;
        while let Some(left) = end.checked_duration_since(Instant::now()) {
            match self.rx.recv_timeout(left) {
                Ok(line) if line.starts_with("speaker> ") => return Some(line),
                Ok(_) => {}
                Err(_) => return None,
            }
        }
        None
    }

    fn type_line(&mut self, text: &str) {
        let stdin = self.stdin.as_mut().expect("started with a console");
        stdin.write_all(text.as_bytes()).unwrap();
        stdin.write_all(b"\n").unwrap();
    }

    fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop();
    }
}

fn post_hook(port: u16, token: &str, body: &str) -> u16 {
    let mut s = connect(port);
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let req = format!(
        "POST /hook/claude-code HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {token}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
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
    let deadline = Instant::now() + START;
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
    let dir = TempDir::new("bin-test");
    let token = create_endpoint(&dir);
    let (mut server, port) = Server::start_free(
        &[
            "--no-console",
            "--echo-speech",
            "--listen",
            "127.0.0.1:{port}",
            "--data-dir",
            dir.to_str().unwrap(),
        ],
        false,
    );
    server.wait_listening();

    assert_eq!(post_hook(port, &token, STOP), 202);

    let spoken = server.wait_line("the announcement", REPLY, |l| {
        l.starts_with("speaker> ") && l.contains("clawdot finished")
    });
    server.stop();
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
    let dir = TempDir::new("llm-test");
    let (mut server, _) = Server::start_free(
        &[
            "--listen",
            "127.0.0.1:{port}",
            "--data-dir",
            dir.to_str().unwrap(),
            "--llm-url",
            &format!("http://127.0.0.1:{llm_port}"),
            "--llm-model",
            "test-model",
        ],
        true,
    );
    // Wait for the console before typing.
    server.wait_for(&["console ready"], START);

    server.type_line("tell me what you think about owls");
    let spoken = server.next_speech();
    server.stop();

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
    let dir = TempDir::new("bad-config");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.json");
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

    let dir = TempDir::new("tool-test");
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
    let (mut server, _) = Server::start_free(
        &[
            "--config",
            cfg.to_str().unwrap(),
            "--listen",
            "127.0.0.1:{port}",
            "--data-dir",
            dir.join("state").to_str().unwrap(),
        ],
        true,
    );
    server.wait_for(&["console ready"], START);
    server.type_line("make the den bright please");
    let asked = server.next_speech().expect("the host should ask");
    assert!(
        asked.contains("set light") && asked.contains("room: den") && asked.contains("Yes or no?"),
        "got: {asked}"
    );
    server.type_line("yes");
    let spoken = server.next_speech();
    server.stop();
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

    let dir = TempDir::new("voice-test");
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
    std::fs::write(&input, clawdot_core::audio::wav(&vec![1000i16; 8000])).unwrap();
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
    let spoken = clawdot_core::audio::parse_wav(&std::fs::read(&output).unwrap()).unwrap();
    assert_eq!(
        spoken.len(),
        1600,
        "0.1 s of 24 kHz audio becomes 1600 samples at 16 kHz"
    );
}

#[test]
fn voice_mode_explains_a_missing_speech_setup() {
    let dir = TempDir::new("voice-missing");
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("in.wav");
    std::fs::write(&input, clawdot_core::audio::wav(&[0i16; 100])).unwrap();
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
    let dir = TempDir::new("sat-test");
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
    let (mut server, _) = Server::start_free(
        &[
            "--no-console",
            "--echo-speech",
            "--config",
            cfg.to_str().unwrap(),
            "--listen",
            "127.0.0.1:{port}",
            "--data-dir",
            dir.join("state").to_str().unwrap(),
        ],
        false,
    );
    // The satellite listener is bound before this line is printed.
    server.wait_for(&["satellites connect to", "listening on"], START);

    let mut sock = connect(sat_port);
    sock.set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    let mut dec = Decoder::new(Limits::default());
    let send =
        |sock: &mut TcpStream, m: Message| sock.write_all(&encode(&m.to_frame()).unwrap()).unwrap();
    let recv = |sock: &mut TcpStream, dec: &mut Decoder| -> Option<Message> {
        let end = Instant::now() + REPLY;
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
    server.stop();
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
    within: Duration,
) -> Option<clawdot_protocol::message::Message> {
    use clawdot_protocol::message::Message;
    let end = Instant::now() + within;
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
    let sat_port = free_port();
    let dir = TempDir::new("headline");
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
    let (mut server, http_port) = Server::start_free(
        &[
            "--no-console",
            "--echo-speech",
            "--config",
            cfg.to_str().unwrap(),
            "--listen",
            "127.0.0.1:{port}",
            "--data-dir",
            dir.join("state").to_str().unwrap(),
        ],
        false,
    );
    // Both listeners are bound before these lines are printed.
    server.wait_for(&["satellites connect to", "listening on"], START);

    // The satellite connects and waits.
    let mut sock = connect(sat_port);
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
        sat_recv(&mut sock, &mut dec, REPLY),
        Some(Message::Welcome(_))
    ));

    // An agent finishes; the speaker announces it out loud.
    assert_eq!(post_hook(http_port, &token, STOP), 202);
    let mut announced = 0usize;
    loop {
        match sat_recv(&mut sock, &mut dec, REPLY).expect("the announcement should be spoken") {
            Message::AudioChunk { payload, .. } => announced += payload.len() / 2,
            Message::AudioStop => break,
            _ => {}
        }
    }
    assert!(announced > 0, "the announcement has audio");

    // The satellite finished playing: listening reopens without the wake word.
    sat_send(&mut sock, &Message::PlaybackDone { id: None });
    assert_eq!(
        sat_recv(&mut sock, &mut dec, REPLY),
        Some(Message::SetIndicator(Indicator::Listening))
    );
    assert_eq!(
        sat_recv(&mut sock, &mut dec, REPLY),
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
        if sat_recv(&mut sock, &mut dec, REPLY).expect("a spoken confirmation")
            == Message::AudioStop
        {
            break;
        }
    }

    // The instruction is waiting for the agent bridge.
    let mut stream = connect(http_port);
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    write!(
        stream,
        "GET /bridge/claude-code/replies HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {token}\r\nAccept: text/event-stream\r\n\r\n"
    )
    .unwrap();
    let mut got = String::new();
    let mut buf = [0u8; 4096];
    let end = Instant::now() + REPLY;
    while Instant::now() < end && !got.contains("add tests") {
        if let Ok(n) = stream.read(&mut buf) {
            if n == 0 {
                break;
            }
            got.push_str(&String::from_utf8_lossy(&buf[..n]));
        }
    }
    server.stop();
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
    let dir = TempDir::new("mcp-test");
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
    let (mut server, _) = Server::start_free(
        &[
            "--config",
            cfg.to_str().unwrap(),
            "--listen",
            "127.0.0.1:{port}",
            "--data-dir",
            dir.join("state").to_str().unwrap(),
        ],
        true,
    );
    // Saved and configured MCP servers connect after the loop starts (ADR-0021); a turn that begins earlier
    // does not have their tools, so wait for the one this test uses. The two lines come in either order, and
    // reading them one after the other dropped the second when it arrived first.
    server.wait_for(&["console ready", "mcp server `home` connected"], START);
    server.type_line("light up the den for me");
    let asked = server.next_speech().expect("the host should ask");
    assert!(
        asked.contains("home turn on")
            && asked.contains("room: den")
            && asked.contains("Yes or no?"),
        "got: {asked}"
    );
    server.type_line("yes");
    let spoken = server.next_speech();
    server.stop();
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
    let mut s = connect(port);
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    write!(
        s,
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {token}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
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
    let dir = TempDir::new("ui-test");
    let (mut server, port) = Server::start_free(
        &[
            "--no-console",
            "--echo-speech",
            "--listen",
            "127.0.0.1:{port}",
            "--data-dir",
            dir.to_str().unwrap(),
        ],
        false,
    );
    let seen = server.wait_listening();
    assert!(
        !seen.contains("cdt_"),
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
    let end = Instant::now() + REPLY;
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
    server.wait_line("the test phrase", REPLY, |l| {
        l == "speaker> testing one two"
    });
    server.stop();
}

/// Saved settings sit between the configuration file and flags, and a live change reaches the running
/// assistant without a restart (REQ-CFG-038, REQ-CFG-039).
// Uses named time zones, which only a build with the time zone database accepts.
#[cfg(feature = "tz")]
#[test]
fn saved_settings_layer_under_flags_and_apply_live() {
    let dir = TempDir::new("settings-test");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("settings.json"),
        r#"{"schema":1,"data":{"zone":"Europe/Berlin","device_name":"Den"}}"#,
    )
    .unwrap();
    let (mut server, port) = Server::start_free(
        &[
            "--no-console",
            "--listen",
            "127.0.0.1:{port}",
            "--zone",
            "America/Chicago",
            "--data-dir",
            dir.to_str().unwrap(),
        ],
        false,
    );
    server.wait_listening();
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
        r#"{"daily_spend_usd":7}"#,
    );
    assert_eq!(s, 200, "{b}");
    let mut seen = false;
    let end = Instant::now() + REPLY;
    while Instant::now() < end && !seen {
        let (_, b) = http(port, "GET", "/api/status", &admin, "");
        seen = b.contains(r#""daily_spend_cap_usd":7.0"#);
        std::thread::sleep(Duration::from_millis(200));
    }
    let saved = std::fs::read_to_string(dir.join("settings.json")).unwrap();
    server.stop();
    assert!(seen, "the new limit should reach the running assistant");
    assert!(
        saved.contains("daily_spend_usd") && saved.contains("Den"),
        "{saved}"
    );
}

/// The agent settings are layered (a flag beats a saved value), are validated, and reach the running assistant
/// without a restart: the daily caps show in the audit policy, and a bad value changes nothing (REQ-CFG-062).
#[test]
fn agent_settings_are_layered_validated_and_applied_live() {
    let dir = TempDir::new("agent-settings");
    std::fs::create_dir_all(&dir).unwrap();
    let (mut server, _, port) = start_free_and_read(&[
        "--no-console",
        "--dry-run",
        "--listen",
        "127.0.0.1:{port}",
        "--data-dir",
        dir.to_str().unwrap(),
    ]);
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
    let get = |path: &str| -> serde_json::Value {
        let (s, b) = http(port, "GET", path, &admin, "");
        assert_eq!(s, 200, "{b}");
        serde_json::from_str(&b).unwrap()
    };
    let v = get("/api/settings");
    assert_eq!(v["fields"]["agent_max_rounds"]["value"], 8, "the default");
    assert_eq!(v["fields"]["agent_max_rounds"]["source"], "default");
    assert_eq!(v["fields"]["dry_run"]["value"], true);
    assert_eq!(v["fields"]["dry_run"]["source"], "flag");
    assert_eq!(get("/api/audit")["policy"]["dry_run"], true);

    let (s, b) = http(
        port,
        "PUT",
        "/api/settings",
        &admin,
        r#"{"max_actions_per_day":3,"max_costly_per_day":1,"agent_max_rounds":5,"plan_speech":false,"quiet_start":"22:00","quiet_end":"07:00","notes":true,"dry_run":false}"#,
    );
    assert_eq!(s, 200, "{b}");
    let v = get("/api/settings");
    assert_eq!(v["fields"]["agent_max_rounds"]["source"], "saved");
    assert_eq!(
        v["fields"]["dry_run"]["source"], "flag",
        "a flag beats a saved value"
    );
    assert_eq!(v["fields"]["dry_run"]["ignored"], true);
    // The caps reach the running audit policy; the flag keeps dry run on.
    let mut policy = serde_json::Value::Null;
    let end = Instant::now() + REPLY;
    while Instant::now() < end && policy["max_actions_per_day"] != 3 {
        policy = get("/api/audit")["policy"].clone();
        std::thread::sleep(Duration::from_millis(200));
    }
    assert_eq!(policy["max_actions_per_day"], 3, "{policy}");
    assert_eq!(policy["max_costly_per_day"], 1);
    assert_eq!(policy["dry_run"], true, "the flag still wins");
    // Notes are on now, so the administrator can add one.
    let (s, b) = http(
        port,
        "POST",
        "/api/notes",
        &admin,
        r#"{"text":"the spare key is under the pot"}"#,
    );
    assert_eq!(s, 200, "{b}");
    // A bad value is refused with a reason and changes nothing.
    let (s, b) = http(
        port,
        "PUT",
        "/api/settings",
        &admin,
        r#"{"agent_max_rounds":99,"max_actions_per_day":4}"#,
    );
    assert_eq!(s, 400, "{b}");
    assert!(b.contains("1 to 20"), "{b}");
    assert_eq!(get("/api/audit")["policy"]["max_actions_per_day"], 3);
    server.stop();
    let saved = std::fs::read_to_string(dir.join("settings.json")).unwrap();
    assert!(
        saved.contains("quiet_start") && !saved.contains("\"agent_max_rounds\":99"),
        "{saved}"
    );
}

/// A taken port stops startup with a message that names `--listen`, and no administrator token is
/// created or printed for a server that never started (REQ-CFG-014, REQ-CFG-015).
#[test]
fn a_taken_port_fails_without_revealing_a_token() {
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = taken.local_addr().unwrap().port();
    let dir = TempDir::new("bind");
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
}

/// Start the binary, wait for the "listening" line, and return it with everything printed so far.
/// It fails at once if the binary exits first (a port already taken, say) and otherwise waits as long as
/// [`START`], so a slow start is not mistaken for a missing server.
fn start_and_read(args: &[&str]) -> (Server, String) {
    let mut all = vec!["--no-console", "--echo-speech"];
    all.extend_from_slice(args);
    let server = Server::start(&all, false);
    let seen = server.wait_listening();
    (server, seen)
}

/// [`start_and_read`] on a free port: `{port}` in `args` is replaced (see [`Server::start_free`]), and the port
/// is returned.
fn start_free_and_read(args: &[&str]) -> (Server, String, u16) {
    let mut all = vec!["--no-console", "--echo-speech"];
    all.extend_from_slice(args);
    let (server, port) = Server::start_free(&all, false);
    let seen = server.wait_listening();
    (server, seen, port)
}

/// No setup code is printed, whatever the listen address: the device may have no visible console
/// (REQ-CFG-021). The URL and the first-visitor warning are, and setup completes without a code.
#[test]
fn setup_prints_the_url_and_a_warning_but_no_code() {
    let dir = TempDir::new("code");
    let (mut server, seen, port) = start_free_and_read(&[
        "--listen",
        "0.0.0.0:{port}",
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
    server.stop();
}

/// An old admin-token.hash is converted and its token still signs in (REQ-CFG-023); --reset-admin clears
/// only the administrator setting, then setup shows again (REQ-CFG-022).
#[test]
fn legacy_token_migration_and_reset_admin() {
    let dir = TempDir::new("reset");
    std::fs::create_dir_all(&dir).unwrap();
    let old = "cdt_oldtokenoldtokenoldtokenoldtokenoldtoken1";
    std::fs::write(
        dir.join("admin-token.hash"),
        clawdot_core::inbox::endpoint::hash_token(old),
    )
    .unwrap();
    let data = dir.to_str().unwrap();
    let (mut server, seen, port) =
        start_free_and_read(&["--listen", "127.0.0.1:{port}", "--data-dir", data]);
    assert!(
        seen.contains("old administrator token still works"),
        "{seen}"
    );
    assert!(!dir.join("admin-token.hash").exists() && dir.join("admin.json").exists());
    let login = format!(r#"{{"password":"{old}"}}"#);
    let (status, b) = http(port, "POST", "/api/login", "", &login);
    assert_eq!(status, 200, "{b}");
    server.stop();

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
    let (mut server, _) =
        start_and_read(&["--listen", &format!("127.0.0.1:{port}"), "--data-dir", data]);
    assert_eq!(
        http(port, "GET", "/api/status", "", "").0,
        403,
        "setup shows again"
    );
    server.stop();

    // The removed flag explains itself.
    let out = Command::new(env!("CARGO_BIN_EXE_clawdot"))
        .arg("--rotate-admin-token")
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("--reset-admin"));
}

/// Printed snippets use the listen address when `--public-url` is absent (REQ-CFG-016), and the
/// command exits after printing (REQ-CFG-015).
#[test]
fn snippets_follow_the_listen_address() {
    let dir = TempDir::new("snippet");
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
    // The host is this machine's own name when it can be read (REQ-CFG-016), so only its shape is pinned here;
    // the choice of name is unit-tested.
    let named = any
        .split("/hook/claude-code")
        .next()
        .and_then(|head| head.rsplit("http://").next())
        .unwrap_or("");
    assert!(
        any.contains(&format!("http://{named}/hook/claude-code"))
            && named.ends_with(".local:9000")
            && !named.contains('/'),
        "{any}"
    );
    let explicit = run(3, "0.0.0.0:9000", &["--public-url", "http://dot:1"]);
    assert!(
        explicit.contains("http://dot:1/hook/claude-code"),
        "{explicit}"
    );
}

/// Typing `quit` leaves the console (REQ-CFG-017).
#[test]
fn quit_leaves_the_console() {
    let dir = TempDir::new("quit");
    let (mut server, _) = Server::start_free(
        &[
            "--listen",
            "127.0.0.1:{port}",
            "--data-dir",
            dir.to_str().unwrap(),
        ],
        true,
    );
    // Quit only once the console reads, instead of guessing how long start-up takes.
    server.wait_for(&["console ready"], START);
    server.type_line("quit");
    let deadline = Instant::now() + REPLY;
    let status = loop {
        if let Some(s) = server.child.try_wait().unwrap() {
            break Some(s);
        }
        if Instant::now() > deadline {
            break None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(status.expect("`quit` should end the program").success());
}

/// Wait for the program to end by itself and return whether it succeeded.
fn exits_by_itself(server: &mut Server) -> bool {
    let deadline = Instant::now() + REPLY;
    while Instant::now() < deadline {
        if let Some(s) = server.child.try_wait().unwrap() {
            return s.success();
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    false
}

/// REQ-TMR-003: a timer the speaker confirmed is on disk once the confirmation is out, and the program, ended
/// at once by `quit`, leaves the usage counters of the turn it just answered on disk too (they are written
/// beside the loop and flushed on the way out).
#[test]
fn a_confirmed_timer_and_the_last_turns_counters_are_on_disk_after_quit() {
    let answer =
        sse(&[r#"{"choices":[{"delta":{"content":"Owls are lovely."},"finish_reason":"stop"}]}"#]);
    let (llm_port, llm) = fake_llm(vec![answer]);
    let dir = TempDir::new("flush-on-quit");
    let (mut server, _) = Server::start_free(
        &[
            "--listen",
            "127.0.0.1:{port}",
            "--data-dir",
            dir.to_str().unwrap(),
            "--llm-url",
            &format!("http://127.0.0.1:{llm_port}"),
            "--llm-model",
            "test-model",
        ],
        true,
    );
    server.wait_for(&["console ready"], START);
    server.type_line("set a timer for 10 minutes");
    let said = server.next_speech().expect("the timer is confirmed");
    assert!(said.contains("Timer set"), "{said}");
    let timers =
        std::fs::read_to_string(dir.join("timers.json")).expect("saved before it was said");
    assert!(timers.contains("\"fire_at\""), "{timers}");

    server.type_line("tell me about owls");
    let said = server.next_speech().expect("the model answers");
    assert!(said.contains("Owls are lovely."), "{said}");
    server.type_line("quit");
    assert!(
        exits_by_itself(&mut server),
        "`quit` should end the program"
    );
    let usage = std::fs::read_to_string(dir.join("usage.json")).expect("flushed on the way out");
    assert!(usage.contains("\"requests\":1"), "{usage}");
    drop(llm);
}

/// Start the program with a console and a model that answers once, let it answer, send it the signal `sig`
/// (`TERM` or `INT`), and check that it leaves by itself with success and has left the usage counters of the
/// answer it just gave on disk.
#[cfg(unix)]
fn stop_with_signal_and_check_the_counters(sig: &str) {
    let answer =
        sse(&[r#"{"choices":[{"delta":{"content":"Owls are lovely."},"finish_reason":"stop"}]}"#]);
    let (llm_port, llm) = fake_llm(vec![answer]);
    let dir = TempDir::new("flush-on-signal");
    let (mut server, _) = Server::start_free(
        &[
            "--listen",
            "127.0.0.1:{port}",
            "--data-dir",
            dir.to_str().unwrap(),
            "--llm-url",
            &format!("http://127.0.0.1:{llm_port}"),
            "--llm-model",
            "test-model",
        ],
        true,
    );
    server.wait_for(&["console ready"], START);
    server.type_line("tell me about owls");
    let said = server.next_speech().expect("the model answers");
    assert!(said.contains("Owls are lovely."), "{said}");

    let sent = Command::new("kill")
        .arg(format!("-{sig}"))
        .arg(server.child.id().to_string())
        .status()
        .unwrap();
    assert!(sent.success(), "kill -{sig} failed");
    assert!(
        exits_by_itself(&mut server),
        "SIG{sig} should end the program with success, not kill it"
    );
    let usage = std::fs::read_to_string(dir.join("usage.json")).expect("flushed on the way out");
    assert!(usage.contains("\"requests\":1"), "{usage}");
    drop(llm);
}

/// REQ-CFG-085: SIGTERM, which is how a service manager stops the program, is an orderly stop like `quit`.
#[cfg(unix)]
#[test]
fn sigterm_stops_the_program_with_its_changes_on_disk() {
    stop_with_signal_and_check_the_counters("TERM");
}

/// REQ-CFG-085: so is SIGINT, which is Ctrl-C at a terminal.
#[cfg(unix)]
#[test]
fn sigint_stops_the_program_with_its_changes_on_disk() {
    stop_with_signal_and_check_the_counters("INT");
}
