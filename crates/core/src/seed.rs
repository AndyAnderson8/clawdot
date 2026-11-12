//! Settings seeding: turn the parsed configuration file and flags into the layered settings the portal edits
//! (REQ-CFG-038), then copy the effective values back into [`Args`].

use crate::audit::ActionPolicy;
use crate::cli::Args;
use crate::config::{AgentConfig, Config};
use crate::env::Env;
use crate::inbox::endpoint::QuietHours;
use crate::settings::{self, Layer, SettingsState, Sources};

/// Where one setting comes from in the configuration file: the setting's key (a row of [`settings::FIELDS`]) and
/// the JSON pointer to its value in the file's settings sections, as [`settings_view`] lays them out.
#[derive(Debug, Clone, Copy)]
struct FileKey {
    setting: &'static str,
    path: &'static str,
    /// A whole number above the setting's ceiling is shown at the ceiling it will be held to (REQ-CFG-062).
    clamp: bool,
}

const fn key(setting: &'static str, path: &'static str) -> FileKey {
    FileKey {
        setting,
        path,
        clamp: false,
    }
}

const fn clamped(setting: &'static str, path: &'static str) -> FileKey {
    FileKey {
        setting,
        path,
        clamp: true,
    }
}

/// Every setting whose value can come from the configuration file, and where in the file it comes from. A setting
/// the file cannot set (the persona text comes from a file of its own, the price per token and the quiet hours
/// only from the portal) has no row. A test requires that every field of the file's settings sections has a row
/// here or is named in `NOT_SETTINGS`, and that every row names a setting and a field that exist.
const FILE_KEYS: &[FileKey] = &[
    key("device_name", "/name"),
    key("zone", "/zone"),
    key("listen", "/listen"),
    key("public_url", "/public_url"),
    key("llm_url", "/llm/url"),
    key("llm_model", "/llm/model"),
    key("llm_key_env", "/llm/key_env"),
    key("daily_spend_usd", "/limits/daily_spend_usd"),
    key("voice", "/speech/voice"),
    key("speech_provider", "/speech/provider"),
    key("speech_language", "/speech/language"),
    clamped("agent_max_rounds", "/agent/max_rounds"),
    clamped("agent_max_tool_calls", "/agent/max_tool_calls"),
    clamped("agent_deadline_secs", "/agent/deadline_secs"),
    clamped("agent_tool_timeout_secs", "/agent/tool_timeout_secs"),
    key("plan_speech", "/agent/plan_speech"),
    key("progress_speech", "/agent/progress_speech"),
    key("notes", "/agent/notes"),
    key("update_check", "/updates/check"),
    key("max_actions_per_day", "/limits/max_actions_per_day"),
    key("max_costly_per_day", "/limits/max_costly_per_day"),
    key("dry_run", "/limits/dry_run"),
    key("realtime_enabled", "/voice/realtime/enabled"),
    key("realtime_url", "/voice/realtime/url"),
    key("realtime_model", "/voice/realtime/model"),
    key("realtime_voice", "/voice/realtime/voice"),
    key("realtime_key_env", "/voice/realtime/key_env"),
    key(
        "realtime_transcription_model",
        "/voice/realtime/transcription_model",
    ),
    key("realtime_usd_per_min_in", "/voice/realtime/usd_per_min_in"),
    key(
        "realtime_usd_per_min_out",
        "/voice/realtime/usd_per_min_out",
    ),
    key(
        "realtime_max_session_secs",
        "/voice/realtime/max_session_secs",
    ),
];

/// The settings sections of `c` as one JSON object, so a row of [`FILE_KEYS`] can find its value by pointer. An
/// unset field is `null`.
fn settings_view(c: &Config) -> serde_json::Value {
    serde_json::json!({
        "name": c.name,
        "zone": c.zone,
        "listen": c.listen,
        "public_url": c.public_url,
        "llm": c.llm,
        "limits": c.limits,
        "agent": c.agent,
        "speech": c.speech,
        "updates": c.updates,
        "voice": c.voice,
    })
}

