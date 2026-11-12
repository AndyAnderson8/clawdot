use super::*;
use crate::app::{Adapters, Fail, Limits, Stores};
use crate::sched::SystemClock;
use crate::session::Conversation;
use crate::startup::build_brain;
use crate::store::testutil::TempDir;
use crate::{cli, seed};

type Map = serde_json::Map<String, serde_json::Value>;

/// A real brain over a scratch data directory, and the live values it started with.
async fn started(dir: &TempDir, extra: &[&str]) -> (Brain, Arc<Core>, Map) {
    let mut argv = vec!["--data-dir".to_owned(), dir.0.display().to_string()];
    argv.extend(extra.iter().map(|s| (*s).to_owned()));
    let mut args = cli::parse(&argv).unwrap();
    let (state, _) = seed::resolve_settings(&mut args, &crate::env::Env::empty()).unwrap();
    let live = state.live_values();
    let clock: Arc<dyn Clock + Send + Sync> = Arc::new(SystemClock);
    let core = Arc::new(
        Core::open(
            clock.clone(),
            Adapters::builtin(),
            Limits::default(),
            Stores::in_dir(&args.data_dir),
        )
        .unwrap(),
    );
    let zone = zone_for(None).unwrap();
    let (brain, _hub) = build_brain(&mut args, &live, zone, &clock, &core)
        .await
        .unwrap();
    (brain, core, live)
}

fn with(live: &Map, changes: &[(&str, serde_json::Value)]) -> Map {
    let mut m = live.clone();
    for (k, v) in changes {
        m.insert((*k).to_owned(), v.clone());
    }
    m
}

// REQ-CFG-039: the agent group applies as a whole and reaches the language model's turn settings.
#[tokio::test]
async fn a_change_to_the_agent_group_reaches_the_brain_and_the_model_settings() {
    let dir = TempDir::new("apply-live-agent");
    let (mut brain, core, live) = started(
        &dir,
        &["--llm-url", "http://127.0.0.1:1/v1", "--llm-model", "m"],
    )
    .await;
    let mut applied = live.clone();
    assert_ne!(brain.llm.as_ref().unwrap().cfg.max_tool_loops, 3);

    let next = with(
        &live,
        &[
            ("agent_max_rounds", 3.into()),
            ("plan_speech", false.into()),
        ],
    );
    apply_live(&mut brain, &core, &next, &mut applied);

    assert_eq!(brain.agent.max_rounds, Some(3));
    assert_eq!(brain.agent.plan_speech, Some(false));
    let cfg = &brain.llm.as_ref().unwrap().cfg;
    assert_eq!(cfg.max_tool_loops, 3);
    assert!(!cfg.plan_speech);
    assert_eq!(applied, next, "what is in effect is remembered");
}

#[tokio::test]
async fn only_what_changed_is_applied() {
    let dir = TempDir::new("apply-live-persona");
    let (mut brain, core, live) = started(&dir, &[]).await;
    let mut applied = live.clone();

    // Nothing changed: a value set behind the loop's back is left alone.
    brain.persona = "set elsewhere".into();
    apply_live(&mut brain, &core, &live, &mut applied);
    assert_eq!(brain.persona, "set elsewhere");

    let next = with(&live, &[("persona", "You are terse.".into())]);
    apply_live(&mut brain, &core, &next, &mut applied);
    assert_eq!(brain.persona, "You are terse.");
    assert!(brain.llm.is_none(), "no model was configured or added");
}

#[tokio::test]
async fn the_daily_spend_cap_and_notes_switch_follow_the_settings() {
    let dir = TempDir::new("apply-live-spend");
    let (mut brain, core, live) = started(&dir, &[]).await;
    let mut applied = live.clone();
    assert_eq!(brain.usage.limits().daily_spend_cap_usd, None);

    let next = with(
        &live,
        &[("daily_spend_usd", 2.5.into()), ("notes", true.into())],
    );
    apply_live(&mut brain, &core, &next, &mut applied);
    assert_eq!(brain.usage.limits().daily_spend_cap_usd, Some(2.5));
    assert!(core.notes().unwrap().enabled());
}

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

