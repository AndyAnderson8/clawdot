use super::*;
use crate::store::testutil::TempDir;

fn env_with(vars: &'static [&'static str]) -> EnvFn {
    Box::new(move |n| vars.contains(&n).then(|| "secret-value".to_owned()))
}

fn layer(pairs: &[(&str, Value)]) -> Layer {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), v.clone()))
        .collect()
}

fn defaults() -> Layer {
    layer(&[
        ("device_name", json!("Kitchen")),
        ("zone", json!("UTC")),
        ("listen", json!(DEFAULT_LISTEN)),
        ("llm_key_env", json!("OPENROUTER_API_KEY")),
    ])
}

fn open_in(dir: &Path, file: Layer, flags: Layer) -> SettingsState {
    SettingsState::open(
        Some(dir),
        Sources {
            defaults: defaults(),
            file,
            flags,
        },
        false,
        env_with(&["MY_KEY"]),
    )
    .unwrap()
    .0
}

fn source(st: &SettingsState, key: &str) -> String {
    st.json()["fields"][key]["source"]
        .as_str()
        .unwrap()
        .to_owned()
}

// REQ-CFG-037
#[test]
fn every_row_has_a_unique_key_and_a_default_its_own_check_accepts() {
    let mut seen = std::collections::BTreeSet::new();
    for f in FIELDS {
        assert!(seen.insert(f.key), "duplicate row {}", f.key);
        if let Some(d) = f.default.value() {
            let lk = Lookup {
                env: &|_| Some("x".to_owned()),
                secrets: &Secrets::none(),
            };
            let got = normalize(f.key, &d, &lk);
            assert_eq!(
                got.as_ref(),
                Ok(&d),
                "the default of {} must be valid",
                f.key
            );
        }
    }
    for k in RETIRED {
        assert!(field(k).is_none(), "retired key {k} is still a row");
    }
    assert!(field("nonsense").is_none() && is_live("nonsense").is_none());
}

// The sets below are what the hand-written lists said before the table; a change here is deliberate.
#[test]
fn the_table_keeps_the_old_live_restart_default_and_group_answers() {
    let restart: Vec<_> = FIELDS.iter().filter(|f| !f.live).map(|f| f.key).collect();
    assert_eq!(restart, ["speech_provider", "listen", "public_url"]);
    assert_eq!(FIELDS.len(), 36);
    let mut defaults: Vec<_> = built_in_defaults().into_keys().collect();
    defaults.sort();
    let mut want = vec![
        "device_name",
        "zone",
        "persona",
        "listen",
        "llm_key_env",
        "speech_provider",
        "agent_max_rounds",
        "agent_max_tool_calls",
        "agent_deadline_secs",
        "agent_tool_timeout_secs",
        "plan_speech",
        "progress_speech",
        "notes",
        "update_check",
        "max_actions_per_day",
        "max_costly_per_day",
        "dry_run",
        "realtime_enabled",
        "realtime_voice",
        "realtime_key_env",
        "realtime_transcription_model",
        "realtime_usd_per_min_in",
        "realtime_usd_per_min_out",
        "realtime_max_session_secs",
    ];
    want.sort_unstable();
    assert_eq!(defaults, want);
    let members = |g| -> Vec<_> {
        FIELDS
            .iter()
            .filter(|f| f.group == g)
            .map(|f| f.key)
            .collect()
    };
    assert_eq!(
        members(Group::Agent),
        [
            "agent_max_rounds",
            "agent_max_tool_calls",
            "agent_deadline_secs",
            "agent_tool_timeout_secs",
            "plan_speech",
            "progress_speech"
        ]
    );
    assert_eq!(
        members(Group::Policy),
        ["max_actions_per_day", "max_costly_per_day", "dry_run"]
    );
    assert_eq!(members(Group::Llm), ["llm_url", "llm_model", "llm_key_env"]);
    assert_eq!(members(Group::Speech), ["voice", "speech_language"]);
    assert_eq!(members(Group::Quiet), ["quiet_start", "quiet_end"]);
    let realtime = members(Group::Realtime);
    assert!(realtime.len() == 9 && realtime.iter().all(|k| k.starts_with("realtime_")));
    assert!(
        FIELDS
            .iter()
            .filter(|f| f.key.starts_with("realtime_"))
            .all(|f| f.group == Group::Realtime)
    );
}

