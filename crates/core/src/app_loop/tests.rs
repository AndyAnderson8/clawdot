//! The loop's events handled in process, with no binary, socket or terminal: each `on_*` method is called
//! directly and its effects are read from the state the loop owns.

use super::*;
use crate::app::{Adapters, Fail, Limits, Op, Stores};
use crate::extensions::ExtOp;
use crate::sched::{Clock, SystemClock};
use crate::session::ToolHost;
use crate::speaker::Say;
use crate::startup::build_brain;
use crate::store::JsonStore;
use crate::store::testutil::TempDir;
use crate::{cli, seed};
use std::sync::atomic::AtomicBool;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::watch;

mod connections;
mod portal;
mod turns;

type Map = serde_json::Map<String, serde_json::Value>;

/// An application over a scratch data directory, and what the speaker task would receive.
struct Rig {
    app: App,
    said: mpsc::UnboundedReceiver<Say>,
}

async fn rig(dir: &TempDir, extra: &[&str]) -> Rig {
    rig_with(dir, extra, Arc::new(SystemClock)).await
}

async fn rig_with(dir: &TempDir, extra: &[&str], clock: Arc<dyn Clock + Send + Sync>) -> Rig {
    let mut argv = vec!["--data-dir".to_owned(), dir.0.display().to_string()];
    argv.extend(extra.iter().map(|s| (*s).to_owned()));
    let (cli, file) = cli::parse(&argv).unwrap();
    let (args, state, _) = seed::resolve(cli, file, &crate::env::Env::empty()).unwrap();
    let live = state.live_values();
    let core = Arc::new(
        Core::open(
            clock.clone(),
            Adapters::builtin(),
            Limits::default(),
            Stores::in_dir(&args.data_dir),
        )
        .unwrap(),
    );
    let zone = crate::runtime::zone_for(None).unwrap();
    let (brain, hub) = build_brain(&args, &live, zone, &clock, &core)
        .await
        .unwrap();
    let (usage, usage_store) = brain.usage_handles();
    let announcer = Announcer::default();
    let voice = Arc::new(Voice {
        announcer: announcer.clone(),
        core: Arc::clone(&core),
        usage,
        usage_store,
        echo: false,
        available: AtomicBool::new(false),
        polling: AtomicBool::new(false),
        waits_for_satellite: false,
        rung_store: None,
    });
    let (tx, said) = mpsc::unbounded_channel();
    // Nothing sends on these; the loop is driven by calling its methods.
    let (_p, portal) = mpsc::channel(1);
    let (_m, mcp) = mpsc::channel(1);
    let (_r, realtime) = mpsc::channel(1);
    let (_a, asks) = mpsc::channel(1);
    let (_l, lines) = mpsc::channel(1);
    let sources = Sources {
        portal,
        mcp,
        realtime,
        asks,
        lines,
        console: true,
    };
    let speaker_task = SpeakerHandle {
        tx,
        voice,
        announcer,
        console: true,
        lost: false,
    };
    let app = App::new(brain, hub, core, live, sources, speaker_task);
    Rig { app, said }
}

/// `live` with `changes` put over it.
fn with(live: &Map, changes: &[(&str, serde_json::Value)]) -> Map {
    let mut m = live.clone();
    for (k, v) in changes {
        m.insert((*k).to_owned(), v.clone());
    }
    m
}

/// How many timers are saved in the data directory `dir`.
fn timers_on_disk(dir: &TempDir) -> usize {
    let store = JsonStore::new(dir.0.join("timers.json"), crate::sched::SCHEMA);
    crate::sched::Scheduler::load(&store, 64, &crate::sched::SystemClock)
        .unwrap()
        .list(&crate::sched::SystemClock)
        .len()
}

/// A portal job for `op`, and the receiver its answer arrives on.
fn ask(
    op: Op,
) -> (
    Job,
    tokio::sync::oneshot::Receiver<Result<serde_json::Value, Fail>>,
) {
    let (reply, answer) = tokio::sync::oneshot::channel();
    (Job { op, reply }, answer)
}

fn add_home(port: u16) -> Op {
    Op::Ext(ExtOp::McpAdd(serde_json::json!({
        "name": "home", "url": format!("http://127.0.0.1:{port}/mcp"),
    })))
}