/// The settings layer for the configuration file: the value of every setting the file sets, and nothing else, so
/// the defaults show through for the rest.
pub fn file_layer(c: &Config) -> Layer {
    let view = settings_view(c);
    FILE_KEYS
        .iter()
        .filter_map(|k| {
            let v = view.pointer(k.path).filter(|v| !v.is_null())?;
            let v = match v.as_u64() {
                // The range is the one on the setting's table row, so the two cannot drift apart.
                Some(n) if k.clamp => {
                    let (lo, hi) = settings::whole_range(k.setting).unwrap_or((0, u64::MAX));
                    serde_json::Value::from(n.clamp(lo, hi))
                }
                _ => v.clone(),
            };
            Some((k.setting.to_owned(), v))
        })
        .collect()
}

pub fn seed_from_config(a: &mut Args, c: &Config) -> Result<(), String> {
    if let Some(d) = &c.data_dir {
        a.data_dir.clone_from(d);
    }
    if let Some(l) = c.listen_addr()? {
        a.listen = l;
    }
    a.persona_file.clone_from(&c.persona_file);
    a.tools.clone_from(&c.tools);
    a.mcp.clone_from(&c.mcp);
    a.stations.clone_from(&c.stations);
    a.speech.clone_from(&c.speech);
    a.satellite.clone_from(&c.satellite);
    a.updates.clone_from(&c.updates);
    a.device.clone_from(&c.device);
    #[cfg(feature = "esphome-satellite")]
    a.esphome.clone_from(&c.esphome);
    a.create.extend(c.endpoints.iter().cloned());
    a.agent = c.agent.clone();
    a.file_layer.extend(file_layer(c));
    Ok(())
}

/// Combine defaults, the configuration file, saved settings, and flags (REQ-CFG-038), then copy the
/// effective values into `args`. Returns the state the portal edits and any warnings.
pub fn resolve_settings(
    args: &mut Args,
    env: &Env,
) -> Result<(SettingsState, Vec<String>), String> {
    if let Some(p) = &args.persona_file {
        let text = std::fs::read_to_string(p)
            .ok()
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| format!("cannot use persona file {}", p.display()))?;
        let layer = if args.persona_flag {
            &mut args.flag_layer
        } else {
            &mut args.file_layer
        };
        layer.insert("persona".into(), text.into());
    }
    let (state, warnings) = SettingsState::open(
        Some(&args.data_dir),
        Sources {
            defaults: settings::built_in_defaults(),
            file: std::mem::take(&mut args.file_layer),
            flags: std::mem::take(&mut args.flag_layer),
        },
        args.speech.url.is_some(),
        env.lookup(),
    )?;
    let text = |k: &str| state.get(k).as_str().map(str::to_owned);
    args.listen = text("listen")
        .and_then(|l| l.parse().ok())
        .ok_or("internal: bad listen address")?;
    args.public_url = text("public_url");
    args.zone = text("zone").filter(|z| z != "UTC");
    args.persona = text("persona").unwrap_or_default();
    args.llm_url = text("llm_url");
    args.llm_model = text("llm_model");
    args.key_env = text("llm_key_env").unwrap_or_default();
    args.daily_spend = state.get("daily_spend_usd").as_f64();
    if let Some(v) = text("voice") {
        args.speech.voice = Some(v);
    }
    // The language is empty when detected, which is not the same as unset in the configuration file.
    args.speech.language = text("speech_language").filter(|l| !l.is_empty());
    // ElevenLabs and Deepgram bring their own address, models and voice (REQ-VOI-021).
    args.speech.provider = text("speech_provider");
    args.speech.apply_provider_defaults();
    let live = state.live_values();
    args.agent = agent_from(&live, args.agent.summarize_over_chars);
    args.actions = policy_from(&live);
    args.quiet = quiet_from(&live);
    Ok((state, warnings))
}