// REQ-AST-027, REQ-CFG-033: while the portal's new MCP server is still connecting, the loop rings a timer
// and applies a settings change; the server is added only when its result comes back. The server holds
// its reply until the test opens it, so nothing here depends on a sleep being long enough.
#[tokio::test(flavor = "current_thread")]
async fn a_timer_rings_and_a_setting_applies_while_an_mcp_connect_is_in_flight() {
    let dir = TempDir::new("mcp-in-flight");
    let (mut brain, core, live) = started(&dir, &[]).await;
    let mut applied = live.clone();
    let mut done_rx = brain.connects.attach().unwrap();
    let (open_tx, open) = tokio::sync::watch::channel(false);
    let port = crate::connects::testutil::gated_server(open).await;
    brain
        .assistant
        .call_tool("set_timer", &serde_json::json!({"seconds": 1}))
        .await
        .unwrap();

    let (job, mut added) = ask(add_home(port));
    portal_op(&mut brain, &core, &mut applied, job).await;
    assert!(brain.connects.is_connecting("home"));
    assert!(
        added.try_recv().is_err(),
        "the request waits for the server"
    );

    // The settings change goes through and is answered at once.
    let next = with(&live, &[("persona", "You are terse.".into())]);
    let (job, settings) = ask(Op::Settings(next));
    portal_op(&mut brain, &core, &mut applied, job).await;
    assert!(settings.await.unwrap().is_ok());
    assert_eq!(brain.persona, "You are terse.");

    // The loop's tick rings the timer although the server has not answered.
    let mut tick = tokio::time::interval(Duration::from_millis(20));
    let rang = loop {
        tokio::select! {
            _ = tick.tick() => {
                if !brain.assistant.tick().is_empty() { break true; }
            }
            Some(done) = done_rx.recv() => {
                mcp_connected(&mut brain, &core, done);
                break false;
            }
        }
    };
    assert!(rang, "the timer rang first");
    assert!(brain.connects.is_connecting("home"), "still connecting");
    assert!(!brain.mcp.has_server("home"), "nothing added yet");
    assert!(added.try_recv().is_err());

    // The server answers; the loop applies the result and answers the request.
    open_tx.send_replace(true);
    let done = done_rx.recv().await.unwrap();
    mcp_connected(&mut brain, &core, done);
    let listing = added.await.unwrap().unwrap();
    assert!(brain.mcp.has_server("home") && brain.mcp.owns("home__ping"));
    assert!(!brain.connects.is_connecting("home"));
    assert!(listing.to_string().contains("home__ping"), "{listing}");
    assert_eq!(core.ui_tools(), listing);
}

// Removing a source that is still connecting stops it: the server never appears, not even when the server
// answers later, and the request that asked for it is told. The name can then be added again.
#[tokio::test(flavor = "current_thread")]
async fn removing_a_source_while_it_connects_cancels_it_and_it_can_be_added_again() {
    let dir = TempDir::new("mcp-cancel");
    let (mut brain, core, live) = started(&dir, &[]).await;
    let mut applied = live.clone();
    let mut done_rx = brain.connects.attach().unwrap();
    let (open_tx, open) = tokio::sync::watch::channel(false);
    let port = crate::connects::testutil::gated_server(open).await;

    let (job, first) = ask(add_home(port));
    portal_op(&mut brain, &core, &mut applied, job).await;
    let (job, twin) = ask(add_home(port));
    portal_op(&mut brain, &core, &mut applied, job).await;
    assert!(
        matches!(twin.await.unwrap(), Err(Fail::Invalid(m)) if m.contains("already")),
        "a second connect for the name is refused"
    );

    let (job, removed) = ask(Op::Ext(ExtOp::McpDelete("home".into())));
    portal_op(&mut brain, &core, &mut applied, job).await;
    assert!(removed.await.unwrap().is_ok());
    assert!(matches!(first.await.unwrap(), Err(Fail::Invalid(m)) if m.contains("removed")));
    assert!(!brain.connects.is_connecting("home"));

    // The server answers after the removal: nothing arrives, nothing is added.
    open_tx.send_replace(true);
    let late = tokio::time::timeout(Duration::from_millis(300), done_rx.recv()).await;
    assert!(late.is_err(), "the cancelled task sent nothing");
    assert!(!brain.mcp.has_server("home"));

    // And the name is free again.
    let (job, again) = ask(add_home(port));
    portal_op(&mut brain, &core, &mut applied, job).await;
    let done = done_rx.recv().await.unwrap();
    mcp_connected(&mut brain, &core, done);
    assert!(again.await.unwrap().is_ok());
    assert!(brain.mcp.has_server("home"));
}