/// A loopback language model that answers every request with `reply`, but only once `open` says so.
async fn gated_model(open: watch::Receiver<bool>, reply: &'static str) -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut c, _)) = l.accept().await else {
                return;
            };
            let mut open = open.clone();
            tokio::spawn(async move {
                let mut raw = Vec::new();
                let mut buf = [0u8; 8192];
                loop {
                    let Ok(n) = c.read(&mut buf).await else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
                    raw.extend_from_slice(&buf[..n]);
                    if let Some(p) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&raw[..p]).to_ascii_lowercase();
                        let want: usize = head
                            .lines()
                            .find_map(|l| l.strip_prefix("content-length:")?.trim().parse().ok())
                            .unwrap_or(0);
                        if raw.len() >= p + 4 + want {
                            break;
                        }
                    }
                }
                if open.wait_for(|v| *v).await.is_err() {
                    return;
                }
                let body = serde_json::json!({
                    "choices": [{"message": {"role": "assistant", "content": reply}, "finish_reason": "stop"}],
                    "usage": {"prompt_tokens": 5, "completion_tokens": 2},
                })
                .to_string();
                let _ = c
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
    port
}

// REQ-AST-021: once a second the loop publishes the status for the web UI and hands what is due (a portal
// phrase) to the speaker task, then asks it to look for an announcement.
#[tokio::test(flavor = "current_thread")]
async fn the_tick_publishes_the_status_and_hands_speech_to_the_speaker() {
    let dir = TempDir::new("loop-tick");
    let Rig { mut app, mut said } = rig(&dir, &[]).await;
    assert!(app.core.ui_status().is_null(), "nothing is published yet");
    assert!(app.core.queue_speech("Hello from the portal"));

    app.on_tick();

    let status = app.core.ui_status();
    assert_eq!(status["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(status["speaker"]["name"], "console");
    match said.try_recv().unwrap() {
        Say::Phrase { text, preview } => {
            assert_eq!((text.as_str(), preview), ("Hello from the portal", None))
        }
        other => panic!("expected the portal phrase first, got {other:?}"),
    }
    assert!(matches!(said.try_recv().unwrap(), Say::Poll { .. }));
    assert!(said.try_recv().is_err(), "nothing else was due");

    // A poll still running is not asked for again.
    app.on_tick();
    assert!(said.try_recv().is_err());
}

// A speaker task that is gone is reported once, and the loop carries on.
#[tokio::test(flavor = "current_thread")]
async fn a_lost_speaker_task_does_not_stop_the_tick() {
    let dir = TempDir::new("loop-lost-speaker");
    let Rig { mut app, said } = rig(&dir, &[]).await;
    drop(said);
    assert!(app.core.queue_speech("anyone there"));
    app.on_tick();
    assert!(app.speaker_task.as_ref().is_some_and(|s| s.lost));
    app.on_tick();
    assert!(!app.core.ui_status().is_null());
}

// REQ-AST-001: a line typed at the console is handled as a request: the fast path answers a timer at once;
// an empty line is ignored and "quit" leaves.
#[tokio::test(flavor = "current_thread")]
async fn a_console_line_becomes_a_request() {
    let dir = TempDir::new("loop-console");
    let Rig { mut app, .. } = rig(&dir, &[]).await;
    assert!(app.brain.assistant.scheduled().is_empty());

    let flow = app
        .on_console_line(Some("set a timer for 5 minutes".into()))
        .await;
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.brain.assistant.scheduled().len(), 1);

    assert_eq!(
        app.on_console_line(Some(String::new())).await,
        Flow::Continue
    );
    assert_eq!(app.brain.assistant.scheduled().len(), 1);

    assert_eq!(app.on_console_line(Some("quit".into())).await, Flow::Exit);
}

// A line the fast path cannot answer becomes a turn beside the loop, which the loop then finishes.
#[tokio::test(flavor = "current_thread")]
async fn a_console_line_for_the_model_starts_a_turn_beside_the_loop() {
    let dir = TempDir::new("loop-turn");
    let (open_tx, open) = watch::channel(false);
    let port = gated_model(open, "Owls hoot.").await;
    let url = format!("http://127.0.0.1:{port}/v1");
    let Rig { mut app, .. } = rig(&dir, &["--llm-url", &url, "--llm-model", "m"]).await;

    let flow = app
        .on_console_line(Some("tell me about owls and their habits".into()))
        .await;
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.hub.active(), 1, "the turn runs as a task");

    open_tx.send_replace(true);
    while app.hub.active() > 0 {
        let ev = app.hub.next().await;
        app.on_hub_event(ev).await;
    }
}

