//! Settings seeding: turn the parsed configuration file and flags into the layered settings the portal edits
//! (REQ-CFG-038), then copy the effective values back into [`Args`].

use crate::audit::ActionPolicy;
use crate::cli::Args;
use crate::config::{AgentConfig, Config};
use crate::inbox::endpoint::QuietHours;
use crate::session::{MAX_CALLS_CEILING, MAX_DEADLINE_CEILING_SECS, MAX_ROUNDS_CEILING};
use crate::settings::{self, Layer, SettingsState};

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
    #[cfg(feature = "esphome-satellite")]
    a.esphome.clone_from(&c.esphome);
    a.create.extend(c.endpoints.iter().cloned());
    let mut put = |k: &str, v: Option<serde_json::Value>| {
        if let Some(v) = v {
            a.file_layer.insert(k.to_owned(), v);
        }
    };
    put("device_name", c.name.clone().map(Into::into));
    put("zone", c.zone.clone().map(Into::into));
    put("listen", c.listen.clone().map(Into::into));
    put("public_url", c.public_url.clone().map(Into::into));
    put("llm_url", c.llm.url.clone().map(Into::into));
    put("llm_model", c.llm.model.clone().map(Into::into));
    put("llm_key_env", c.llm.key_env.clone().map(Into::into));
    put("daily_spend_usd", c.limits.daily_spend_usd.map(Into::into));
    put("voice", c.speech.voice.clone().map(Into::into));
    put("speech_provider", c.speech.provider.clone().map(Into::into));
    put("speech_language", c.speech.language.clone().map(Into::into));
    a.agent = c.agent.clone();
    // The agent and limit settings join the layers so the portal can show and change them (REQ-CFG-062).
    // A value above its ceiling is shown at the ceiling it will be held to.
    let ceil =
        |v: Option<u64>, lo: u64, hi: u64| v.map(|n| serde_json::Value::from(n.clamp(lo, hi)));
    put(
        "agent_max_rounds",
        ceil(
            c.agent.max_rounds.map(|n| n as u64),
            1,
            MAX_ROUNDS_CEILING as u64,
        ),
    );
    put(
        "agent_max_tool_calls",
        ceil(
            c.agent.max_tool_calls.map(|n| n as u64),
            1,
            MAX_CALLS_CEILING as u64,
        ),
    );
    put(
        "agent_deadline_secs",
        ceil(c.agent.deadline_secs, 5, MAX_DEADLINE_CEILING_SECS),
    );
    put(
        "agent_tool_timeout_secs",
        ceil(c.agent.tool_timeout_secs, 1, 120),
    );
    put("plan_speech", c.agent.plan_speech.map(Into::into));
    put("progress_speech", c.agent.progress_speech.map(Into::into));
    put("notes", c.agent.notes.map(Into::into));
    put("update_check", c.updates.check.map(Into::into));
    put(
        "max_actions_per_day",
        c.limits.max_actions_per_day.map(Into::into),
    );
    put(
        "max_costly_per_day",
        c.limits.max_costly_per_day.map(Into::into),
    );
    put("dry_run", c.limits.dry_run.map(Into::into));
    let r = &c.voice.realtime;
    put("realtime_enabled", r.enabled.map(Into::into));
    put("realtime_url", r.url.clone().map(Into::into));
    put("realtime_model", r.model.clone().map(Into::into));
    put("realtime_voice", r.voice.clone().map(Into::into));
    put("realtime_key_env", r.key_env.clone().map(Into::into));
    put(
        "realtime_transcription_model",
        r.transcription_model.clone().map(Into::into),
    );
    put("realtime_usd_per_min_in", r.usd_per_min_in.map(Into::into));
    put(
        "realtime_usd_per_min_out",
        r.usd_per_min_out.map(Into::into),
    );
    put(
        "realtime_max_session_secs",
        r.max_session_secs.map(Into::into),
    );
    Ok(())
}

/// Combine defaults, the configuration file, saved settings, and flags (REQ-CFG-038), then copy the
/// effective values into `args`. Returns the state the portal edits and any warnings.
pub fn resolve_settings(args: &mut Args) -> Result<(SettingsState, Vec<String>), String> {
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
    let defaults: Layer = settings::built_in_defaults();
    let (state, warnings) = SettingsState::open(
        Some(&args.data_dir),
        defaults,
        std::mem::take(&mut args.file_layer),
        std::mem::take(&mut args.flag_layer),
        args.speech.url.is_some(),
        Box::new(|n| std::env::var(n).ok()),
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
