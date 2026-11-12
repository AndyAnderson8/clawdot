use super::*;
use crate::llm::{Completion, LlmError, Role, ToolCall};
use crate::sched::{Clock, FixedOffset, Scheduler};
use crate::session::Tier;
use crate::usage::UsageLimits;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// A clock that runs ten times faster than the real one, so a "two second" timer rings after 200 ms.
struct FastClock(Instant);
impl Clock for FastClock {
    fn now(&self) -> Ts {
        1_800_000_000 + i64::try_from(self.0.elapsed().as_millis() / 100).unwrap()
    }
    fn monotonic_secs(&self) -> i64 {
        self.now()
    }
}

/// A model that answers from what it is asked: `call NAME JSON` makes that call, anything with `slow`
/// takes a while, a tool result is repeated, and anything else is echoed.
struct Fake {
    delay: Duration,
    systems: Mutex<Vec<String>>,
    requests: Mutex<u32>,
}

impl Fake {
    fn new(delay_ms: u64) -> Arc<Self> {
        Arc::new(Self {
            delay: Duration::from_millis(delay_ms),
            systems: Mutex::new(Vec::new()),
            requests: Mutex::new(0),
        })
    }
}

impl ChatBackend for Fake {
    async fn complete(&self, req: &crate::llm::ChatRequest) -> Result<Completion, LlmError> {
        *self.requests.lock().unwrap() += 1;
        self.systems
            .lock()
            .unwrap()
            .push(req.messages[0].content.clone());
        let last = req.messages.last().unwrap();
        let text = |t: String| {
            Ok(Completion {
                text: t,
                ..Completion::default()
            })
        };
        if last.role == Role::Tool {
            return text(format!("result: {}", last.content));
        }
        if last.content.contains("slow") {
            tokio::time::sleep(self.delay).await;
        }
        if let Some(rest) = last.content.strip_prefix("call ") {
            let (name, args) = rest.split_once(' ').unwrap_or((rest, "{}"));
            return Ok(Completion {
                tool_calls: vec![ToolCall {
                    id: "c1".into(),
                    name: name.into(),
                    arguments: args.into(),
                }],
                ..Completion::default()
            });
        }
        text(format!("heard {}", last.content))
    }
}

/// The loop's side, as `main` has it, in miniature.
struct Lab {
    assistant: Assistant,
    hub: Hub,
    backend: Arc<Fake>,
    usage: UsageTracker,
    http: Arc<HttpTools>,
    mcp: Arc<McpClient>,
    ext: Extensions,
    ext_view: Arc<Extensions>,
    persona: String,
    builtin: Vec<String>,
    started: Instant,
    finished: Vec<(Duration, SpeakerId, String)>,
    fired: Vec<Duration>,
    answers: Vec<oneshot::Receiver<Answer>>,
    others: usize,
}

fn env(_: &str) -> Option<String> {
    None
}

impl Lab {
    async fn new(delay_ms: u64) -> Self {
        let clock: Arc<dyn Clock + Send + Sync> = Arc::new(FastClock(Instant::now()));
        let assistant = Assistant::new(
            clock,
            Box::new(FixedOffset(0)),
            Scheduler::new(16),
            None,
            None,
        );
        let builtin: Vec<String> = ToolHost::tool_specs(&assistant)
            .into_iter()
            .map(|t| t.name)
            .collect();
        let http = HttpTools::new(Vec::new(), &builtin, env).unwrap();
        let (mcp, _) = McpClient::connect(Vec::new(), &builtin, env).await.unwrap();
        let ext = Extensions::in_memory();
        let ext_view = Arc::new(ext.snapshot());
        Self {
            assistant,
            hub: Hub::new(|| Conversation::standard("persona")),
            backend: Fake::new(delay_ms),
            usage: UsageTracker::new(UsageLimits::default()),
            http: Arc::new(http),
            mcp: Arc::new(mcp),
            ext,
            ext_view,
            persona: "persona".into(),
            builtin,
            started: Instant::now(),
            finished: Vec::new(),
            fired: Vec::new(),
            answers: Vec::new(),
            others: 0,
        }
    }

