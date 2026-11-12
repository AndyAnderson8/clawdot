use super::*;
use crate::app::{Adapters, Limits, Stores};
use crate::sched::SystemClock;
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