#[test]
fn a_changed_group_member_is_noticed_and_an_unrelated_change_is_not() {
    let before = json!({"agent_max_rounds": 5, "zone": "UTC"});
    let mut now = before.clone();
    let (b, n) = (before.as_object().unwrap(), now.as_object_mut().unwrap());
    n.insert("zone".into(), json!("Europe/Berlin"));
    assert!(!group_changed(Group::Agent, n, b));
    n.insert("agent_max_rounds".into(), json!(6));
    assert!(group_changed(Group::Agent, n, b));
}

// REQ-CFG-038
// Uses named time zones, which only a build with the time zone database accepts.
#[cfg(feature = "tz")]
#[test]
fn layers_apply_defaults_then_file_then_saved_then_flags() {
    let dir = TempDir::new("layers");
    let file = layer(&[
        ("zone", json!("America/Chicago")),
        ("device_name", json!("File name")),
    ]);
    let flags = layer(&[("device_name", json!("Flag name"))]);
    let mut st = open_in(&dir.0, file, flags);
    assert_eq!(
        (st.get("zone"), source(&st, "zone")),
        (json!("America/Chicago"), "file".into())
    );
    assert_eq!(source(&st, "public_url"), "default");
    st.change(&json!({"zone": "Europe/Berlin", "public_url": "http://dot.local:3335"}))
        .unwrap();
    assert_eq!(
        (st.get("zone"), source(&st, "zone")),
        (json!("Europe/Berlin"), "saved".into())
    );
    assert_eq!(
        (st.get("public_url"), source(&st, "public_url")),
        (json!("http://dot.local:3335"), "saved".into())
    );
    // A flag beats a saved value, and the page is told the saved value is ignored.
    st.change(&json!({"device_name": "Saved name"})).unwrap();
    let j = st.json();
    assert_eq!(j["fields"]["device_name"]["value"], "Flag name");
    assert_eq!(j["fields"]["device_name"]["source"], "flag");
    assert_eq!(j["fields"]["device_name"]["ignored"], true);
    // Clearing a saved value falls back to the layer below.
    st.change(&json!({"zone": null})).unwrap();
    assert_eq!(
        (st.get("zone"), source(&st, "zone")),
        (json!("America/Chicago"), "file".into())
    );
    st.change(&json!({"public_url": ""})).unwrap();
    assert_eq!(source(&st, "public_url"), "default");
}

// A file saved before the request cap was removed still starts, and the old value is dropped.
// Uses named time zones, which only a build with the time zone database accepts.
#[cfg(feature = "tz")]
#[test]
fn a_retired_setting_in_an_old_file_is_skipped() {
    let dir = TempDir::new("retired");
    std::fs::write(
        dir.0.join("settings.json"),
        r#"{"schema":1,"data":{"max_requests_per_day":40,"board":"auto","zone":"Europe/Berlin"}}"#,
    )
    .unwrap();
    let mut st = open_in(&dir.0, Layer::new(), Layer::new());
    assert_eq!(st.get("zone"), "Europe/Berlin");
    assert!(field("board").is_none(), "the board setting is gone");
    // An export taken from an older version can still be sent back as it is.
    st.change(&json!({"board": "none", "device_name": "Den"}))
        .unwrap();
    assert_eq!(st.get("device_name"), "Den");
    assert!(
        !st.json()["fields"]
            .as_object()
            .unwrap()
            .contains_key("board")
    );
}