/// Save two connections in `dir` the way the portal would: `stuck` (answers only when told to, gives up
/// after 2 s) and `good`.
fn save_sources(dir: &TempDir, stuck_port: u16, good_port: u16) {
    crate::store::JsonStore::new(dir.0.join("connections.json"), crate::extensions::SCHEMA)
        .save(&serde_json::json!({ "servers": [
            { "name": "stuck", "url": format!("http://127.0.0.1:{stuck_port}/mcp"), "timeout_secs": 2 },
            { "name": "good", "url": format!("http://127.0.0.1:{good_port}/mcp") },
        ]}))
        .unwrap();
}

fn source<'a>(listing: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
    listing["mcp"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == name)
        .unwrap_or_else(|| panic!("no source {name} in {listing}"))
}

// ADR-0021, REQ-AST-027: saved sources connect after the loop is running. One that never answers does not
// hold up start-up: the loop rings a timer and answers a portal request while it is still connecting, the
// good source's tools appear as soon as it answers, and the stalled one ends as failed after its own
// timeout, still saved and listed with its reason.
#[tokio::test(flavor = "current_thread")]
async fn a_stalled_saved_source_does_not_hold_up_start_and_ends_failed_after_its_timeout() {
    let dir = TempDir::new("mcp-startup");
    let (_hold, closed) = tokio::sync::watch::channel(false);
    let (_open_tx, open) = tokio::sync::watch::channel(true);
    let stuck = crate::connects::testutil::gated_server(closed).await;
    let good = crate::connects::testutil::gated_server(open).await;
    save_sources(&dir, stuck, good);

    // Building the brain returns at once although one source will never answer.
    let began = std::time::Instant::now();
    let (mut brain, core, live) = started(&dir, &[]).await;
    assert!(
        began.elapsed() < Duration::from_millis(1500),
        "start-up did not wait for the sources"
    );
    let mut applied = live.clone();
    let mut done_rx = brain.connects.attach().unwrap();
    assert!(brain.mcp.is_connecting("stuck") && brain.mcp.is_connecting("good"));
    assert_eq!(brain.connects.len(), 2);
    assert_eq!(
        brain.connects.portal_len(),
        0,
        "saved sources are not portal requests"
    );
    let listing = brain.listing();
    assert_eq!(source(&listing, "stuck")["connecting"], true);
    assert_eq!(source(&listing, "stuck")["connected"], false);

    brain
        .assistant
        .call_tool("set_timer", &serde_json::json!({"seconds": 1}))
        .await
        .unwrap();

    // The loop: a portal request is answered at once, the good source is applied when it reports, and the
    // timer rings, all while the stalled source is still connecting.
    let next = with(&live, &[("persona", "You are terse.".into())]);
    let (job, settings) = ask(Op::Settings(next));
    portal_op(&mut brain, &core, &mut applied, job).await;
    assert!(settings.await.unwrap().is_ok());
    assert_eq!(brain.persona, "You are terse.");

    let mut tick = tokio::time::interval(Duration::from_millis(20));
    let (mut rang, mut good_in) = (false, false);
    while !(rang && good_in) {
        tokio::select! {
            _ = tick.tick() => {
                rang |= !brain.assistant.tick().is_empty();
            }
            Some(done) = done_rx.recv() => {
                assert_eq!(done.pending.name(), "good", "the stalled source cannot report first");
                mcp_connected(&mut brain, &core, done);
                good_in = true;
            }
        }
    }
    assert!(
        brain.mcp.owns("good__ping"),
        "the good source's tools appear"
    );
    let listing = brain.listing();
    assert_eq!(source(&listing, "good")["connected"], true);
    assert_eq!(source(&listing, "good")["connecting"], false);
    assert!(brain.mcp.is_connecting("stuck"), "still connecting");
    assert_eq!(source(&listing, "stuck")["connecting"], true);
    assert_eq!(core.ui_tools(), listing, "the portal's copy is refreshed");

    // Its own timeout ends the stalled one: failed, with the reason, and still saved.
    let done = done_rx.recv().await.unwrap();
    assert_eq!(done.pending.name(), "stuck");
    mcp_connected(&mut brain, &core, done);
    let listing = brain.listing();
    let s = source(&listing, "stuck");
    assert_eq!(s["connected"], false);
    assert_eq!(s["connecting"], false);
    assert!(s["error"].as_str().is_some_and(|e| !e.is_empty()), "{s}");
    assert!(!brain.mcp.owns("stuck__ping") && brain.connects.is_empty());
    assert_eq!(listing["mcp"].as_array().map(Vec::len), Some(2));
    assert_eq!(
        brain
            .ext
            .snapshot()
            .listing(&[], &brain.http_tools, &brain.mcp)["mcp"]
            .as_array()
            .map(Vec::len),
        Some(2),
        "both stay saved"
    );
}

