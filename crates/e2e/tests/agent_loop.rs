//! The headline loop, end to end, with real sockets:
//!
//! agent hook -> speaker announces -> user speaks an instruction -> bridge pulls it over SSE ->
//! bridge runs the resume command -> bridge acknowledges.
//!
//! No audio, no LLM, no network beyond localhost. This is the demo that needs no hardware.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use clawdot_bridge::client::Url;
use clawdot_bridge::harness::{ClaudeCode, CommandSpec};
use clawdot_bridge::state::Handled;
use clawdot_bridge::{Config, Runner, stream_once};
use clawdot_core::app::{Adapters, Core, LOCALHOST_PEER, Limits};
use clawdot_core::assistant::Assistant;
use clawdot_core::http::{ServeOptions, serve};
use clawdot_core::sched::{FixedOffset, Scheduler, SystemClock};
use clawdot_protocol::bridge::ReplyKind;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

const STOP: &str = r#"{"session_id":"sess-42","cwd":"/home/dev/clawdot","hook_event_name":"Stop","last_assistant_message":"I added the timer tool and all tests pass."}"#;

struct Fixture {
    core: Arc<Core>,
    addr: SocketAddr,
    token: String,
    assistant: Assistant,
}

fn start() -> Fixture {
    let clock = Arc::new(SystemClock);
    let core = Arc::new(Core::in_memory(
        clock.clone(),
        Adapters::builtin(),
        Limits::default(),
    ));
    let (_, token) = core.create_endpoint("claude-code", "claude-code").unwrap();

    let std_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    std_listener.set_nonblocking(true).unwrap();
    let addr = std_listener.local_addr().unwrap();
    let served = Arc::clone(&core);
    thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let listener = tokio::net::TcpListener::from_std(std_listener).unwrap();
            let _ = serve(
                listener,
                served,
                ServeOptions::default(),
                std::future::pending(),
            )
            .await;
        });
    });

    let assistant = Assistant::new(
        clawdot_core::lang::DEFAULT,
        clock,
        Box::new(FixedOffset(0)),
        Scheduler::new(16),
        None,
        Some(Arc::clone(&core)),
    );
    Fixture {
        core,
        addr,
        token,
        assistant,
    }
}

/// A blocking one-shot HTTP request; returns the status code.
fn post(addr: SocketAddr, path: &str, token: &str, body: &str) -> u16 {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
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

#[derive(Clone, Default)]
struct Recorder {
    ran: Arc<Mutex<Vec<CommandSpec>>>,
    fail_to_start: Arc<Mutex<bool>>,
}

impl Runner for Recorder {
    fn run(&mut self, spec: &CommandSpec) -> io::Result<i32> {
        if *self.fail_to_start.lock().unwrap() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "claude is not installed",
            ));
        }
        self.ran.lock().unwrap().push(spec.clone());
        Ok(0)
    }
}

fn bridge_config(f: &Fixture) -> Config {
    Config {
        url: Url::parse(&format!("http://{}", f.addr)).unwrap(),
        endpoint: "claude-code".into(),
        token: f.token.clone(),
        read_timeout: Duration::from_secs(20),
        harness: Box::new(ClaudeCode::default()),
    }
}

/// Announce the pending event the way the speaker would.
fn announce(f: &Fixture) {
    let ev = f
        .core
        .next_announcement(true, 0)
        .expect("an event should be waiting")
        .event;
    f.core.mark_announced(&[ev.id]);
}