    fn kit(&self) -> TurnKit<Fake> {
        TurnKit {
            backend: Arc::clone(&self.backend),
            cfg: TurnConfig {
                model: "m".into(),
                ..TurnConfig::default()
            },
            usage: self.usage.clone(),
            http: Arc::clone(&self.http),
            mcp: Arc::clone(&self.mcp),
            ext: Arc::clone(&self.ext_view),
            builtin: ToolHost::tool_specs(&self.assistant),
            system: self.persona.clone(),
            day: 1,
            now: 1_800_000_000,
            mono: 1_800_000_000,
        }
    }

    fn say(&mut self, speaker: &str, line: &str) {
        let (tx, rx) = oneshot::channel();
        let req = Request {
            speaker: speaker.into(),
            line: line.into(),
            ctl: TurnCtl::default(),
            origin: Origin::Ask(tx),
        };
        let kit = self.kit();
        assert!(matches!(
            self.hub.submit(req, 0, || kit),
            Submitted::Started | Submitted::Queued
        ));
        self.answers.push(rx);
    }

    async fn tool_off(&mut self, op: crate::extensions::ExtOp) {
        self.ext
            .apply(
                op,
                crate::extensions::ToolSets::new(
                    Arc::make_mut(&mut self.http),
                    Arc::make_mut(&mut self.mcp),
                    &self.builtin,
                    &env,
                ),
            )
            .await
            .unwrap();
        self.ext_view = Arc::new(self.ext.snapshot());
    }

    /// Run the loop until `n` turns have finished, ringing timers every 20 ms as the real one does.
    async fn run_until(&mut self, n: usize) {
        self.pump(|lab| lab.finished.len() >= n).await;
    }

    async fn pump(&mut self, done: impl Fn(&Self) -> bool) {
        let mut tick = tokio::time::interval(Duration::from_millis(20));
        while !done(self) {
            tokio::select! {
                ev = self.hub.next() => match ev {
                    Event::Tool(req) => req.serve(&mut self.assistant).await,
                    Event::Turn(f) => {
                        let speaker = f.speaker.clone();
                        let (origin, reply, next) = self.hub.finish(*f, 0);
                        self.finished.push((self.started.elapsed(), speaker, reply.say.clone()));
                        origin.deliver(reply);
                        if let Some(next) = next {
                            let kit = self.kit();
                            let _ = self.hub.submit(next, 0, || kit);
                        }
                    }
                    Event::Other(_) => self.others += 1,
                    Event::Lost(_) => {}
                },
                _ = tick.tick() => {
                    if !self.assistant.tick().is_empty() {
                        self.fired.push(self.started.elapsed());
                    }
                }
            }
        }
    }

    fn say_of(&self, speaker: &str) -> Vec<String> {
        self.finished
            .iter()
            .filter(|(_, s, _)| s == speaker)
            .map(|(_, _, t)| t.clone())
            .collect()
    }
}

// REQ-AST-027
#[tokio::test(flavor = "current_thread")]
async fn a_timer_rings_on_time_while_a_slow_model_turn_is_in_flight() {
    let mut lab = Lab::new(900).await;
    lab.assistant
        .call_tool("set_timer", &serde_json::json!({"seconds": 2}))
        .await
        .unwrap();
    lab.say("console", "that is slow");
    lab.run_until(1).await;
    let (done_at, _, _) = &lab.finished[0];
    assert!(
        done_at >= &Duration::from_millis(850),
        "the turn took its time"
    );
    assert_eq!(lab.fired.len(), 1, "the timer rang");
    assert!(
        lab.fired[0] < Duration::from_millis(500),
        "it rang at {:?}, long before the model answered at {done_at:?}",
        lab.fired[0]
    );
}