/// The agent settings among the live values (REQ-CFG-062).
pub fn agent_from(
    live: &serde_json::Map<String, serde_json::Value>,
    summarize: Option<usize>,
) -> AgentConfig {
    let num = |k: &str| live.get(k).and_then(serde_json::Value::as_u64);
    let flag = |k: &str| live.get(k).and_then(serde_json::Value::as_bool);
    AgentConfig {
        max_rounds: num("agent_max_rounds").and_then(|n| usize::try_from(n).ok()),
        max_tool_calls: num("agent_max_tool_calls").and_then(|n| usize::try_from(n).ok()),
        deadline_secs: num("agent_deadline_secs"),
        tool_timeout_secs: num("agent_tool_timeout_secs"),
        plan_speech: flag("plan_speech"),
        progress_speech: flag("progress_speech"),
        summarize_over_chars: summarize,
        notes: flag("notes"),
    }
}

/// The daily caps and the dry-run switch among the live values.
pub fn policy_from(live: &serde_json::Map<String, serde_json::Value>) -> ActionPolicy {
    let d = ActionPolicy::default();
    let num = |k: &str, or: u32| {
        live.get(k)
            .and_then(serde_json::Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(or)
    };
    ActionPolicy {
        dry_run: live
            .get("dry_run")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(d.dry_run),
        max_actions_per_day: num("max_actions_per_day", d.max_actions_per_day),
        max_costly_per_day: num("max_costly_per_day", d.max_costly_per_day),
    }
}

/// The quiet-hours window among the live values, when both ends are set (REQ-INB-040).
pub fn quiet_from(live: &serde_json::Map<String, serde_json::Value>) -> Option<QuietHours> {
    let at = |k: &str| {
        live.get(k)
            .and_then(serde_json::Value::as_str)
            .and_then(settings::minute_of_day)
    };
    Some(QuietHours {
        start: at("quiet_start")?,
        end: at("quiet_end")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli;
    use crate::session::{MAX_CALLS_CEILING, MAX_DEADLINE_CEILING_SECS, MAX_ROUNDS_CEILING};
    use crate::store::testutil::TempDir;

    fn blank_args() -> Args {
        cli::parse(&[]).unwrap()
    }

    // REQ-CFG-083
    #[test]
    fn the_device_section_reaches_the_arguments() {
        let mut a = blank_args();
        assert_eq!(a.device, crate::config::DeviceConfig::default());
        let c = Config::parse(
            r#"{"device": {"board": "kitchen-1", "label": "Kitchen", "parts": {"leds": true}}}"#,
        )
        .unwrap();
        seed_from_config(&mut a, &c).unwrap();
        let h = crate::hardware::Hardware::default().with_device(&a.device);
        assert_eq!(
            (h.board.as_str(), h.label.as_str()),
            ("kitchen-1", "Kitchen")
        );
        assert!(h.parts.leds);
    }

    // REQ-CFG-038, REQ-CFG-062
    #[test]
    fn the_file_layer_gets_what_the_file_sets_and_nothing_else() {
        let mut a = blank_args();
        let c = Config::parse(
            r#"{"name": "Kitchen", "listen": "127.0.0.1:4100", "llm": {"url": "http://x/v1", "model": "m"},
                "limits": {"max_actions_per_day": 7, "dry_run": true},
                "agent": {"max_rounds": 6, "plan_speech": true}}"#,
        )
        .unwrap();
        seed_from_config(&mut a, &c).unwrap();
        assert_eq!(a.listen, "127.0.0.1:4100".parse().unwrap());
        let f = &a.file_layer;
        assert_eq!(f["device_name"], "Kitchen");
        assert_eq!(f["listen"], "127.0.0.1:4100");
        assert_eq!(f["llm_model"], "m");
        assert_eq!(f["max_actions_per_day"], 7);
        assert_eq!(f["dry_run"], true);
        assert_eq!(f["agent_max_rounds"], 6);
        assert_eq!(f["plan_speech"], true);
        // Settings the file does not mention stay out of the layer, so the defaults show through.
        for k in [
            "zone",
            "public_url",
            "notes",
            "agent_deadline_secs",
            "voice",
        ] {
            assert!(!f.contains_key(k), "{k}");
        }
        assert!(a.flag_layer.is_empty());
    }

    #[test]
    fn an_empty_file_seeds_nothing() {
        let mut a = blank_args();
        seed_from_config(&mut a, &Config::parse("{}").unwrap()).unwrap();
        assert!(a.file_layer.is_empty());
        assert_eq!(a.listen, "127.0.0.1:3335".parse().unwrap());
    }

    // The clamp reads the range from the settings table row, which holds the same numbers as the turn ceilings.
    #[test]
    fn agent_limits_above_their_ceiling_are_shown_at_the_ceiling() {
        let mut a = blank_args();
        let c = Config::parse(
            r#"{"agent": {"max_rounds": 99, "max_tool_calls": 99, "deadline_secs": 1, "tool_timeout_secs": 500}}"#,
        )
        .unwrap();
        seed_from_config(&mut a, &c).unwrap();
        assert_eq!(a.file_layer["agent_max_rounds"], MAX_ROUNDS_CEILING);
        assert_eq!(a.file_layer["agent_max_tool_calls"], MAX_CALLS_CEILING);
        assert_eq!(a.file_layer["agent_deadline_secs"], 5);
        assert_eq!(a.file_layer["agent_tool_timeout_secs"], 120);
    }

    #[test]
    fn the_table_ranges_match_the_turn_ceilings() {
        assert_eq!(
            settings::whole_range("agent_max_rounds"),
            Some((1, MAX_ROUNDS_CEILING as u64))
        );
        assert_eq!(
            settings::whole_range("agent_max_tool_calls"),
            Some((1, MAX_CALLS_CEILING as u64))
        );
        assert_eq!(
            settings::whole_range("agent_deadline_secs"),
            Some((5, MAX_DEADLINE_CEILING_SECS))
        );
        assert_eq!(
            settings::whole_range("agent_tool_timeout_secs"),
            Some((1, 120))
        );
        assert_eq!(settings::whole_range("zone"), None);
    }

    /// Top-level keys of the file that are not settings: the program reads them straight from the parsed file.
    /// (`speech` and `updates` have rows in `FILE_KEYS` for the few of their fields that are settings; the rest of
    /// those two sections is read directly too.)
    const READ_DIRECTLY: &[&str] = &[
        "data_dir",
        "persona_file",
        "endpoints",
        "tools",
        "mcp",
        "stations",
        "speech",
        "satellite",
        "device",
        "updates",
        "esphome",
    ];

    /// Fields of the settings sections that are not settings and are not mapped: three were removed and are
    /// accepted but ignored (see `settings::RETIRED`), `summarize_over_chars` is read from the parsed file, and
    /// the realtime `provider` has one possible value.
    const NOT_SETTINGS: &[&str] = &[
        "/llm/fallback_model",
        "/llm/summary_model",
        "/limits/max_requests_per_day",
        "/agent/summarize_over_chars",
        "/voice/realtime/provider",
    ];

    /// The top-level keys of the file, as serde lists them in the error for an unknown key.
    fn file_keys() -> Vec<String> {
        let e = serde_json::from_str::<Config>(r#"{"__no_such_key": 0}"#)
            .unwrap_err()
            .to_string();
        let listed = e.split("expected").nth(1).unwrap();
        listed
            .split('`')
            .skip(1)
            .step_by(2)
            .map(str::to_owned)
            .collect()
    }

    /// The pointer to every field in `v`, down to the ones that are not objects.
    fn leaves(v: &serde_json::Value, at: &str, out: &mut Vec<String>) {
        match v {
            serde_json::Value::Object(m) => {
                for (k, x) in m {
                    leaves(x, &format!("{at}/{k}"), out);
                }
            }
            _ => out.push(at.to_owned()),
        }
    }

    // REQ-CFG-038, REQ-CFG-062: a setting that can come from the file says where, in one place.
    #[test]
    fn every_row_of_the_file_table_names_a_setting_and_a_field_that_exist() {
        let mut leaf = Vec::new();
        leaves(&settings_view(&Config::default()), "", &mut leaf);
        let mut seen = std::collections::BTreeSet::new();
        for k in FILE_KEYS {
            assert!(settings::field(k.setting).is_some(), "{}", k.setting);
            assert!(leaf.iter().any(|l| l == k.path), "{}", k.path);
            assert!(seen.insert(k.setting), "{} appears twice", k.setting);
            assert!(
                !k.clamp || settings::whole_range(k.setting).is_some(),
                "{} is clamped but is not a whole number",
                k.setting
            );
        }
        let paths: std::collections::BTreeSet<_> = FILE_KEYS.iter().map(|k| k.path).collect();
        assert_eq!(paths.len(), FILE_KEYS.len(), "a path is mapped twice");
    }

    // A field added to the file's settings sections without a row fails here until it has one (or is named as not
    // a setting).
    #[test]
    fn every_field_of_the_settings_sections_has_a_row_or_is_named_as_not_a_setting() {
        let top = file_keys();
        for k in READ_DIRECTLY {
            assert!(top.iter().any(|t| t == k), "`{k}` is not a key of the file");
        }
        for t in &top {
            let mapped = FILE_KEYS
                .iter()
                .any(|k| k.path.split('/').nth(1) == Some(t.as_str()));
            assert!(
                mapped || READ_DIRECTLY.contains(&t.as_str()),
                "`{t}` is a key of the file with no row in FILE_KEYS; add one, or add it to READ_DIRECTLY"
            );
        }
        let mut leaf = Vec::new();
        leaves(&settings_view(&Config::default()), "", &mut leaf);
        for l in &leaf {
            let section = l.split('/').nth(1).unwrap();
            if READ_DIRECTLY.contains(&section) {
                continue;
            }
            let mapped = FILE_KEYS.iter().any(|k| k.path == l);
            assert!(
                mapped != NOT_SETTINGS.contains(&l.as_str()),
                "`{l}` must have a row in FILE_KEYS or be listed in NOT_SETTINGS, and not both"
            );
        }
        for n in NOT_SETTINGS {
            assert!(leaf.iter().any(|l| l == n), "{n} is not a field");
        }
    }

    // Each row delivers the field it names: a file that sets only that field puts only that setting in the layer.
    #[test]
    fn each_row_carries_its_field_into_the_layer() {
        use serde_json::json;
        for k in FILE_KEYS {
            let delivered = [json!("x"), json!(true), json!(7), json!(1.5)]
                .into_iter()
                .find_map(|probe| {
                    let mut file = probe.clone();
                    for part in k
                        .path
                        .split('/')
                        .skip(1)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                    {
                        file = json!({ part: file });
                    }
                    let c: Config = serde_json::from_value(file).ok()?;
                    Some(file_layer(&c)).filter(|l| !l.is_empty())
                })
                .unwrap_or_else(|| panic!("{} did not reach the layer", k.path));
            assert_eq!(
                delivered.keys().map(String::as_str).collect::<Vec<_>>(),
                [k.setting],
                "{}",
                k.path
            );
        }
    }

    #[test]
    fn defaults_apply_when_no_layer_sets_a_value() {
        let dir = TempDir::new("seed-defaults");
        let mut a = blank_args();
        a.data_dir.clone_from(&dir.0);
        let (state, _) = resolve_settings(&mut a, &Env::empty()).unwrap();
        assert_eq!(a.listen, "127.0.0.1:3335".parse().unwrap());
        assert_eq!(a.zone, None);
        assert_eq!(a.persona, crate::assistant::DEFAULT_PERSONA);
        assert_eq!(a.key_env, "OPENROUTER_API_KEY");
        assert_eq!(a.daily_spend, None);
        assert_eq!(a.llm_url, None);
        assert!(!a.actions.dry_run && a.quiet.is_none());
        assert_eq!(state.get("device_name"), "ClawDot");
    }

    #[test]
    fn the_file_layer_beats_defaults_and_flags_beat_the_file() {
        let dir = TempDir::new("seed-layers");
        let path = dir.0.join("c.json");
        std::fs::write(
            &path,
            r#"{"listen": "127.0.0.1:4100", "zone": "America/Chicago", "llm": {"url": "http://x/v1", "model": "from-file"},
                "limits": {"max_actions_per_day": 7, "daily_spend_usd": 1.5},
                "agent": {"max_rounds": 6, "notes": true}}"#,
        )
        .unwrap();
        let mut a = cli::parse(&[
            "--config".to_owned(),
            path.display().to_string(),
            "--data-dir".to_owned(),
            dir.0.display().to_string(),
            "--llm-model".to_owned(),
            "from-flag".to_owned(),
        ])
        .unwrap();
        let (state, _) = resolve_settings(&mut a, &Env::empty()).unwrap();
        assert_eq!(a.listen, "127.0.0.1:4100".parse().unwrap());
        assert_eq!(a.llm_url.as_deref(), Some("http://x/v1"));
        assert_eq!(a.llm_model.as_deref(), Some("from-flag"));
        assert_eq!(a.zone.as_deref(), Some("America/Chicago"));
        assert_eq!(a.daily_spend, Some(1.5));
        assert_eq!(a.agent.max_rounds, Some(6));
        assert_eq!(a.agent.notes, Some(true));
        assert_eq!(a.actions.max_actions_per_day, 7);
        assert_eq!(state.get("llm_model"), "from-flag");
        // The layers were handed to the settings state.
        assert!(a.file_layer.is_empty() && a.flag_layer.is_empty());
    }

    #[test]
    fn a_persona_file_that_cannot_be_used_is_an_error() {
        let dir = TempDir::new("seed-persona");
        let mut a = blank_args();
        a.data_dir.clone_from(&dir.0);
        a.persona_file = Some(dir.0.join("missing.txt"));
        let e = resolve_settings(&mut a, &Env::empty()).unwrap_err();
        assert!(e.starts_with("cannot use persona file"), "{e}");
        std::fs::write(dir.0.join("p.txt"), "You are terse.").unwrap();
        a.persona_file = Some(dir.0.join("p.txt"));
        a.persona_flag = true;
        resolve_settings(&mut a, &Env::empty()).unwrap();
        assert_eq!(a.persona, "You are terse.");
    }

    #[test]
    fn live_values_become_the_agent_policy_and_quiet_hours() {
        let live: serde_json::Map<String, serde_json::Value> = serde_json::from_str(
            r#"{"agent_max_rounds": 3, "agent_deadline_secs": 40, "plan_speech": false,
                "dry_run": true, "max_actions_per_day": 9, "quiet_start": "22:00", "quiet_end": "07:00"}"#,
        )
        .unwrap();
        let agent = agent_from(&live, Some(500));
        assert_eq!(agent.max_rounds, Some(3));
        assert_eq!(agent.deadline_secs, Some(40));
        assert_eq!(agent.plan_speech, Some(false));
        assert_eq!(agent.summarize_over_chars, Some(500));
        assert_eq!(agent.max_tool_calls, None);
        let p = policy_from(&live);
        assert!(p.dry_run);
        assert_eq!(p.max_actions_per_day, 9);
        assert_eq!(
            p.max_costly_per_day,
            ActionPolicy::default().max_costly_per_day
        );
        let q = quiet_from(&live).unwrap();
        assert!(q.contains(23 * 60) && !q.contains(12 * 60));
        assert!(quiet_from(&serde_json::Map::new()).is_none());
    }
}
