//! Settings seeding: turn the configuration file and the command line into the layered settings the portal edits
//! (REQ-CFG-038), and from those the [`Resolved`] values the program starts with.

use crate::audit::ActionPolicy;
use crate::cli::Cli;
use crate::config::{
    AgentConfig, Config, DeviceConfig, SatelliteConfig, SpeechConfig, UpdatesConfig,
};
use crate::env::Env;
#[cfg(feature = "esphome-satellite")]
use crate::esphome_cfg::EsphomeConfig;
use crate::httptools::ToolDef;
use crate::inbox::endpoint::QuietHours;
use crate::mcp::ServerDef;
use crate::media::Station;
use crate::settings::{self, Layer, SettingsState, Sources};
use std::net::SocketAddr;
use std::path::PathBuf;

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

/// What the program starts with: the effective value of every setting, the sections of the configuration file
/// that are read directly, and what the command line asked for. Built once by [`resolve`] and never changed
/// afterwards; start-up reads it by reference. It is not the command line ([`Cli`]) and not the file ([`Config`]):
/// those are its inputs.
#[derive(Debug)]
pub struct Resolved {
    pub data_dir: PathBuf,
    pub listen: SocketAddr,
    pub public_url: Option<String>,
    pub zone: Option<String>,
    /// The system prompt in effect.
    pub persona: String,
    pub llm_url: Option<String>,
    pub llm_model: Option<String>,
    pub key_env: String,
    pub daily_spend: Option<f64>,
    /// Daily action caps and the dry-run switch (REQ-SEC-015, REQ-SEC-016); the effective settings (REQ-CFG-062).
    pub actions: ActionPolicy,
    /// Turn limits, speech while working and notes (REQ-AST-019..025); the effective settings (REQ-CFG-062).
    pub agent: AgentConfig,
    /// The device-wide quiet-hours window (REQ-INB-040).
    pub quiet: Option<QuietHours>,
    /// The speech section with the effective voice, language and provider, and the provider's defaults filled in.
    pub speech: SpeechConfig,
    pub tools: Vec<ToolDef>,
    pub mcp: Vec<ServerDef>,
    pub stations: Vec<Station>,
    pub satellite: SatelliteConfig,
    pub updates: UpdatesConfig,
    /// What the device is called and which parts it has that cannot be seen (the `device` section).
    pub device: DeviceConfig,
    #[cfg(feature = "esphome-satellite")]
    pub esphome: EsphomeConfig,
    /// Endpoints to create: those named in the file, then those named on the command line.
    pub create: Vec<String>,
    pub rotate: Vec<String>,
    /// An endpoint command was given on the command line: print and exit instead of serving.
    pub manage: bool,
    pub console: bool,
    /// Echo spoken text to stdout even without the console (diagnosis only).
    pub echo_speech: bool,
    pub voice_in: Option<PathBuf>,
    pub voice_out: Option<PathBuf>,
    #[cfg(feature = "hive")]
    pub hive: Option<crate::hive::Mode>,
    #[cfg(feature = "hive")]
    pub hive_publish_name: bool,
}

/// One warning for each retired key the configuration file still sets. They are accepted so an older file loads,
/// and nothing reads them, so without a warning a person who sets a fallback model believes they have one.
fn retired_key_warnings(c: &Config) -> Vec<String> {
    let present = [
        ("llm.fallback_model", c.llm.fallback_model.is_some()),
        ("llm.summary_model", c.llm.summary_model.is_some()),
        (
            "limits.max_requests_per_day",
            c.limits.max_requests_per_day.is_some(),
        ),
    ];
    present
        .into_iter()
        .filter(|(_, set)| *set)
        .map(|(key, _)| {
            format!("warning: `{key}` in the configuration file is retired and ignored; remove it")
        })
        .collect()
}