// Removing a saved source that is still connecting removes it and stops its connection; its late result
// is not applied.
#[tokio::test(flavor = "current_thread")]
async fn removing_a_saved_source_while_it_connects_stops_it() {
    let dir = TempDir::new("mcp-startup-remove");
    let (open_tx, closed) = tokio::sync::watch::channel(false);
    let (_g, open) = tokio::sync::watch::channel(true);
    let stuck = crate::connects::testutil::gated_server(closed).await;
    let good = crate::connects::testutil::gated_server(open).await;
    save_sources(&dir, stuck, good);
    let (mut brain, core, live) = started(&dir, &[]).await;
    let mut applied = live.clone();
    let mut done_rx = brain.connects.attach().unwrap();
    assert!(brain.connects.is_connecting("stuck"));

    let (job, removed) = ask(Op::Ext(ExtOp::McpDelete("stuck".into())));
    portal_op(&mut brain, &core, &mut applied, job).await;
    assert!(removed.await.unwrap().is_ok());
    assert!(!brain.connects.is_connecting("stuck") && !brain.mcp.has_server("stuck"));

    // Even if the server answers now, nothing arrives for the removed name.
    open_tx.send_replace(true);
    let done = done_rx.recv().await.unwrap();
    assert_eq!(done.pending.name(), "good");
    mcp_connected(&mut brain, &core, done);
    assert!(
        tokio::time::timeout(Duration::from_millis(300), done_rx.recv())
            .await
            .is_err()
    );
    assert!(!brain.mcp.has_server("stuck") && brain.mcp.owns("good__ping"));
}

// A connection task that dies is answered like any other failed connection, and the name can be tried
// again.
#[tokio::test(flavor = "current_thread")]
async fn a_connect_task_that_dies_answers_the_request_and_frees_the_name() {
    let dir = TempDir::new("mcp-died");
    let (mut brain, core, live) = started(&dir, &[]).await;
    let mut applied = live.clone();
    let mut done_rx = brain.connects.attach().unwrap();
    brain.connects.make_next_task_die();
    let (job, answer) = ask(add_home(9));
    portal_op(&mut brain, &core, &mut applied, job).await;
    let done = done_rx.recv().await.unwrap();
    mcp_connected(&mut brain, &core, done);
    assert!(matches!(
        answer.await.unwrap(),
        Err(Fail::Invalid(m)) if m == "The connection could not be made."
    ));
    assert!(!brain.mcp.has_server("home") && brain.connects.is_empty());
    let (job, _again) = ask(add_home(9));
    portal_op(&mut brain, &core, &mut applied, job).await;
    assert!(brain.connects.is_connecting("home"));
}

// A server that cannot be used is reported to the request and leaves nothing behind.
#[tokio::test(flavor = "current_thread")]
async fn a_connect_that_fails_answers_the_request_and_adds_nothing() {
    let dir = TempDir::new("mcp-fail");
    let (mut brain, core, live) = started(&dir, &[]).await;
    let mut applied = live.clone();
    let mut done_rx = brain.connects.attach().unwrap();
    let (job, answer) = ask(add_home(9));
    portal_op(&mut brain, &core, &mut applied, job).await;
    let done = done_rx.recv().await.unwrap();
    mcp_connected(&mut brain, &core, done);
    assert!(matches!(answer.await.unwrap(), Err(Fail::Invalid(_))));
    assert!(!brain.mcp.has_server("home") && brain.connects.is_empty());
    // A bad request is refused on the spot, without a task.
    let (job, bad) = ask(Op::Ext(ExtOp::McpAdd(
        serde_json::json!({"name": "Bad Name", "url": "http://h/mcp"}),
    )));
    portal_op(&mut brain, &core, &mut applied, job).await;
    assert!(matches!(bad.await.unwrap(), Err(Fail::Invalid(_))));
    assert!(brain.connects.is_empty());
}