// REQ-AST-026: closing the terminal lets the turns that are running finish and show their replies before
// the loop leaves.
#[tokio::test(flavor = "current_thread")]
async fn console_eof_drains_running_turns_before_leaving() {
    let dir = TempDir::new("loop-eof");
    let (open_tx, open) = watch::channel(false);
    let port = gated_model(open, "Owls hoot.").await;
    let url = format!("http://127.0.0.1:{port}/v1");
    let Rig { mut app, .. } = rig(&dir, &["--llm-url", &url, "--llm-model", "m"]).await;

    app.on_console_line(Some("tell me about owls and their habits".into()))
        .await;
    assert_eq!(app.hub.active(), 1);

    open_tx.send_replace(true);
    let flow = app.on_console_line(None).await;
    assert_eq!(flow, Flow::Exit);
    assert_eq!(app.hub.active(), 0, "the running turn was finished first");
    assert!(
        app.brain.usage.requests_today() >= 1,
        "and its request was counted"
    );
}

// REQ-AST-007, REQ-AST-026: a speaker's idle time is a duration, so the loop sweeps conversations on the
// monotonic clock and a step of the wall clock neither keeps one alive nor drops it.
#[tokio::test(flavor = "current_thread")]
async fn speaker_eviction_follows_monotonic_time() {
    use crate::sched::testclock::StepClock;
    for (step, later, kept) in [
        (-3 * 3600, crate::speakers::IDLE_SECS + 1, false),
        (3 * 3600, crate::speakers::IDLE_SECS + 1, false),
        (3 * 3600, 60, true),
        (-3 * 3600, crate::speakers::IDLE_SECS, true),
    ] {
        let dir = TempDir::new("loop-evict");
        let (open_tx, open) = watch::channel(false);
        let port = gated_model(open, "Owls hoot.").await;
        let url = format!("http://127.0.0.1:{port}/v1");
        let clock = Arc::new(StepClock::at(1_794_225_600));
        let Rig { mut app, .. } = rig_with(
            &dir,
            &["--llm-url", &url, "--llm-model", "m"],
            clock.clone(),
        )
        .await;
        app.on_console_line(Some("tell me about owls and their habits".into()))
            .await;
        open_tx.send_replace(true);
        while app.hub.active() > 0 {
            let ev = app.hub.next().await;
            app.on_hub_event(ev).await;
        }
        assert_eq!(app.hub.speakers().len(), 1);
        clock.step_wall(step);
        clock.advance(later);
        app.on_tick();
        assert_eq!(
            app.hub.speakers().len() == 1,
            kept,
            "step {step} s, {later} s on the monotonic clock"
        );
    }
}

/// The one-shot command's application: the pieces of `app`, with no speaker task and only the connection
/// results as a source.
fn one_shot(app: App) -> App {
    App::one_shot(
        app.brain,
        app.hub,
        app.core,
        app.applied,
        Sources::mcp_only(mpsc::channel(1).1),
    )
}

// The recorded-utterance command answers through the same methods as the loop, with no loop and no speaker
// task: a fast-path line, a line that needs the model (its turn runs beside, and the call waits for it).
#[tokio::test(flavor = "current_thread")]
async fn a_one_shot_app_answers_through_the_loops_methods_without_a_loop() {
    let dir = TempDir::new("loop-one-shot");
    let (open_tx, open) = watch::channel(true);
    let port = gated_model(open, "Owls hoot.").await;
    let url = format!("http://127.0.0.1:{port}/v1");
    let Rig { app, .. } = rig(&dir, &["--llm-url", &url, "--llm-model", "m"]).await;
    let mut app = one_shot(app);
    assert!(app.speaker_task.is_none());

    let timer = app.respond_inline("set a timer for 5 minutes").await;
    assert!(timer.say.starts_with("Timer set"), "{}", timer.say);
    assert_eq!(app.brain.assistant.scheduled().len(), 1);

    let reply = app
        .respond_inline("tell me about owls and their habits")
        .await;
    assert_eq!(reply.say, "Owls hoot.");
    assert_eq!(app.hub.active(), 0);
    assert!(app.brain.usage.requests_today() >= 1, "and it was counted");

    // There is no speaker task to hand anything to, so a tick does nothing.
    app.on_tick();
    assert!(app.core.ui_status().is_null());
    drop(open_tx);
}

// With no model configured the one-shot command says so, as the loop does.
#[tokio::test(flavor = "current_thread")]
async fn a_one_shot_app_without_a_model_says_one_is_needed() {
    let dir = TempDir::new("loop-one-shot-no-model");
    let Rig { app, .. } = rig(&dir, &[]).await;
    let mut app = one_shot(app);
    let reply = app
        .respond_inline("tell me about owls and their habits")
        .await;
    assert_eq!(reply.say, crate::app_loop::turns::NO_LLM);
}