// REQ-AST-027, REQ-CFG-039: the loop applies a portal change while a turn runs; the running turn keeps
// the snapshot it began with and the next turn sees the change.
#[tokio::test(flavor = "current_thread")]
async fn a_portal_change_is_applied_during_a_turn_and_reaches_the_next_one() {
    let mut lab = Lab::new(600).await;
    lab.say("console", "slow one");
    // Let the turn begin, then change the persona and switch a tool off, as the portal would.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let before = Instant::now();
    lab.persona = "a new persona".into();
    lab.tool_off(crate::extensions::ExtOp::ToolEnable(
        "set_timer".into(),
        false,
    ))
    .await;
    assert!(
        before.elapsed() < Duration::from_millis(200),
        "the change did not wait for the turn"
    );
    assert_eq!(lab.hub.active(), 1, "the turn is still running");
    lab.run_until(1).await;
    lab.say("console", "second");
    lab.run_until(2).await;
    let systems = lab.backend.systems.lock().unwrap().clone();
    assert_eq!(systems.first().map(String::as_str), Some("persona"));
    assert_eq!(
        systems.last().map(String::as_str),
        Some("a new persona"),
        "the next turn has the new persona"
    );
}

// REQ-AST-026, REQ-SEC-011
#[tokio::test(flavor = "current_thread")]
async fn two_speakers_have_separate_conversations_and_separate_held_confirmations() {
    let mut lab = Lab::new(10).await;
    // Make the timer tool costly, so a call to it is held for a yes.
    lab.tool_off(crate::extensions::ExtOp::ToolTier(
        "set_timer".into(),
        Some(Tier::Costly),
    ))
    .await;

    lab.say("sat:a", r#"call set_timer {"seconds": 600}"#);
    lab.run_until(1).await;
    assert!(
        lab.say_of("sat:a")[0].contains("Yes or no?"),
        "{:?}",
        lab.say_of("sat:a")
    );
    assert!(lab.assistant.scheduled().is_empty(), "nothing has run yet");

    // B says yes. It has nothing held, so the yes is just words for the model, and nothing runs.
    lab.say("sat:b", "yes");
    lab.run_until(2).await;
    assert_eq!(lab.say_of("sat:b"), vec!["heard yes".to_owned()]);
    assert!(
        lab.assistant.scheduled().is_empty(),
        "B's yes released nothing"
    );

    // A says yes: its own held action runs, exactly once.
    lab.say("sat:a", "yes");
    lab.run_until(3).await;
    assert_eq!(lab.assistant.scheduled().len(), 1);
    assert_eq!(lab.hub.speakers().len(), 2);
}

// REQ-AST-026: a speaker's second request waits for its first; another speaker does not.
#[tokio::test(flavor = "current_thread")]
async fn a_speaker_waits_for_its_own_turn_but_not_for_anyone_elses() {
    let mut lab = Lab::new(500).await;
    lab.say("sat:a", "slow first");
    lab.say("sat:a", "second from a");
    lab.say("sat:b", "quick from b");
    lab.run_until(3).await;
    let order: Vec<&str> = lab.finished.iter().map(|(_, s, _)| s.as_str()).collect();
    assert_eq!(order[0], "sat:b", "b was not held up by a: {order:?}");
    assert_eq!(
        lab.say_of("sat:a"),
        vec![
            "heard slow first".to_owned(),
            "heard second from a".to_owned()
        ],
        "a's requests ran in order"
    );
}

// REQ-AST-027: a built-in tool call changes the loop's state and counts as the model rounds it took.
#[tokio::test(flavor = "current_thread")]
async fn a_built_in_tool_runs_in_the_loop_and_counts_one_request_per_model_round() {
    let mut lab = Lab::new(10).await;
    lab.say("console", r#"call set_timer {"seconds": 600}"#);
    lab.run_until(1).await;
    assert_eq!(
        lab.assistant.scheduled().len(),
        1,
        "the loop owns the timer"
    );
    assert!(lab.say_of("console")[0].starts_with("result: Timer set"));
    assert_eq!(
        *lab.backend.requests.lock().unwrap(),
        2,
        "a call round and an answer round"
    );
    assert_eq!(lab.usage.requests_today(), 2);
}

// REQ-AST-022: cancel still stops a turn that runs in its own task.
#[tokio::test(flavor = "current_thread")]
async fn cancel_stops_a_turn_that_runs_beside_the_loop() {
    let mut lab = Lab::new(5_000).await;
    let ctl = TurnCtl::default();
    let cancel = ctl.cancel.clone();
    let (tx, rx) = oneshot::channel();
    let kit = lab.kit();
    let _ = lab.hub.submit(
        Request {
            speaker: "sat:a".into(),
            line: "slow, very slow".into(),
            ctl,
            origin: Origin::Ask(tx),
        },
        0,
        || kit,
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    cancel.cancel();
    lab.run_until(1).await;
    let said = rx.await.unwrap().say;
    assert!(
        said.to_lowercase().contains("stopped") || said.to_lowercase().contains("cancel"),
        "{said}"
    );
    assert!(
        lab.started.elapsed() < Duration::from_secs(2),
        "it did not wait for the model"
    );
}

// REQ-AST-027, REQ-RTV-009: a realtime session is a task of its own. A timer rings in the middle of it,
// and a built-in tool its model calls is run by the loop.
#[cfg(feature = "realtime")]
#[tokio::test(flavor = "current_thread")]
async fn a_realtime_session_does_not_delay_a_timer_and_its_calls_reach_the_loop() {
    use crate::realtime::ws::accept;
    use crate::rtvoice::{Job, Realtime, RtSettings};
    let mut lab = Lab::new(0).await;
    lab.assistant
        .call_tool("set_timer", &serde_json::json!({"seconds": 2}))
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let (mut ws, _) = accept(tcp).await.unwrap();
        ws.recv().await.unwrap(); // the session setup
        let call = serde_json::json!({"type": "response.done", "response": {
            "status": "completed", "output": [{
                "type": "function_call", "status": "completed", "call_id": "c1",
                "name": "set_timer", "arguments": "{\"seconds\": 600}"}]}});
        ws.send_text(&call.to_string()).await.unwrap();
        ws.recv().await.unwrap(); // the function output
        ws.recv().await.unwrap(); // the request to speak
        tokio::time::sleep(Duration::from_millis(900)).await;
        ws.send_close().await.unwrap();
    });
    let settings = RtSettings {
        enabled: true,
        url: Some(format!("ws://127.0.0.1:{port}/v1/realtime")),
        model: Some("m".into()),
        ..RtSettings::default()
    };
    let kit = crate::turns::RtKit {
        rt: Realtime::new(settings.clone()),
        settings,
        key: Some("sk-test".into()),
        persona: "persona".into(),
        cfg: TurnConfig::default(),
        usage: lab.usage.clone(),
        http: Arc::clone(&lab.http),
        mcp: Arc::clone(&lab.mcp),
        ext: Arc::clone(&lab.ext_view),
        builtin: ToolHost::tool_specs(&lab.assistant),
        day: 1,
        clock: Arc::new(FastClock(Instant::now())),
        conv: Conversation::standard("persona"),
    };
    let (_mic_tx, mic) = mpsc::channel(8);
    let (speaker, _spk_rx) = mpsc::channel(8);
    let (done, done_rx) = oneshot::channel();
    lab.hub.spawn_realtime(
        kit,
        Job {
            speaker_id: "sat:a".into(),
            mic,
            speaker,
            done,
        },
    );
    lab.pump(|l| l.others >= 1).await;
    let ended = lab.started.elapsed();
    assert!(
        ended >= Duration::from_millis(850),
        "the session took its time"
    );
    assert_eq!(lab.fired.len(), 1, "the timer rang during the session");
    assert!(
        lab.fired[0] < Duration::from_millis(500),
        "it rang at {:?}, session ended at {ended:?}",
        lab.fired[0]
    );
    assert_eq!(
        lab.assistant.scheduled().len(),
        1,
        "the call the model made set a timer in the loop"
    );
    let done = done_rx.await.unwrap();
    assert!(!done.failed, "{:?}", done.speak);
    assert_eq!(lab.usage.requests_today(), 1, "the session is one request");
}

// REQ-AST-027: a task that dies hands the speaker a fresh start and does not wedge the loop.
#[tokio::test(flavor = "current_thread")]
async fn a_lost_turn_does_not_wedge_its_speaker() {
    let mut lab = Lab::new(10).await;
    let (tx, _rx) = oneshot::channel();
    let kit = lab.kit();
    let _ = lab.hub.submit(
        Request {
            speaker: "sat:a".into(),
            line: "slow".into(),
            ctl: TurnCtl::default(),
            origin: Origin::Ask(tx),
        },
        0,
        || kit,
    );
    lab.hub.tasks.abort_all();
    match lab.hub.next().await {
        Event::Lost(s) => {
            assert_eq!(s, "sat:a");
            assert!(lab.hub.lost(&s, 0).is_none());
        }
        other => panic!("expected a lost turn, got {other:?}"),
    }
    lab.say("sat:a", "after");
    lab.run_until(1).await;
    assert_eq!(lab.say_of("sat:a"), vec!["heard after".to_owned()]);
}

/// A tool that already ran for a turn that has since died is reported in the log, by name and never with
/// its arguments.
#[tokio::test(flavor = "current_thread")]
async fn a_result_for_a_dead_turn_is_logged_by_tool_name_only() {
    let mut lab = Lab::new(0).await;
    let (reply, answer) = oneshot::channel();
    drop(answer);
    let req = ToolReq {
        name: "set_timer".into(),
        args: serde_json::json!({"seconds": 600, "label": "zq-secret-label"}),
        pin: false,
        reply,
    };
    req.serve(&mut lab.assistant).await;
    let lines = crate::diag::recent(200);
    let hit = lines
        .iter()
        .filter(|l| l.level == "error" && l.text.contains("ended before its result"))
        .filter(|l| l.text.contains("`set_timer`"))
        .count();
    assert!(hit >= 1, "no log line: {lines:?}");
    assert!(
        lines.iter().all(|l| !l.text.contains("zq-secret-label")),
        "arguments were logged"
    );
}

/// REQ-TMR-003: the model (or a realtime session) hears "timer set" only once the timer is on disk, and the
/// loop that serves the call is not held while it waits.
#[tokio::test(flavor = "current_thread")]
async fn a_timer_tool_result_waits_for_the_disk_without_holding_the_loop() {
    use crate::app::{Adapters, Core, Limits, Stores};
    use crate::store::{JsonStore, testutil::TempDir};
    let dir = TempDir::new("tool-durable");
    let clock: Arc<dyn Clock + Send + Sync> = Arc::new(crate::sched::SystemClock);
    let core = Arc::new(
        Core::open(
            clock.clone(),
            Adapters::builtin(),
            Limits::default(),
            Stores::in_dir(&dir.0),
        )
        .unwrap(),
    );
    let store = JsonStore::new(dir.0.join("timers.json"), crate::sched::SCHEMA);
    let mut assistant = Assistant::new(
        clock,
        Box::new(FixedOffset(0)),
        Scheduler::new(8),
        Some(store.clone()),
        Some(Arc::clone(&core)),
    );
    let writer = core.writer().unwrap();
    writer.pause();
    let (reply, mut answer) = oneshot::channel();
    let req = ToolReq {
        name: "set_timer".into(),
        args: serde_json::json!({"seconds": 600, "label": "tea"}),
        pin: false,
        reply,
    };
    req.serve(&mut assistant).await;
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    assert!(answer.try_recv().is_err(), "answered before the save");
    assert!(!store.path().exists());
    writer.resume();
    let a = answer.await.unwrap();
    assert!(a.result.is_ok(), "{:?}", a.result);
    let saved: Scheduler = Scheduler::load(&store, 8, &crate::sched::SystemClock).unwrap();
    assert_eq!(saved.list(&crate::sched::SystemClock).len(), 1);
}

/// REQ-TMR-003: a timer the model set that could not be saved is still set (it rings), and the tool result the
/// model reads says so, so it does not tell the person it is safe. The next call that does save says nothing.
#[tokio::test(flavor = "current_thread")]
async fn a_timer_tool_result_says_when_the_save_failed() {
    use crate::app::{Adapters, Core, Limits, Stores};
    use crate::store::{JsonStore, testutil::TempDir};
    let dir = TempDir::new("tool-unsaved");
    let clock: Arc<dyn Clock + Send + Sync> = Arc::new(crate::sched::SystemClock);
    let core = Arc::new(
        Core::open(
            clock.clone(),
            Adapters::builtin(),
            Limits::default(),
            Stores::in_dir(&dir.0),
        )
        .unwrap(),
    );
    let store = JsonStore::new(dir.0.join("timers.json"), crate::sched::SCHEMA);
    let mut assistant = Assistant::new(
        clock,
        Box::new(FixedOffset(0)),
        Scheduler::new(8),
        Some(store.clone()),
        Some(Arc::clone(&core)),
    );
    let blocker = crate::store::temp_path(store.path());
    std::fs::create_dir(&blocker).unwrap();
    async fn run(assistant: &mut Assistant, args: serde_json::Value) -> ToolAnswer {
        let (reply, answer) = oneshot::channel();
        let req = ToolReq {
            name: "set_timer".into(),
            args,
            pin: false,
            reply,
        };
        req.serve(assistant).await;
        answer.await.unwrap()
    }
    let answer = run(
        &mut assistant,
        serde_json::json!({"seconds": 600, "label": "tea"}),
    )
    .await;
    let text = answer.result.unwrap();
    assert!(
        text.contains(crate::assistant::NOT_SAVED_NOTE),
        "the model was not told: {text}"
    );
    assert_eq!(assistant.scheduled().len(), 1, "it is still set in memory");
    // Storage works again: the next call is reported plainly.
    std::fs::remove_dir(&blocker).unwrap();
    let answer = run(
        &mut assistant,
        serde_json::json!({"seconds": 300, "label": "eggs"}),
    )
    .await;
    let text = answer.result.unwrap();
    assert!(!text.contains("could not be saved"), "{text}");
    assert_eq!(
        Scheduler::load(&store, 8, &crate::sched::SystemClock)
            .unwrap()
            .list(&crate::sched::SystemClock)
            .len(),
        2
    );
}

/// REQ-SEC-011, REQ-INB-023: a turn that runs beside the loop pins a held reply through the loop too. The
/// read-back names the agent, and the yes sends to that event after another one was announced.
#[tokio::test(flavor = "current_thread")]
async fn a_turn_task_binds_a_held_reply_to_its_event_through_the_loop() {
    use crate::app::{Adapters, Core, LOCALHOST_PEER, Limits};
    let mut lab = Lab::new(10).await;
    let clock: Arc<dyn Clock + Send + Sync> = Arc::new(FastClock(Instant::now()));
    let core = Arc::new(Core::in_memory(
        Arc::clone(&clock),
        Adapters::builtin(),
        Limits::default(),
    ));
    lab.assistant = Assistant::new(
        clock,
        Box::new(FixedOffset(0)),
        Scheduler::new(16),
        None,
        Some(Arc::clone(&core)),
    );
    let announce = |name: &str, spoken: &str| {
        let (_, token) = core.create_endpoint(name, "claude-code").unwrap();
        core.set_endpoint_spoken_name(name, spoken).unwrap();
        let hook = serde_json::json!({"session_id":"s","cwd":"/a/p","hook_event_name":"Stop","last_assistant_message":"Done."});
        core.ingest_hook(name, &token, LOCALHOST_PEER, hook.to_string().as_bytes())
            .unwrap();
        let id = core.next_announcement(true, 0).unwrap().event.id;
        core.mark_announced(&[id]);
        token
    };
    let first = announce("build", "Build Server");

    lab.say(
        "console",
        r#"call inbox_reply {"kind":"instruct","text":"add tests","pinned_event_id":99,"pinned_origin":"Your Bank"}"#,
    );
    lab.run_until(1).await;
    let ask = lab.say_of("console")[0].clone();
    assert!(
        ask.contains("I will send this to Build Server, word for word"),
        "{ask}"
    );
    assert!(!ask.contains("Your Bank"), "{ask}");

    let second = announce("mailer", "Mail Robot");
    lab.say("console", "yes");
    lab.run_until(2).await;
    let sent = |name: &str, token: &str| {
        core.bridge_pending(name, token, LOCALHOST_PEER, 0)
            .unwrap()
            .len()
    };
    assert_eq!(
        sent("build", &first),
        1,
        "went to the event it was read back for"
    );
    assert_eq!(sent("mailer", &second), 0);
}