#[tokio::test]
async fn the_status_snapshot_reports_the_brain_without_secrets() {
    let dir = TempDir::new("status");
    let (brain, _core, _live) = started(&dir, &[]).await;
    let view = SpeakerView {
        available: false,
        console: true,
        quiet: None,
    };
    let s = brain.status_json(0, &view, &["Jazz".to_owned()], &serde_json::json!({}));
    assert_eq!(s["llm"]["configured"], false);
    assert_eq!(s["speaker"]["name"], "console");
    assert_eq!(s["speaker"]["available"], true);
    assert_eq!(s["stations"][0], "Jazz");
    assert_eq!(s["satellites"], 0);
    assert_eq!(s["version"], env!("CARGO_PKG_VERSION"));
}

// REQ-CFG-083
#[tokio::test]
async fn the_status_names_the_device_only_from_the_configured_preset() {
    let view = SpeakerView {
        available: false,
        console: true,
        quiet: None,
    };
    let none = TempDir::new("status-hw-none");
    let (brain, _core, _live) = started(&none, &[]).await;
    let hw = brain.status_json(0, &view, &[], &serde_json::json!({}))["hardware"].clone();
    assert_eq!(hw["board"], "unknown");
    assert_eq!(hw["label"], "");
    assert_eq!(hw["configured_parts"], serde_json::json!([]));

    let dir = TempDir::new("status-hw-set");
    let file = dir.0.join("c.json");
    std::fs::write(
        &file,
        r#"{"device": {"board": "my-board", "label": "Den speaker", "parts": {"leds": true}}}"#,
    )
    .unwrap();
    let (brain, _core, _live) = started(&dir, &["--config", &file.display().to_string()]).await;
    let hw = brain.status_json(0, &view, &[], &serde_json::json!({}))["hardware"].clone();
    assert_eq!(hw["board"], "my-board");
    assert_eq!(hw["label"], "Den speaker");
    assert_eq!(hw["parts"]["leds"], true);
    assert_eq!(hw["configured_parts"], serde_json::json!(["leds"]));
}

// REQ-AST-011: the model prices follow the settings (unset stays unset, 0 stays 0) and the status says
// what the day's spend rests on.
#[tokio::test]
async fn model_prices_follow_the_settings_and_the_status_names_the_basis_of_the_spend() {
    let dir = TempDir::new("apply-live-prices");
    let (mut brain, core, live) = started(&dir, &[]).await;
    let mut applied = live.clone();
    let view = SpeakerView {
        available: false,
        console: true,
        quiet: None,
    };
    let status = |b: &Brain| b.status_json(0, &view, &[], &serde_json::json!({}))["usage"].clone();
    let u = status(&brain);
    assert_eq!(u["spend_basis"], "none");
    assert!(u["llm_usd_per_mtok_in"].is_null() && u["llm_usd_per_mtok_out"].is_null());

    // Nothing is known about the provider: the day's spend is an estimate.
    brain.usage.record_sized(7, None, 3000, 300);
    assert_eq!(status(&brain)["spend_basis"], "estimated");

    let next = with(
        &live,
        &[
            ("llm_usd_per_mtok_in", 0.into()),
            ("llm_usd_per_mtok_out", 2.5.into()),
        ],
    );
    apply_live(&mut brain, &core, &next, &mut applied);
    let l = brain.usage.limits();
    assert_eq!(
        (l.llm_usd_per_mtok_in, l.llm_usd_per_mtok_out),
        (Some(0.0), Some(2.5))
    );
    let u = status(&brain);
    assert_eq!(u["llm_usd_per_mtok_in"], 0.0);
    assert_eq!(u["llm_usd_per_mtok_out"], 2.5);

    // Clearing a price puts it back to unset, not to 0.
    let mut cleared = next.clone();
    cleared.remove("llm_usd_per_mtok_in");
    apply_live(&mut brain, &core, &cleared, &mut applied);
    assert_eq!(brain.usage.limits().llm_usd_per_mtok_in, None);
}