// REQ-CFG-038
// Uses named time zones, which only a build with the time zone database accepts.
#[cfg(feature = "tz")]
#[test]
fn saved_settings_survive_a_restart() {
    let dir = TempDir::new("persist");
    let mut st = open_in(&dir.0, Layer::new(), Layer::new());
    st.change(&json!({"zone": "Europe/Berlin", "persona": "Be brief.\nBe kind."}))
        .unwrap();
    let st2 = open_in(&dir.0, Layer::new(), Layer::new());
    assert_eq!(st2.get("zone"), "Europe/Berlin");
    assert_eq!(st2.get("persona"), "Be brief.\nBe kind.");
    assert!(dir.0.join("settings.json").exists());
    assert!(!dir.0.join("settings.json.tmp").exists());
}

// REQ-CFG-038, REQ-SEC-009
#[cfg(unix)]
#[test]
fn the_settings_file_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new("perm");
    let mut st = open_in(&dir.0, Layer::new(), Layer::new());
    st.change(&json!({"device_name": "Den"})).unwrap();
    let mode = std::fs::metadata(dir.0.join("settings.json"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}

// REQ-CFG-038
#[test]
fn a_damaged_unknown_or_newer_file_stops_startup_with_a_clear_message() {
    let open = |dir: &Path| {
        SettingsState::open(
            Some(dir),
            Sources {
                defaults: defaults(),
                ..Sources::default()
            },
            false,
            env_with(&[]),
        )
    };
    let dir = TempDir::new("bad");
    let path = dir.0.join("settings.json");
    std::fs::write(&path, "not json").unwrap();
    assert!(open(&dir.0).unwrap_err().contains("settings.json"));
    std::fs::write(&path, r#"{"schema":1,"data":{"zoen":"x"}}"#).unwrap();
    let e = open(&dir.0).unwrap_err();
    assert!(e.contains("unknown setting `zoen`"), "{e}");
    std::fs::write(&path, r#"{"schema":9,"data":{}}"#).unwrap();
    assert!(open(&dir.0).unwrap_err().contains("upgrade"));
    // An invalid saved value does not stop a headless device; it is skipped with a warning.
    std::fs::write(
        &path,
        r#"{"schema":1,"data":{"zone":"Mars/Base","device_name":"Den"}}"#,
    )
    .unwrap();
    let (st, warnings) = open(&dir.0).unwrap();
    assert_eq!(st.get("zone"), "UTC");
    assert_eq!(st.get("device_name"), "Den");
    assert!(warnings[0].contains("zone"), "{warnings:?}");
}

// REQ-CFG-040
// REQ-AST-011: the prices are live, empty by default, and an explicit 0 is kept (free), not dropped.
#[test]
fn model_prices_are_unset_by_default_zero_is_kept_and_the_range_is_enforced() {
    let dir = TempDir::new("model-prices");
    let mut st = open_in(&dir.0, Layer::new(), Layer::new());
    for k in ["llm_usd_per_mtok_in", "llm_usd_per_mtok_out"] {
        assert_eq!(is_live(k), Some(true), "{k} applies without a restart");
        assert!(st.live_values()[k].is_null(), "{k} starts unset");
        assert!(!built_in_defaults().contains_key(k));
    }
    st.change(&json!({"llm_usd_per_mtok_in": 0, "llm_usd_per_mtok_out": 2.5}))
        .unwrap();
    let live = st.live_values();
    assert_eq!(live["llm_usd_per_mtok_in"].as_f64(), Some(0.0));
    assert_eq!(live["llm_usd_per_mtok_out"].as_f64(), Some(2.5));
    for (patch, needle) in [
        (json!({"llm_usd_per_mtok_in": -1}), "0 to 1000"),
        (json!({"llm_usd_per_mtok_out": 1000.5}), "0 to 1000"),
        (json!({"llm_usd_per_mtok_out": "free"}), "0 to 1000"),
    ] {
        let e = st.change(&patch).unwrap_err();
        assert!(e.contains(needle), "{patch}: {e}");
    }
    st.change(&json!({"llm_usd_per_mtok_in": null})).unwrap();
    assert!(st.live_values()["llm_usd_per_mtok_in"].is_null());
}

// REQ-CFG-040
// Uses named time zones, which only a build with the time zone database accepts.
#[cfg(feature = "tz")]
#[test]
fn bad_values_are_refused_with_what_to_fix_and_nothing_changes() {
    let dir = TempDir::new("validate");
    let mut st = open_in(&dir.0, Layer::new(), Layer::new());
    for (patch, needle) in [
        (json!({"zone": "Mars/Base"}), "time zone"),
        (json!({"zone": 5}), "text"),
        (json!({"listen": "everywhere"}), "127.0.0.1:3335"),
        (json!({"public_url": "dot.local"}), "http://"),
        (json!({"daily_spend_usd": -1}), "zero or more"),
        (json!({"device_name": "a\nb"}), "control"),
        (json!({"llm_url": "ftp://x"}), "https://"),
        (
            json!({"llm_url": "http://example.com/v1", "llm_model": "m"}),
            "unencrypted",
        ),
        (json!({"llm_url": "https://x.example/v1"}), "both"),
        (json!({"llm_key_env": "sk-abc def"}), "NAME"),
        (
            json!({"llm_key_env": "NOT_SET_ANYWHERE"}),
            "NOT_SET_ANYWHERE is not set",
        ),
        (json!({"voice": "alloy"}), "speech provider"),
        (json!({"nonsense": 1}), "not a setting"),
        (json!({}), "JSON object"),
        (
            json!({"zone": "Europe/Berlin", "listen": "bad"}),
            "127.0.0.1",
        ),
    ] {
        let e = st.change(&patch).unwrap_err();
        assert!(e.contains(needle), "{patch}: {e}");
    }
    assert!(
        !dir.0.join("settings.json").exists(),
        "a rejected change must save nothing"
    );
    assert_eq!(
        st.get("zone"),
        "UTC",
        "the valid half of a rejected change is not applied"
    );
}

// REQ-CFG-040
// Uses named time zones, which only a build with the time zone database accepts.
#[cfg(feature = "tz")]
#[test]
fn a_valid_model_group_is_accepted_together() {
    let dir = TempDir::new("llm");
    let mut st = open_in(&dir.0, Layer::new(), Layer::new());
    st.change(&json!({
        "llm_url": "https://openrouter.ai/api/v1", "llm_model": "a/b",
        "llm_key_env": "MY_KEY"
    }))
    .unwrap();
    let j = st.json();
    assert_eq!(j["llm"]["configured"], true);
    assert_eq!(j["llm"]["key_present"], true);
    // Clearing the address alone would orphan the model.
    assert!(
        st.change(&json!({"llm_url": null}))
            .unwrap_err()
            .contains("both")
    );
    st.change(&json!({"llm_url": null, "llm_model": null}))
        .unwrap();
    assert_eq!(st.json()["llm"]["configured"], false);
}

// REQ-CFG-039
// Uses named time zones, which only a build with the time zone database accepts.
#[cfg(feature = "tz")]
#[test]
fn live_settings_are_listed_for_the_assistant_and_restart_ones_are_reported() {
    let dir = TempDir::new("restart");
    let mut st = open_in(&dir.0, Layer::new(), Layer::new());
    assert_eq!(st.json()["restart_needed"], json!([]));
    st.change(&json!({"zone": "Europe/Berlin", "listen": "0.0.0.0:4000"}))
        .unwrap();
    let j = st.json();
    assert_eq!(j["restart_needed"], json!(["listen", "public_url"]));
    assert_eq!(j["fields"]["zone"]["live"], true);
    assert_eq!(j["fields"]["listen"]["live"], false);
    let live = st.live_values();
    assert_eq!(live["zone"], "Europe/Berlin");
    assert!(!live.contains_key("listen"));
    // Going back to the running value needs no restart.
    st.change(&json!({"listen": null})).unwrap();
    assert_eq!(st.json()["restart_needed"], json!([]));
}

// REQ-CFG-016
#[test]
fn the_public_address_follows_the_listen_address_until_set() {
    let mut st = SettingsState::in_memory(defaults(), false, env_with(&[]));
    assert_eq!(st.get("public_url"), "http://127.0.0.1:3335");
    st.change(&json!({"listen": "0.0.0.0:4000"})).unwrap();
    assert_eq!(st.get("public_url"), "http://clawdot.local:4000");
    // With the machine's name known, the derived address uses it, and that is not a change needing a restart.
    st.set_machine_name(Some("Speaker.Example.Lan"));
    assert_eq!(st.get("public_url"), "http://speaker.local:4000");
    assert_eq!(st.json()["restart_needed"], json!([]));
    st.change(&json!({"public_url": "http://dot.local:4000/"}))
        .unwrap();
    assert_eq!(st.get("public_url"), "http://dot.local:4000");
}

// REQ-CFG-042
#[test]
fn no_secret_value_is_ever_in_the_output_or_the_file() {
    let dir = TempDir::new("secret");
    let mut st = open_in(&dir.0, Layer::new(), Layer::new());
    st.change(&json!({"llm_key_env": "MY_KEY"})).unwrap();
    let out = st.json().to_string();
    let file = std::fs::read_to_string(dir.0.join("settings.json")).unwrap();
    for s in [&out, &file] {
        assert!(s.contains("MY_KEY") && !s.contains("secret-value"), "{s}");
    }
}

// REQ-CFG-042
#[test]
fn a_stored_key_cannot_be_smuggled_in_as_an_unknown_setting() {
    let mut st = SettingsState::in_memory(defaults(), false, env_with(&[]));
    for k in ["llm_key", "api_key", "key"] {
        assert!(st.change(&json!({ (k): "sk-1234" })).is_err());
    }
}

// REQ-VOI-021
#[test]
fn the_speech_provider_is_a_known_name_that_needs_a_restart_and_frees_the_voice() {
    let mut st = SettingsState::in_memory(defaults(), false, env_with(&[]));
    // A voice needs a speech section for the OpenAI style...
    let e = st.change(&json!({"voice": "alloy"})).unwrap_err();
    assert!(e.contains("speech provider"), "{e}");
    let e = st.change(&json!({"speech_provider": "acme"})).unwrap_err();
    assert!(e.contains("openai, elevenlabs, deepgram"), "{e}");
    // ...but ElevenLabs and Deepgram bring their own address.
    st.change(&json!({"speech_provider": " ElevenLabs ", "voice": "21m00Tcm4TlvDq8ikWAM"}))
        .unwrap();
    assert_eq!(st.get("speech_provider"), "elevenlabs");
    let j = st.json();
    assert_eq!(j["fields"]["speech_provider"]["live"], false);
    assert!(
        j["restart_needed"]
            .as_array()
            .unwrap()
            .contains(&json!("speech_provider"))
    );
    st.change(&json!({"speech_provider": null})).unwrap_err();
}

// REQ-CFG-062, REQ-CFG-038
#[test]
fn agent_settings_are_layered_validated_and_live() {
    let dir = TempDir::new("agent");
    let file = layer(&[("agent_max_rounds", json!(6)), ("notes", json!(true))]);
    let flags = layer(&[("dry_run", json!(true))]);
    let mut st = open_in(&dir.0, file, flags);
    assert_eq!(source(&st, "agent_max_rounds"), "file");
    st.change(&json!({
        "agent_max_rounds": 10, "agent_max_tool_calls": 3, "agent_deadline_secs": 30,
        "agent_tool_timeout_secs": 20, "plan_speech": false, "progress_speech": false,
        "max_actions_per_day": 0, "max_costly_per_day": 5, "quiet_start": "22:00", "quiet_end": "07:30",
    }))
    .unwrap();
    let live = st.live_values();
    for (k, v) in [
        ("agent_max_rounds", json!(10)),
        ("plan_speech", json!(false)),
        ("max_actions_per_day", json!(0)),
        ("quiet_start", json!("22:00")),
        ("notes", json!(true)),
        ("dry_run", json!(true)),
    ] {
        assert_eq!(live[k], v, "{k}");
    }
    assert_eq!(source(&st, "agent_max_rounds"), "saved");
    // a flag beats a saved value, and the page is told
    st.change(&json!({"dry_run": false})).unwrap();
    assert_eq!(st.json()["fields"]["dry_run"]["ignored"], true);
    assert_eq!(st.get("dry_run"), true);
    // clearing falls back to the file, then the default
    st.change(&json!({"agent_max_rounds": null, "quiet_start": null, "quiet_end": ""}))
        .unwrap();
    assert_eq!(st.get("agent_max_rounds"), 6);
    assert!(st.get("quiet_start").is_null());
}

// REQ-CFG-062, REQ-CFG-040
#[test]
fn bad_agent_values_are_refused_and_change_nothing() {
    let mut st = SettingsState::in_memory(defaults(), false, env_with(&[]));
    for (patch, needle) in [
        (json!({"agent_max_rounds": 0}), "1 to 20"),
        (json!({"agent_max_rounds": 21}), "1 to 20"),
        (json!({"agent_max_tool_calls": "many"}), "1 to 20"),
        (json!({"agent_deadline_secs": 4}), "5 to 300"),
        (json!({"agent_tool_timeout_secs": 121}), "1 to 120"),
        (json!({"max_actions_per_day": -1}), "0 to 100000"),
        (json!({"max_costly_per_day": 1.5}), "0 to 100000"),
        (json!({"plan_speech": "yes"}), "on or off"),
        (json!({"dry_run": 1}), "on or off"),
        (
            json!({"quiet_start": "10pm", "quiet_end": "07:00"}),
            "22:00",
        ),
        (
            json!({"quiet_start": "24:00", "quiet_end": "07:00"}),
            "22:00",
        ),
        (
            json!({"quiet_start": "7:00", "quiet_end": "08:00"}),
            "22:00",
        ),
        (json!({"quiet_start": "22:00"}), "both"),
        (
            json!({"quiet_start": "22:00", "quiet_end": "22:00"}),
            "same time",
        ),
    ] {
        let e = st.change(&patch).unwrap_err();
        assert!(e.contains(needle), "{patch}: {e}");
    }
    assert!(st.get("quiet_start").is_null() && st.get("agent_max_rounds").is_null());
    assert_eq!(minute_of_day("07:30"), Some(450));
    assert_eq!(minute_of_day("23:59"), Some(1439));
    assert_eq!(minute_of_day("7:30"), None);
}

// REQ-RTV-008, REQ-RTV-005, REQ-CFG-038
#[test]
fn realtime_settings_are_layered_validated_live_and_expose_no_key() {
    let dir = TempDir::new("realtime");
    let file = layer(&[("realtime_voice", json!("verse"))]);
    let mut st = open_in(&dir.0, file, Layer::new());
    assert_eq!(source(&st, "realtime_voice"), "file");
    st.change(&json!({
        "realtime_url": "wss://api.openai.com/v1/realtime", "realtime_model": "gpt-realtime",
        "realtime_usd_per_min_in": 0.05, "realtime_usd_per_min_out": 0.2,
        "realtime_max_session_secs": 120, "realtime_transcription_model": "gpt-4o-mini-transcribe",
    }))
    .unwrap();
    let live = st.live_values();
    for k in [
        "realtime_url",
        "realtime_model",
        "realtime_max_session_secs",
        "realtime_voice",
    ] {
        assert!(!live[k].is_null(), "{k} applies without a restart");
    }
    assert_eq!(st.json()["restart_needed"], json!([]));
    assert_eq!(st.json()["realtime"]["configured"], false, "not turned on");
    assert_eq!(st.json()["realtime"]["provider"], "openai");
    // Turning it on needs the build and a key; the answer says which is missing.
    let e = st.change(&json!({"realtime_enabled": true})).unwrap_err();
    if crate::rtvoice::RtSettings::built() {
        assert!(e.contains("API key") && e.contains("OPENAI_API_KEY"), "{e}");
        st.change(&json!({"realtime_key_env": "MY_KEY", "realtime_enabled": true}))
            .unwrap();
        assert_eq!(st.json()["realtime"]["configured"], true);
    } else {
        assert!(e.contains("does not include realtime voice"), "{e}");
        assert_eq!(st.json()["realtime"]["built"], false);
    }
    let shown = st.json().to_string();
    let saved = std::fs::read_to_string(dir.0.join("settings.json")).unwrap();
    assert!(!shown.contains("secret-value") && !saved.contains("secret-value"));
}

// REQ-RTV-008, REQ-CFG-040
#[test]
fn bad_realtime_values_are_refused_with_what_to_fix_and_change_nothing() {
    let mut st = SettingsState::in_memory(defaults(), false, env_with(&[]));
    for (patch, needle) in [
        (json!({"realtime_enabled": "yes"}), "on or off"),
        (
            json!({"realtime_url": "https://x/v1", "realtime_model": "m"}),
            "wss://",
        ),
        (
            json!({"realtime_url": "wss://x y/v1", "realtime_model": "m"}),
            "wss://",
        ),
        (json!({"realtime_url": "wss://x/v1"}), "both"),
        (json!({"realtime_model": "bad name&x"}), "letters, digits"),
        (json!({"realtime_key_env": "sk-abc def"}), "NAME"),
        (
            json!({"realtime_key_env": "NOT_SET_ANYWHERE"}),
            "NOT_SET_ANYWHERE is not set",
        ),
        (json!({"realtime_usd_per_min_in": -1}), "0 to 100"),
        (json!({"realtime_usd_per_min_out": "free"}), "0 to 100"),
        (json!({"realtime_max_session_secs": 5}), "10 to 3600"),
        (json!({"realtime_max_session_secs": 4000}), "10 to 3600"),
        (json!({"realtime_enabled": true}), "address and a model"),
    ] {
        let e = st.change(&patch).unwrap_err();
        assert!(e.contains(needle), "{patch}: {e}");
    }
    assert!(st.get("realtime_url").is_null() && st.get("realtime_enabled").is_null());
}

// REQ-SEC-001, REQ-RTV-008: plain ws:// beyond the private network is refused when saved, not later.
#[test]
fn a_plain_realtime_address_beyond_the_private_network_is_refused_when_saved() {
    let mut st = SettingsState::in_memory(defaults(), false, env_with(&[]));
    let e = st
        .change(&json!({"realtime_url": "ws://api.openai.com/v1/realtime", "realtime_model": "m"}))
        .unwrap_err();
    assert!(e.contains("must not cross the internet unencrypted"), "{e}");
    assert!(st.get("realtime_url").is_null());
    for ok in [
        "ws://192.168.1.9:8000/v1",
        "ws://localhost:8000",
        "wss://x.example/v1",
    ] {
        st.change(&json!({"realtime_url": ok, "realtime_model": "m"}))
            .unwrap_or_else(|e| panic!("{ok}: {e}"));
    }
}

// REQ-CFG-065
#[test]
fn the_export_holds_only_saved_values_and_loads_back_through_the_same_checks() {
    let dir = TempDir::new("export");
    let mut st = open_in(
        &dir.0,
        layer(&[("zone", json!("America/Chicago"))]),
        Layer::new(),
    );
    assert_eq!(st.export()["settings"], json!({}), "nothing saved yet");
    st.change(&json!({"device_name": "Den", "llm_key_env": "MY_KEY", "plan_speech": false}))
        .unwrap();
    let x = st.export();
    assert_eq!(
        (x["format"].clone(), x["version"].clone()),
        (json!("clawdot-settings"), json!(1))
    );
    assert_eq!(
        x["settings"],
        json!({"device_name": "Den", "llm_key_env": "MY_KEY", "plan_speech": false})
    );
    assert!(
        !x.to_string().contains("secret-value"),
        "a key is only a variable name"
    );
    // A fresh device accepts it, and a file with a bad value is refused whole.
    let other = TempDir::new("export2");
    let mut st2 = open_in(&other.0, Layer::new(), Layer::new());
    st2.change(&x["settings"]).unwrap();
    assert_eq!(st2.get("device_name"), "Den");
    let e = st2
        .change(&json!({"device_name": "Ok", "agent_max_rounds": 99}))
        .unwrap_err();
    assert!(e.contains("1 to 20"));
    assert_eq!(st2.get("device_name"), "Den", "nothing applied");
}