/// Combine defaults, the configuration file, saved settings, and flags (REQ-CFG-038) into the [`Resolved`] values
/// the program starts with. Also returns the state the portal edits and any warnings.
///
/// # Errors
/// A persona file that cannot be used, a data directory whose saved settings cannot be read, or a listen address
/// that is not one.
pub fn resolve(
    cli: Cli,
    file: Config,
    env: &Env,
) -> Result<(Resolved, SettingsState, Vec<String>), String> {
    let data_dir = cli.data_dir(&file);
    let manage = !cli.create.is_empty() || !cli.rotate.is_empty();
    let mut from_file = file_layer(&file);
    let mut from_flags = cli.flags;
    // A persona file given as a flag beats one named in the file, and its text joins the layer it came from.
    let persona_flag = cli.persona_file.is_some();
    if let Some(p) = cli.persona_file.as_ref().or(file.persona_file.as_ref()) {
        let text = std::fs::read_to_string(p)
            .ok()
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| format!("cannot use persona file {}", p.display()))?;
        let layer = if persona_flag {
            &mut from_flags
        } else {
            &mut from_file
        };
        layer.insert("persona".into(), text.into());
    }
    let (mut state, mut warnings) = SettingsState::open(
        Some(&data_dir),
        Sources {
            defaults: settings::built_in_defaults(),
            file: from_file,
            flags: from_flags,
        },
        file.speech.url.is_some(),
        env.lookup(),
    )?;
    warnings.extend(retired_key_warnings(&file));
    // The machine's name is read here, once, for the address snippets print when `listen` is unspecified.
    state.set_machine_name(crate::hostguard::machine_name(env).as_deref());
    let text = |k: &str| state.get(k).as_str().map(str::to_owned);
    let listen = text("listen")
        .and_then(|l| l.parse().ok())
        .ok_or("internal: bad listen address")?;
    let mut speech = file.speech;
    if let Some(v) = text("voice") {
        speech.voice = Some(v);
    }
    // The language is empty when detected, which is not the same as unset in the configuration file.
    speech.language = text("speech_language").filter(|l| !l.is_empty());
    // ElevenLabs and Deepgram bring their own address, models and voice (REQ-VOI-021).
    speech.provider = text("speech_provider");
    speech.apply_provider_defaults();
    let live = state.live_values();
    let mut create = file.endpoints;
    create.extend(cli.create);
    let resolved = Resolved {
        data_dir,
        listen,
        public_url: text("public_url"),
        zone: text("zone").filter(|z| z != "UTC"),
        persona: text("persona").unwrap_or_default(),
        llm_url: text("llm_url"),
        llm_model: text("llm_model"),
        key_env: text("llm_key_env").unwrap_or_default(),
        daily_spend: state.get("daily_spend_usd").as_f64(),
        actions: policy_from(&live),
        agent: agent_from(&live, file.agent.summarize_over_chars),
        quiet: quiet_from(&live),
        speech,
        tools: file.tools,
        mcp: file.mcp,
        stations: file.stations,
        satellite: file.satellite,
        updates: file.updates,
        device: file.device,
        #[cfg(feature = "esphome-satellite")]
        esphome: file.esphome,
        manage,
        create,
        rotate: cli.rotate,
        console: !cli.no_console,
        echo_speech: cli.echo_speech,
        voice_in: cli.voice_in,
        voice_out: cli.voice_out,
        #[cfg(feature = "hive")]
        hive: cli.hive,
        #[cfg(feature = "hive")]
        hive_publish_name: cli.hive_publish_name,
    };
    Ok((resolved, state, warnings))
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

    fn argv(dir: &TempDir, more: &[&str]) -> Vec<String> {
        let mut v = vec!["--data-dir".to_owned(), dir.0.display().to_string()];
        v.extend(more.iter().map(|s| (*s).to_owned()));
        v
    }

    /// Resolve a configuration file with the given text and flags over a scratch data directory.
    fn resolved(dir: &TempDir, file: &str, flags: &[&str]) -> (Resolved, SettingsState) {
        let (cli, _) = cli::parse(&argv(dir, flags)).unwrap();
        let (r, state, _) = resolve(cli, Config::parse(file).unwrap(), &Env::empty()).unwrap();
        (r, state)
    }

    // REQ-CFG-038: a retired key that the file still sets is named in a warning; a file without them has none.
    #[test]
    fn each_retired_key_in_the_file_gets_one_warning() {
        let dir = TempDir::new("seed-retired");
        let run = |file: &str| {
            let (cli, _) = cli::parse(&argv(&dir, &[])).unwrap();
            resolve(cli, Config::parse(file).unwrap(), &Env::empty())
                .unwrap()
                .2
        };
        assert!(run("{}").is_empty());
        let all = run(
            r#"{"llm": {"url": "https://x.test/v1", "model": "a", "fallback_model": "b", "summary_model": "c"},
                "limits": {"max_requests_per_day": 40}}"#,
        );
        assert_eq!(
            all,
            [
                "warning: `llm.fallback_model` in the configuration file is retired and ignored; remove it",
                "warning: `llm.summary_model` in the configuration file is retired and ignored; remove it",
                "warning: `limits.max_requests_per_day` in the configuration file is retired and ignored; remove it",
            ]
        );
        let one = run(r#"{"limits": {"max_requests_per_day": 5}}"#);
        assert_eq!(one.len(), 1);
        assert!(one[0].contains("limits.max_requests_per_day"));
    }

    // REQ-CFG-083
    #[test]
    fn the_device_section_reaches_the_resolved_values() {
        let dir = TempDir::new("seed-device");
        assert_eq!(
            resolved(&dir, "{}", &[]).0.device,
            crate::config::DeviceConfig::default()
        );
        let (a, _) = resolved(
            &dir,
            r#"{"device": {"board": "kitchen-1", "label": "Kitchen", "parts": {"leds": true}}}"#,
            &[],
        );
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
        let c = Config::parse(
            r#"{"name": "Kitchen", "listen": "127.0.0.1:4100", "llm": {"url": "http://x/v1", "model": "m"},
                "limits": {"max_actions_per_day": 7, "dry_run": true},
                "agent": {"max_rounds": 6, "plan_speech": true}}"#,
        )
        .unwrap();
        let f = &file_layer(&c);
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
    }

    #[test]
    fn an_empty_file_seeds_nothing() {
        assert!(file_layer(&Config::parse("{}").unwrap()).is_empty());
    }

    // The clamp reads the range from the settings table row, which holds the same numbers as the turn ceilings.
    #[test]
    fn agent_limits_above_their_ceiling_are_shown_at_the_ceiling() {
        let c = Config::parse(
            r#"{"agent": {"max_rounds": 99, "max_tool_calls": 99, "deadline_secs": 1, "tool_timeout_secs": 500}}"#,
        )
        .unwrap();
        let f = file_layer(&c);
        assert_eq!(f["agent_max_rounds"], MAX_ROUNDS_CEILING);
        assert_eq!(f["agent_max_tool_calls"], MAX_CALLS_CEILING);
        assert_eq!(f["agent_deadline_secs"], 5);
        assert_eq!(f["agent_tool_timeout_secs"], 120);
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
        let (a, state) = resolved(&dir, "{}", &[]);
        assert_eq!(a.listen, "127.0.0.1:3335".parse().unwrap());
        assert_eq!(a.zone, None);
        assert_eq!(a.persona, crate::assistant::DEFAULT_PERSONA);
        assert_eq!(a.key_env, "OPENROUTER_API_KEY");
        assert_eq!(a.daily_spend, None);
        assert_eq!(a.llm_url, None);
        assert!(!a.actions.dry_run && a.quiet.is_none());
        assert_eq!(state.get("device_name"), "ClawDot");
        assert!(a.console && !a.manage && a.create.is_empty() && a.rotate.is_empty());
    }

    // REQ-CFG-038: defaults, then the file, then saved settings, then flags.
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
        let (cli, file) = cli::parse(&argv(
            &dir,
            &[
                "--config",
                &path.display().to_string(),
                "--llm-model",
                "from-flag",
            ],
        ))
        .unwrap();
        let (a, state, _) = resolve(cli, file, &Env::empty()).unwrap();
        assert_eq!(a.listen, "127.0.0.1:4100".parse().unwrap());
        assert_eq!(a.llm_url.as_deref(), Some("http://x/v1"));
        assert_eq!(a.llm_model.as_deref(), Some("from-flag"));
        assert_eq!(a.zone.as_deref(), Some("America/Chicago"));
        assert_eq!(a.daily_spend, Some(1.5));
        assert_eq!(a.agent.max_rounds, Some(6));
        assert_eq!(a.agent.notes, Some(true));
        assert_eq!(a.actions.max_actions_per_day, 7);
        assert_eq!(state.get("llm_model"), "from-flag");
    }

    // REQ-CFG-038: what the portal saved sits between the file and the flags.
    #[test]
    fn saved_settings_sit_between_the_file_and_the_flags() {
        let dir = TempDir::new("seed-saved");
        let file = r#"{"limits": {"daily_spend_usd": 1.5}, "llm": {"url": "http://x/v1", "model": "from-file"}}"#;
        let (_, mut state) = resolved(&dir, file, &[]);
        state
            .change(&serde_json::json!({"daily_spend_usd": 2.5, "llm_model": "saved"}))
            .unwrap();
        drop(state);
        let (a, _) = resolved(&dir, file, &["--llm-model", "from-flag"]);
        // The saved spend beats the file, and the flag beats the saved model.
        assert_eq!(a.daily_spend, Some(2.5));
        assert_eq!(a.llm_model.as_deref(), Some("from-flag"));
    }

    #[test]
    fn the_data_directory_is_the_flag_then_the_file_then_the_default() {
        let (cli, file) = cli::parse(&[]).unwrap();
        assert_eq!(cli.data_dir(&file), PathBuf::from("clawdot-data"));
        let file = Config::parse(r#"{"data_dir": "/from/file"}"#).unwrap();
        assert_eq!(cli.data_dir(&file), PathBuf::from("/from/file"));
        let (cli, _) = cli::parse(&["--data-dir".to_owned(), "/from/flag".to_owned()]).unwrap();
        assert_eq!(cli.data_dir(&file), PathBuf::from("/from/flag"));
    }

    #[test]
    fn endpoints_come_from_the_file_then_the_flags_and_only_flags_make_it_a_command() {
        let dir = TempDir::new("seed-endpoints");
        let (a, _) = resolved(&dir, r#"{"endpoints": ["from-file"]}"#, &[]);
        assert_eq!(a.create, ["from-file"]);
        assert!(!a.manage && a.console);
        let (a, _) = resolved(
            &dir,
            r#"{"endpoints": ["from-file"]}"#,
            &[
                "--create-endpoint",
                "ci:generic",
                "--rotate-endpoint",
                "old",
                "--no-console",
            ],
        );
        assert_eq!(a.create, ["from-file", "ci:generic"]);
        assert_eq!(a.rotate, ["old"]);
        assert!(a.manage && !a.console);
    }

    #[test]
    fn the_speech_section_gets_the_effective_voice_language_and_provider_defaults() {
        let dir = TempDir::new("seed-speech");
        let (a, _) = resolved(
            &dir,
            r#"{"speech": {"provider": "elevenlabs", "language": "en"}}"#,
            &[],
        );
        assert_eq!(a.speech.provider.as_deref(), Some("elevenlabs"));
        assert_eq!(a.speech.language.as_deref(), Some("en"));
        assert!(a.speech.url.is_some() && a.speech.voice.is_some());
        let (a, _) = resolved(&dir, "{}", &[]);
        assert_eq!(a.speech.provider.as_deref(), Some("openai"));
        assert!(a.speech.url.is_none());
    }

    #[test]
    fn a_persona_file_that_cannot_be_used_is_an_error() {
        let dir = TempDir::new("seed-persona");
        let with_flag = |p: &std::path::Path| {
            let (cli, file) =
                cli::parse(&argv(&dir, &["--persona-file", &p.display().to_string()])).unwrap();
            resolve(cli, file, &Env::empty())
        };
        let e = with_flag(&dir.0.join("missing.txt")).unwrap_err();
        assert!(e.starts_with("cannot use persona file"), "{e}");
        std::fs::write(dir.0.join("p.txt"), "You are terse.").unwrap();
        assert_eq!(
            with_flag(&dir.0.join("p.txt")).unwrap().0.persona,
            "You are terse."
        );
    }

    #[test]
    fn a_persona_file_flag_beats_one_named_in_the_file() {
        let dir = TempDir::new("seed-persona-layers");
        std::fs::write(dir.0.join("file.txt"), "From the file.").unwrap();
        std::fs::write(dir.0.join("flag.txt"), "From the flag.").unwrap();
        let file = format!(
            r#"{{"persona_file": {:?}}}"#,
            dir.0.join("file.txt").display().to_string()
        );
        assert_eq!(resolved(&dir, &file, &[]).0.persona, "From the file.");
        let flag = dir.0.join("flag.txt").display().to_string();
        assert_eq!(
            resolved(&dir, &file, &["--persona-file", &flag]).0.persona,
            "From the flag."
        );
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