#[tokio::test]
async fn the_fast_path_answers_without_a_model_and_other_requests_say_one_is_needed() {
    let dir = TempDir::new("dispatch");
    let (mut brain, _core, _live) = started(&dir, &[]).await;
    let mut hub = Hub::new(|| Conversation::standard(String::new()));
    let ask = |line: &str| Request {
        speaker: "console".into(),
        line: line.to_owned(),
        ctl: TurnCtl::default(),
        origin: Origin::Console,
    };
    let (_, reply) = dispatch(&mut brain, &mut hub, ask("set a timer for 5 minutes")).unwrap();
    assert!(
        !reply.say.is_empty() && reply.say != NO_LLM,
        "{}",
        reply.say
    );
    let (_, reply) = dispatch(
        &mut brain,
        &mut hub,
        ask("tell me about owls and their habits"),
    )
    .unwrap();
    assert_eq!(reply.say, NO_LLM);
}

fn fast_request(line: &str, origin: Origin) -> Request {
    Request {
        speaker: "console".into(),
        line: line.to_owned(),
        ctl: TurnCtl::default(),
        origin,
    }
}

fn timers_on_disk(dir: &TempDir) -> usize {
    let store = JsonStore::new(dir.0.join("timers.json"), crate::sched::SCHEMA);
    crate::sched::Scheduler::load(&store, 64, &crate::sched::SystemClock)
        .unwrap()
        .list(&crate::sched::SystemClock)
        .len()
}

// REQ-TMR-003: a timer the user was told is set survives a crash right after the confirmation. The reply is
// held back until the write lands, and the loop is not held with it.
#[tokio::test(flavor = "current_thread")]
async fn the_confirmation_of_a_timer_is_not_delivered_before_it_is_on_disk() {
    let dir = TempDir::new("timer-durable");
    let (mut brain, core, _live) = started(&dir, &[]).await;
    let mut hub = Hub::new(|| Conversation::standard(String::new()));
    let writer = core.writer().unwrap();
    writer.pause();
    let (tx, mut rx) = tokio::sync::oneshot::channel();
    let (origin, reply) = dispatch(
        &mut brain,
        &mut hub,
        fast_request("set a timer for 5 minutes", Origin::Ask(tx)),
    )
    .unwrap();
    assert!(reply.say.starts_with("Timer set"), "{}", reply.say);
    deliver_confirmed(&brain, origin, reply);
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    assert!(rx.try_recv().is_err(), "spoken before it was saved");
    assert_eq!(timers_on_disk(&dir), 0);
    writer.resume();
    let said = rx.await.unwrap();
    assert!(said.say.starts_with("Timer set"));
    // The confirmation is out and nothing has been flushed by hand: it is already on disk.
    assert_eq!(timers_on_disk(&dir), 1);
}

// REQ-TMR-003: a timer whose save failed is still set (it will ring), and the confirmation says so in one short
// sentence instead of claiming it is safe. The next change that does save is confirmed plainly again, and a reply
// that changed nothing never repeats the bad news.
#[tokio::test(flavor = "current_thread")]
async fn a_timer_that_could_not_be_saved_is_confirmed_honestly() {
    let dir = TempDir::new("timer-unsaved");
    let (mut brain, _core, _live) = started(&dir, &[]).await;
    let mut hub = Hub::new(|| Conversation::standard(String::new()));
    // A directory where the temporary file would be written makes every save fail.
    let blocker = crate::store::temp_path(&dir.0.join("timers.json"));
    std::fs::create_dir_all(&blocker).unwrap();
    async fn said(brain: &mut Brain, hub: &mut Hub, line: &str) -> String {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let (origin, reply) = dispatch(brain, hub, fast_request(line, Origin::Ask(tx))).unwrap();
        deliver_confirmed(brain, origin, reply);
        rx.await.unwrap().say
    }
    let first = said(&mut brain, &mut hub, "set a timer for 10 minutes").await;
    assert_eq!(
        first,
        "Timer set for 10 minutes, but I could not save it, so it will be lost if I restart.",
        "{first}"
    );
    let unrelated = said(&mut brain, &mut hub, "what time is it").await;
    assert!(!unrelated.contains("save"), "{unrelated}");
    assert_eq!(brain.assistant.scheduled().len(), 1, "it is still set");
    std::fs::remove_dir(&blocker).unwrap();
    let later = said(&mut brain, &mut hub, "set a timer for 5 minutes").await;
    assert!(
        later.starts_with("Timer set") && !later.contains("save"),
        "{later}"
    );
    assert_eq!(timers_on_disk(&dir), 2, "the later save carried both");
}