#[test]
fn a_spoken_instruction_resumes_the_agent_session_and_is_acknowledged() {
    let mut f = start();
    assert_eq!(post(f.addr, "/hook/claude-code", &f.token, STOP), 202);
    announce(&f);

    // The user says it; the fast path turns it into a reply for the bridge.
    let out = f
        .assistant
        .handle("tell it to also add unit tests")
        .expect("handled locally");
    assert_eq!(out.say, "Okay, sending that to claude-code.");

    let recorder = Recorder::default();
    let mut handled = Handled::in_memory();
    let n = stream_once(
        &bridge_config(&f),
        &mut handled,
        &mut recorder.clone(),
        Some(1),
    )
    .unwrap();
    assert_eq!(n, 1);

    let ran = recorder.ran.lock().unwrap();
    assert_eq!(ran.len(), 1);
    assert_eq!(&ran[0].args[..3], ["-p", "--resume", "sess-42"]);
    assert!(ran[0].args.last().unwrap().ends_with("also add unit tests"));

    // Acknowledged: nothing left to redeliver.
    assert!(
        f.core
            .bridge_pending("claude-code", &f.token, LOCALHOST_PEER, 0)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_reply_queued_while_the_bridge_is_connected_arrives_promptly() {
    let mut f = start();
    post(f.addr, "/hook/claude-code", &f.token, STOP);
    announce(&f);

    let cfg = bridge_config(&f);
    let recorder = Recorder::default();
    let (done_tx, done_rx) = mpsc::channel();
    let bridge_recorder = recorder.clone();
    thread::spawn(move || {
        let mut handled = Handled::in_memory();
        let r = stream_once(&cfg, &mut handled, &mut bridge_recorder.clone(), Some(1));
        let _ = done_tx.send(r.is_ok());
    });
    thread::sleep(Duration::from_millis(300)); // let the bridge connect first
    f.assistant
        .handle("keep going and fix the failing test")
        .unwrap();

    assert_eq!(
        done_rx.recv_timeout(Duration::from_secs(10)),
        Ok(true),
        "the bridge should finish after one reply"
    );
    let ran = recorder.ran.lock().unwrap();
    assert!(
        ran[0]
            .args
            .last()
            .unwrap()
            .ends_with("fix the failing test")
    );
}

#[test]
fn thanks_ill_handle_it_acknowledges_without_running_anything() {
    let mut f = start();
    post(f.addr, "/hook/claude-code", &f.token, STOP);
    announce(&f);
    assert_eq!(
        f.assistant.handle("thanks I'll handle it").unwrap().say,
        "Okay. I'll let claude-code know you've got it."
    );

    let recorder = Recorder::default();
    let mut handled = Handled::in_memory();
    stream_once(
        &bridge_config(&f),
        &mut handled,
        &mut recorder.clone(),
        Some(1),
    )
    .unwrap();
    assert!(
        recorder.ran.lock().unwrap().is_empty(),
        "an acknowledgement runs no command"
    );
    assert!(
        f.core
            .bridge_pending("claude-code", &f.token, LOCALHOST_PEER, 0)
            .unwrap()
            .is_empty(),
        "but it is confirmed"
    );
}

#[test]
fn a_command_that_could_not_start_is_redelivered_and_runs_once_fixed() {
    let mut f = start();
    post(f.addr, "/hook/claude-code", &f.token, STOP);
    announce(&f);
    f.assistant
        .handle("tell it to go on with the refactor")
        .unwrap();

    let recorder = Recorder::default();
    *recorder.fail_to_start.lock().unwrap() = true;
    let mut handled = Handled::in_memory();
    stream_once(
        &bridge_config(&f),
        &mut handled,
        &mut recorder.clone(),
        Some(1),
    )
    .unwrap();
    assert!(recorder.ran.lock().unwrap().is_empty());
    assert_eq!(
        f.core
            .bridge_pending("claude-code", &f.token, LOCALHOST_PEER, 0)
            .unwrap()
            .len(),
        1,
        "not acknowledged, so still queued"
    );

    *recorder.fail_to_start.lock().unwrap() = false; // the user installs claude
    stream_once(
        &bridge_config(&f),
        &mut handled,
        &mut recorder.clone(),
        Some(1),
    )
    .unwrap();
    assert_eq!(recorder.ran.lock().unwrap().len(), 1);
    assert!(
        f.core
            .bridge_pending("claude-code", &f.token, LOCALHOST_PEER, 0)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn redelivery_after_a_lost_acknowledgement_never_runs_the_instruction_twice() {
    let mut f = start();
    post(f.addr, "/hook/claude-code", &f.token, STOP);
    announce(&f);
    f.assistant.handle("tell it to deploy the fix").unwrap();
    let reply = f
        .core
        .bridge_pending("claude-code", &f.token, LOCALHOST_PEER, 0)
        .unwrap()
        .remove(0);

    // The bridge handled it, but its acknowledgement was lost, so the device still has it queued.
    let recorder = Recorder::default();
    let mut handled = Handled::in_memory();
    handled
        .insert(clawdot_bridge::state::Key::of(&reply))
        .unwrap();
    stream_once(
        &bridge_config(&f),
        &mut handled,
        &mut recorder.clone(),
        Some(1),
    )
    .unwrap();
    assert!(
        recorder.ran.lock().unwrap().is_empty(),
        "already handled, so only re-acknowledged"
    );
    assert!(
        f.core
            .bridge_pending("claude-code", &f.token, LOCALHOST_PEER, 0)
            .unwrap()
            .is_empty()
    );
    assert_eq!(reply.kind, ReplyKind::Instruct);
}

#[test]
fn a_bridge_with_the_wrong_token_is_rejected_cleanly() {
    let f = start();
    let mut cfg = bridge_config(&f);
    cfg.token = "cdt_not_the_token".into();
    let mut handled = Handled::in_memory();
    let err = stream_once(&cfg, &mut handled, &mut Recorder::default(), Some(1)).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
}