// The page is not told a timer it added is saved when the save failed: the answer is an error (500), though the
// timer is set. Once saving works again the same request succeeds.
#[tokio::test(flavor = "current_thread")]
async fn a_portal_timer_that_could_not_be_saved_is_an_error_to_the_page() {
    let dir = TempDir::new("portal-unsaved");
    let (mut brain, core, live) = started(&dir, &[]).await;
    let mut applied = live.clone();
    let blocker = crate::store::temp_path(&dir.0.join("timers.json"));
    std::fs::create_dir_all(&blocker).unwrap();
    let body = serde_json::json!({"kind": "timer", "seconds": 60, "label": "tea"});
    let (job, answer) = ask(Op::Sched(crate::app::SchedOp::Add(body.clone())));
    portal_op(&mut brain, &core, &mut applied, job).await;
    assert!(
        matches!(answer.await.unwrap(), Err(Fail::NotSaved(_, false))),
        "the page was told it was saved"
    );
    assert_eq!(
        brain.assistant.scheduled().len(),
        1,
        "it is set all the same"
    );
    std::fs::remove_dir(&blocker).unwrap();
    let (job, answer) = ask(Op::Sched(crate::app::SchedOp::Add(body)));
    portal_op(&mut brain, &core, &mut applied, job).await;
    assert!(answer.await.unwrap().is_ok());
    assert_eq!(timers_on_disk(&dir), 2);
}

// A reply that announces nothing new is not held back by a write that is still queued.
#[tokio::test(flavor = "current_thread")]
async fn a_reply_with_no_pending_timer_change_is_delivered_at_once() {
    let dir = TempDir::new("timer-no-wait");
    let (mut brain, core, _live) = started(&dir, &[]).await;
    let mut hub = Hub::new(|| Conversation::standard(String::new()));
    let writer = core.writer().unwrap();
    writer.pause();
    let (tx, mut rx) = tokio::sync::oneshot::channel();
    let (origin, reply) = dispatch(
        &mut brain,
        &mut hub,
        fast_request("what time is it", Origin::Ask(tx)),
    )
    .unwrap();
    deliver_confirmed(&brain, origin, reply);
    assert!(rx.try_recv().is_ok());
    writer.resume();
}

// Rapid changes coalesce, and the file never goes back to an older state.
#[tokio::test(flavor = "current_thread")]
async fn rapid_timer_changes_leave_the_newest_state_on_disk() {
    let dir = TempDir::new("timer-rapid");
    let (mut brain, core, _live) = started(&dir, &[]).await;
    let mut hub = Hub::new(|| Conversation::standard(String::new()));
    for n in 1..=5 {
        let line = format!("set a timer for {n} minutes");
        dispatch(&mut brain, &mut hub, fast_request(&line, Origin::Console)).unwrap();
    }
    dispatch(
        &mut brain,
        &mut hub,
        fast_request("cancel the timer", Origin::Console),
    )
    .unwrap();
    for n in 6..=7 {
        let line = format!("set a timer for {n} minutes");
        dispatch(&mut brain, &mut hub, fast_request(&line, Origin::Console)).unwrap();
    }
    let expected = brain.assistant.scheduled().len();
    core.flush_blocking();
    assert_eq!(timers_on_disk(&dir), expected);
    assert_eq!(
        expected, 2,
        "the cancel emptied the list before the last two"
    );
}

// The counters are handed to the writer, not written by the loop, and a burst of saves leaves the newest.
#[tokio::test(flavor = "current_thread")]
async fn usage_saves_go_to_the_writer_and_the_last_one_wins() {
    let dir = TempDir::new("usage-queued");
    let (brain, core, _live) = started(&dir, &[]).await;
    let writer = core.writer().unwrap();
    let path = dir.0.join("usage.json");
    writer.pause();
    for _ in 0..5 {
        brain.usage.record_request(7);
        brain.save_usage();
    }
    assert!(!path.exists(), "the loop must not write the file itself");
    writer.resume();
    core.flush_blocking();
    let back = UsageTracker::load(&brain.usage_store, brain.usage.limits()).unwrap();
    assert_eq!(back.requests_today(), 5);
}
