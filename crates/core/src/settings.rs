//! Settings that can be changed from the portal (REQ-CFG-037..042).
//!
//! Values come from four layers, lowest first: built-in defaults, the configuration file, `settings.json` in
//! the data directory (what the portal saved), and command-line flags. The effective value of a setting is the
//! highest layer that has it, and the portal shows which layer that was. A flag therefore beats a saved value.
//!
//! This module is pure state and validation: it never talks to the network. Applying a live change to the
//! running assistant is the main loop's job (it receives [`SettingsState::live_values`]).
//!
//! Secrets are not settings. A key is named by the environment variable that holds it (REQ-CFG-042).

use crate::store::JsonStore;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::Path;

/// Schema version of `settings.json`.
pub const SCHEMA: u32 = 1;

/// The name of the export file format (REQ-CFG-065).
pub const EXPORT_FORMAT: &str = "clawdot-settings";

/// Settings by key. A missing key means "not set at this layer".
pub type Layer = BTreeMap<String, Value>;

/// How a lookup of an environment variable is done; tests replace it.
pub type EnvFn = Box<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// Which layer an effective value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Built into ClawDot.
    Default,
    /// The configuration file.
    File,
    /// Saved from the portal.
    Saved,
    /// A command-line flag.
    Flag,
}

impl Source {
    /// The word the API uses.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::File => "file",
            Self::Saved => "saved",
            Self::Flag => "flag",
        }
    }
}

/// One setting: its key and whether a change applies without a restart.
#[derive(Debug, Clone, Copy)]
pub struct Field {
    /// The key used in `settings.json` and the API.
    pub key: &'static str,
    /// True when the running assistant can switch immediately.
    pub live: bool,
}

/// Every setting the portal can change (REQ-CFG-037, REQ-CFG-039).
pub const FIELDS: [Field; 35] = [
    Field {
        key: "device_name",
        live: true,
    },
    Field {
        key: "zone",
        live: true,
    },
    Field {
        key: "persona",
        live: true,
    },
    Field {
        key: "llm_url",
        live: true,
    },
    Field {
        key: "llm_model",
        live: true,
    },
    Field {
        key: "llm_key_env",
        live: true,
    },
    Field {
        key: "voice",
        live: true,
    },
    Field {
        key: "speech_language",
        live: true,
    },
    // Which service's wire format the speech client speaks; the clients are built at start (REQ-VOI-021).
    Field {
        key: "speech_provider",
        live: false,
    },
    Field {
        key: "listen",
        live: false,
    },
    Field {
        key: "public_url",
        live: false,
    },
    Field {
        key: "daily_spend_usd",
        live: true,
    },
    Field {
        key: "board",
        live: false,
    },
    Field {
        key: "agent_max_rounds",
        live: true,
    },
    Field {
        key: "agent_max_tool_calls",
        live: true,
    },
    Field {
        key: "agent_deadline_secs",
        live: true,
    },
    Field {
        key: "agent_tool_timeout_secs",
        live: true,
    },
    Field {
        key: "plan_speech",
        live: true,
    },
    Field {
        key: "progress_speech",
        live: true,
    },
    Field {
        key: "notes",
        live: true,
    },
    Field {
        key: "update_check",
        live: true,
    },
    Field {
        key: "max_actions_per_day",
        live: true,
    },
    Field {
        key: "max_costly_per_day",
        live: true,
    },
    Field {
        key: "dry_run",
        live: true,
    },
    Field {
        key: "quiet_start",
        live: true,
    },
    Field {
        key: "quiet_end",
        live: true,
    },
    Field {
        key: "realtime_enabled",
        live: true,
    },
    Field {
        key: "realtime_url",
        live: true,
    },
    Field {
        key: "realtime_model",
        live: true,
    },
    Field {
        key: "realtime_voice",
        live: true,
    },
    Field {
        key: "realtime_key_env",
        live: true,
    },
    Field {
        key: "realtime_transcription_model",
        live: true,
    },
    Field {
        key: "realtime_usd_per_min_in",
        live: true,
    },
    Field {
        key: "realtime_usd_per_min_out",
        live: true,
    },
    Field {
        key: "realtime_max_session_secs",
        live: true,
    },
];

/// The default listen address (REQ-CFG-014).
pub const DEFAULT_LISTEN: &str = "127.0.0.1:3335";

/// The base URL printed in snippets when none is set, and whether the host was assumed because the listen
/// address (`0.0.0.0`, `[::]`) cannot be dialled as written (REQ-CFG-016).
#[must_use]
pub fn public_url_from_listen(listen: SocketAddr) -> (String, bool) {
    if listen.ip().is_unspecified() {
        (format!("http://clawdot.local:{}", listen.port()), true)
    } else {
        (format!("http://{listen}"), false)
    }
}

/// Settings that were removed. A file saved by an older version may still hold one, and it is skipped quietly.
const RETIRED: [&str; 3] = [
    "max_requests_per_day",
    "llm_fallback_model",
    "summary_model",
];

/// Whether `n` can be the name of an environment variable (letters, digits and underscores, not starting
/// with a digit).
#[must_use]
pub fn is_env_name(n: &str) -> bool {
    n.chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn is_field(key: &str) -> bool {
    FIELDS.iter().any(|f| f.key == key)
}

fn text(v: &Value, what: &str) -> Result<String, String> {
    match v {
        Value::String(s) => Ok(s.trim().to_owned()),
        _ => Err(format!("{what} must be text.")),
    }
}

fn plain(s: &str, what: &str, max: usize) -> Result<String, String> {
    if s.chars().count() > max {
        return Err(format!("{what} is too long: use at most {max} characters."));
    }
    if s.chars().any(char::is_control) {
        return Err(format!(
            "{what} cannot contain line breaks or control characters."
        ));
    }
    Ok(s.to_owned())
}

fn check_zone(name: &str) -> Result<(), String> {
    #[cfg(feature = "tz")]
    {
        crate::sched::IanaZone::new(name).map(|_| ()).map_err(|_| {
            format!("`{name}` is not a time zone name. Pick one from the list, for example Europe/Berlin.")
        })
    }
    #[cfg(not(feature = "tz"))]
    {
        if name == "UTC" {
            Ok(())
        } else {
            Err("This build has no time zone database, so only UTC can be used.".to_owned())
        }
    }
}

/// A whole number from `lo` to `hi`.
fn whole(v: &Value, what: &str, lo: u64, hi: u64) -> Result<Value, String> {
    v.as_u64()
        .filter(|n| (lo..=hi).contains(n))
        .map(|n| json!(n))
        .ok_or_else(|| format!("{what} must be a whole number from {lo} to {hi}."))
}

/// Minutes after local midnight for `HH:MM` (24-hour, two digits each), or `None`.
#[must_use]
pub fn minute_of_day(hhmm: &str) -> Option<u16> {
    let (h, m) = hhmm.split_once(':')?;
    let ok = |s: &str| s.len() == 2 && s.bytes().all(|b| b.is_ascii_digit());
    if !ok(h) || !ok(m) {
        return None;
    }
    let (h, m): (u16, u16) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

/// Check one value and return it in the form that is stored. Empty text counts as "clear" and is
/// handled by the caller before this is reached.
fn normalize(key: &str, v: &Value, env: &dyn Fn(&str) -> Option<String>) -> Result<Value, String> {
    let string = |what: &str, max: usize| -> Result<String, String> {
        let s = text(v, what)?;
        plain(&s, what, max)
    };
    Ok(match key {
        "device_name" => json!(string("The device name", 40)?),
        "zone" => {
            let z = string("The time zone", 64)?;
            check_zone(&z)?;
            json!(z)
        }
        "persona" => {
            let s = text(v, "The persona")?;
            if s.chars().count() > 4000 {
                return Err("The persona is too long: use at most 4000 characters.".into());
            }
            if s.chars().any(|c| c.is_control() && c != '\n' && c != '\t') {
                return Err("The persona cannot contain control characters.".into());
            }
            json!(s)
        }
        "llm_url" => {
            let u = string("The language model address", 300)?;
            if u.contains(char::is_whitespace) {
                return Err("The language model address cannot contain spaces.".into());
            }
            if !(u.starts_with("http://") || u.starts_with("https://")) {
                return Err(
                    "The language model address must start with https:// (or http:// on your own network), for example https://openrouter.ai/api/v1.".into(),
                );
            }
            // The same rules the client applies when it connects (REQ-SEC-001).
            crate::transport::HttpBackend::new(&u, "", std::time::Duration::from_secs(1))?;
            json!(u)
        }
        "llm_model" => {
            let what = "The model name";
            let s = string(what, 200)?;
            if s.contains(char::is_whitespace) {
                return Err(format!("{what} cannot contain spaces."));
            }
            json!(s)
        }
        "llm_key_env" => {
            let n = string("The key variable name", 64)?;
            let ok = n
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
            if !ok {
                return Err(
                    "Enter the NAME of an environment variable (letters, digits, and underscores), not the key itself.".into(),
                );
            }
            // A key pasted into the page needs no variable at all.
            if crate::secrets::get(crate::secrets::LLM_KEY).is_none()
                && env(&n).is_none_or(|v| v.is_empty())
            {
                return Err(format!(
                    "The environment variable {n} is not set where ClawDot runs. Set it there (for example in the service's environment file), restart ClawDot, then save this again."
                ));
            }
            json!(n)
        }
        "realtime_enabled" => json!(
            v.as_bool()
                .ok_or("Realtime voice must be on or off (true or false).")?
        ),
        "realtime_url" => {
            let u = string("The realtime voice address", 300)?;
            if u.contains(char::is_whitespace)
                || !(u.starts_with("wss://") || u.starts_with("ws://"))
            {
                return Err("The realtime voice address must start with wss:// (or ws:// on your own network), for example wss://api.openai.com/v1/realtime.".into());
            }
            json!(u)
        }
        "realtime_model" => {
            let s = string("The realtime model name", 100)?;
            if s.is_empty()
                || !s
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_.:".contains(c))
            {
                return Err(
                    "The realtime model name can use only letters, digits, and - _ . :".into(),
                );
            }
            json!(s)
        }
        "realtime_voice" => json!(string("The realtime voice", 64)?),
        "realtime_transcription_model" => json!(string("The transcription model", 100)?),
        "realtime_key_env" => {
            let n = string("The realtime key variable name", 64)?;
            if !is_env_name(&n) {
                return Err(
                    "Enter the NAME of an environment variable (letters, digits, and underscores), not the key itself.".into(),
                );
            }
            if crate::secrets::get(crate::secrets::REALTIME_KEY).is_none()
                && env(&n).is_none_or(|v| v.is_empty())
            {
                return Err(format!(
                    "The environment variable {n} is not set where ClawDot runs. Set it there (for example in the service's environment file), restart ClawDot, then save this again."
                ));
            }
            json!(n)
        }
        "realtime_usd_per_min_in" | "realtime_usd_per_min_out" => {
            let n = v
                .as_f64()
                .filter(|n| n.is_finite() && (0.0..=100.0).contains(n))
                .ok_or(
                    "The price per minute of audio must be a number of dollars from 0 to 100.",
                )?;
            json!(n)
        }
        "realtime_max_session_secs" => whole(v, "Seconds per realtime conversation", 10, 3600)?,
        "voice" => json!(string("The voice", 64)?),
        "speech_provider" => {
            let t = string("The speech provider", 20)?;
            json!(crate::speech::Dialect::parse(&t)?.name())
        }
        "speech_language" => {
            let t = string("The speech language", 12)?;
            if !t.chars().all(|c| c.is_ascii_alphabetic() || c == '-') {
                return Err(
                    "The speech language is a language code such as en, de or pt-BR.".into(),
                );
            }
            json!(t)
        }
        "listen" => {
            let l = string("The listen address", 64)?;
            let a: SocketAddr = l.parse().map_err(|_| {
                "The listen address must look like 127.0.0.1:3335 or 0.0.0.0:3335 (an IP address, a colon, and a port).".to_owned()
            })?;
            json!(a.to_string())
        }
        "public_url" => {
            let u = string("The public address", 200)?;
            if u.contains(char::is_whitespace)
                || !(u.starts_with("http://") || u.starts_with("https://"))
            {
                return Err(
                    "The public address must start with http:// or https:// and have no spaces, for example http://dot.local:3335.".into(),
                );
            }
            json!(u.trim_end_matches('/'))
        }
        "daily_spend_usd" => {
            let n = v.as_f64().filter(|n| n.is_finite() && (0.0..=100_000.0).contains(n)).ok_or(
                "The daily spend limit must be a number of dollars, zero or more. Leave it empty for no limit.",
            )?;
            json!(n)
        }
        "agent_max_rounds" => whole(v, "Rounds per request", 1, 20)?,
        "agent_max_tool_calls" => whole(v, "Tool calls per request", 1, 20)?,
        "agent_deadline_secs" => whole(v, "Seconds per request", 5, 300)?,
        "agent_tool_timeout_secs" => whole(v, "Seconds per tool call", 1, 120)?,
        "max_actions_per_day" => whole(v, "Actions per day", 0, 100_000)?,
        "max_costly_per_day" => whole(v, "Actions that ask first, per day", 0, 100_000)?,
        "plan_speech" | "progress_speech" | "notes" | "dry_run" | "update_check" => {
            json!(
                v.as_bool()
                    .ok_or_else(|| format!("`{key}` must be on or off (true or false)."))?
            )
        }
        "quiet_start" | "quiet_end" => {
            let t = string("The quiet hours time", 5)?;
            minute_of_day(&t)
                .ok_or("Quiet hours times look like 22:00 or 07:30 (24-hour, local time).")?;
            json!(t)
        }
        "board" => match text(v, "The board")?.as_str() {
            b @ ("auto" | "none") => json!(b),
            _ => return Err("The board setting must be `auto` or `none`.".into()),
        },
        _ => return Err(format!("`{key}` is not a setting.")),
    })
}

/// Checks that involve more than one setting, on the values that would be in effect.
fn check(
    eff: &BTreeMap<&'static str, (Value, Source)>,
    speech_configured: bool,
) -> Result<(), String> {
    let has = |k: &str| eff.get(k).is_some_and(|(v, _)| !v.is_null());
    if has("llm_url") != has("llm_model") {
        return Err(
            "Set both the language model address and the model name, or clear both.".into(),
        );
    }
    if has("quiet_start") != has("quiet_end") {
        return Err("Set both the quiet hours start and end, or clear both.".into());
    }
    if has("quiet_start")
        && eff.get("quiet_start").map(|(v, _)| v) == eff.get("quiet_end").map(|(v, _)| v)
    {
        return Err("Quiet hours cannot start and end at the same time. Use different times, or clear both.".into());
    }
    if has("realtime_url") != has("realtime_model") {
        return Err(
            "Set both the realtime voice address and the model name, or clear both.".into(),
        );
    }
    if eff
        .get("realtime_enabled")
        .is_some_and(|(v, _)| v.as_bool() == Some(true))
        && !has("realtime_url")
    {
        return Err(
            "Realtime voice needs an address and a model name before it can be turned on.".into(),
        );
    }
    // ElevenLabs and Deepgram need no `speech` section: they have their own address (REQ-VOI-021).
    let own_service = eff
        .get("speech_provider")
        .and_then(|(v, _)| v.as_str())
        .is_some_and(|p| p != "openai");
    if (has("voice") || has("speech_language")) && !speech_configured && !own_service {
        return Err(
            "A voice needs a speech provider. Add a `speech` section to the configuration file (see docs/architecture/voice-pipeline.md) and restart, or clear the voice.".into(),
        );
    }
    Ok(())
}

/// All layers, the saved file, and what was running when the program started.
pub struct SettingsState {
    defaults: Layer,
    file: Layer,
    flags: Layer,
    saved: Layer,
    running: Layer,
    store: Option<JsonStore>,
    speech_configured: bool,
    extra: Value,
    env: EnvFn,
}

impl std::fmt::Debug for SettingsState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SettingsState")
            .field("saved", &self.saved.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

impl SettingsState {
    /// Build the state from the three layers a program has before the data directory is read.
    /// `dir` is where `settings.json` lives (`None` keeps everything in memory).
    ///
    /// # Errors
    /// A damaged or newer `settings.json`, an unknown key in it, or a combination that cannot work. The
    /// message says which file to fix. Saved values that are no longer valid are dropped and returned as
    /// warnings so a headless device still starts.
    pub fn open(
        dir: Option<&Path>,
        defaults: Layer,
        file: Layer,
        flags: Layer,
        speech_configured: bool,
        env: EnvFn,
    ) -> Result<(Self, Vec<String>), String> {
        let store = dir.map(|d| JsonStore::new(d.join("settings.json"), SCHEMA));
        let mut loaded = Layer::new();
        let mut warnings = Vec::new();
        if let Some(s) = &store {
            let raw: Layer = s
                .load()
                .map_err(|e| format!("{}: {e}. Fix or remove the file.", s.path().display()))?;
            for (k, v) in raw {
                // A setting that was removed: a file saved by an older version may still hold it, and that is no reason to refuse to start.
                if RETIRED.contains(&k.as_str()) {
                    continue;
                }
                if !is_field(&k) {
                    return Err(format!(
                        "{} has an unknown setting `{k}`. Remove that line, or restore a copy of the file.",
                        s.path().display()
                    ));
                }
                // The key variable may not be set yet (it is the service's job); only the shape matters here.
                match normalize(&k, &v, &|_| Some("x".to_owned())) {
                    Ok(v) => {
                        loaded.insert(k, v);
                    }
                    Err(e) => warnings.push(format!("ignoring saved `{k}`: {e}")),
                }
            }
        }
        let mut st = Self {
            defaults,
            file,
            flags,
            saved: loaded,
            running: Layer::new(),
            store,
            speech_configured,
            extra: Value::Null,
            env,
        };
        let eff = st.effective();
        check(&eff, speech_configured).map_err(|e| {
            format!(
                "{e} (saved settings are in {}; flags and the configuration file also count)",
                st.store
                    .as_ref()
                    .map_or_else(|| "memory".into(), |s| s.path().display().to_string())
            )
        })?;
        st.running = st.values_of(&eff);
        Ok((st, warnings))
    }

    /// Settings that exist only in memory, for tests and for a core without a data directory.
    #[must_use]
    pub fn in_memory(defaults: Layer, speech_configured: bool, env: EnvFn) -> Self {
        let mut st = Self {
            defaults,
            file: Layer::new(),
            flags: Layer::new(),
            saved: Layer::new(),
            running: Layer::new(),
            store: None,
            speech_configured,
            extra: Value::Null,
            env,
        };
        st.running = st.values_of(&st.effective());
        st
    }

    /// Facts shown next to the settings but not changeable here (for example the speech provider's models).
    pub fn set_extra(&mut self, extra: Value) {
        self.extra = extra;
    }

    fn effective(&self) -> BTreeMap<&'static str, (Value, Source)> {
        let mut out = BTreeMap::new();
        for f in &FIELDS {
            let mut cur = (Value::Null, Source::Default);
            for (layer, src) in [
                (&self.defaults, Source::Default),
                (&self.file, Source::File),
                (&self.saved, Source::Saved),
                (&self.flags, Source::Flag),
            ] {
                if let Some(v) = layer.get(f.key).filter(|v| !v.is_null()) {
                    cur = (v.clone(), src);
                }
            }
            out.insert(f.key, cur);
        }
        // With no public address set, snippets use one derived from the listen address.
        if out["public_url"].0.is_null()
            && let Some(l) = out["listen"].0.as_str().and_then(|l| l.parse().ok())
        {
            out.insert(
                "public_url",
                (json!(public_url_from_listen(l).0), Source::Default),
            );
        }
        out
    }

    fn values_of(&self, eff: &BTreeMap<&'static str, (Value, Source)>) -> Layer {
        eff.iter()
            .map(|(k, (v, _))| ((*k).to_owned(), v.clone()))
            .collect()
    }

    /// The effective value of `key` (`Null` when nothing sets it).
    #[must_use]
    pub fn get(&self, key: &str) -> Value {
        self.effective()
            .get(key)
            .map_or(Value::Null, |(v, _)| v.clone())
    }

    /// The effective values of every setting that applies without a restart, for the main loop.
    #[must_use]
    pub fn live_values(&self) -> Map<String, Value> {
        let eff = self.effective();
        FIELDS
            .iter()
            .filter(|f| f.live)
            .map(|f| (f.key.to_owned(), eff[f.key].0.clone()))
            .collect()
    }

    /// Save a change to some settings (REQ-CFG-040). A `null` or empty value clears the saved value so the
    /// layer below shows through. Nothing is saved unless the whole result is valid.
    ///
    /// # Errors
    /// A message that says what is wrong and how to fix it; the saved file and the state are unchanged.
    pub fn change(&mut self, patch: &Value) -> Result<(), String> {
        let Some(patch) = patch.as_object().filter(|p| !p.is_empty()) else {
            return Err("Send the settings to change as a JSON object, for example {\"zone\":\"Europe/Berlin\"}.".into());
        };
        let mut saved = self.saved.clone();
        for (k, v) in patch {
            if !is_field(k) {
                return Err(format!("`{k}` is not a setting."));
            }
            if v.is_null() || v.as_str().is_some_and(|s| s.trim().is_empty()) {
                saved.remove(k);
            } else {
                saved.insert(k.clone(), normalize(k, v, &*self.env)?);
            }
        }
        let before = std::mem::replace(&mut self.saved, saved);
        let eff = self.effective();
        if let Err(e) = check(&eff, self.speech_configured)
            .and_then(|()| self.check_realtime_change(patch, &eff))
        {
            self.saved = before;
            return Err(e);
        }
        if let Some(s) = &self.store
            && let Err(e) = s.save(&self.saved)
        {
            self.saved = before;
            return Err(format!(
                "Could not save the settings file: {e}. Check that the data directory is writable."
            ));
        }
        Ok(())
    }

    /// Turning realtime voice on needs a build that has it and a key to use. Checked only when a change touches
    /// a realtime setting, never at start-up, so a device whose service environment lacks the key still starts
    /// (REQ-RTV-008).
    fn check_realtime_change(
        &self,
        patch: &Map<String, Value>,
        eff: &BTreeMap<&'static str, (Value, Source)>,
    ) -> Result<(), String> {
        if !patch.keys().any(|k| k.starts_with("realtime_"))
            || eff["realtime_enabled"].0.as_bool() != Some(true)
        {
            return Ok(());
        }
        if !crate::rtvoice::RtSettings::built() {
            return Err("This build of ClawDot does not include realtime voice (it needs the `realtime` cargo feature), so it cannot be turned on.".into());
        }
        let key_env = eff["realtime_key_env"]
            .0
            .as_str()
            .unwrap_or(crate::rtvoice::DEFAULT_KEY_ENV);
        if crate::secrets::get(crate::secrets::REALTIME_KEY).is_none()
            && (self.env)(key_env).is_none_or(|v| v.is_empty())
        {
            return Err(format!(
                "Realtime voice needs an API key. Set the environment variable {key_env} where ClawDot runs, restart ClawDot, then turn this on."
            ));
        }
        Ok(())
    }

    /// The settings saved on this device as a portable file (REQ-CFG-065). Only values saved in the portal are
    /// in it, and none is a secret: a key is only ever the name of an environment variable (REQ-CFG-042).
    #[must_use]
    pub fn export(&self) -> Value {
        json!({ "format": EXPORT_FORMAT, "version": 1, "settings": self.saved })
    }

    /// Everything the Settings tab shows. Holds no secret: a key is only ever a variable name and whether
    /// that variable is set.
    #[must_use]
    pub fn json(&self) -> Value {
        let eff = self.effective();
        let mut fields = Map::new();
        let mut restart = Vec::new();
        for f in &FIELDS {
            let (v, src) = &eff[f.key];
            let saved = self.saved.contains_key(f.key);
            if !f.live && self.running.get(f.key) != Some(v) {
                restart.push(f.key);
            }
            fields.insert(
                f.key.to_owned(),
                json!({
                    "value": v, "source": src.as_str(), "live": f.live,
                    "saved": saved, "ignored": saved && *src == Source::Flag,
                }),
            );
        }
        let key_env = eff["llm_key_env"].0.as_str().unwrap_or("");
        let rt = {
            let live = self.values_of(&eff).into_iter().collect::<Map<_, _>>();
            crate::rtvoice::RtSettings::from_live(&live)
        };
        let mut out = json!({
            "fields": fields,
            "restart_needed": restart,
            "speech_configured": self.speech_configured,
            "llm": {
                "configured": !eff["llm_url"].0.is_null() && !eff["llm_model"].0.is_null(),
                "key_present": crate::secrets::get(crate::secrets::LLM_KEY).is_some()
                    || (self.env)(key_env).is_some_and(|v| !v.is_empty()),
                // The first and last few characters of the key pasted into the page, never the key.
                "key_hint": crate::secrets::hint(crate::secrets::LLM_KEY),
            },
            // Never the key: whether realtime voice can run, the provider, and the masked end of a pasted key.
            "realtime": {
                "configured": rt.configured_with(&*self.env),
                "built": crate::rtvoice::RtSettings::built(),
                "provider": crate::rtvoice::PROVIDER,
                "key_hint": crate::secrets::hint(crate::secrets::REALTIME_KEY),
            },
            // The speech service key follows the same rules: only a masked hint, never the key (REQ-VOI-023).
            "speech": {
                "key_present": crate::secrets::get(crate::secrets::SPEECH_KEY).is_some()
                    || self.extra["speech_key_env"]
                        .as_str()
                        .is_some_and(|n| (self.env)(n).is_some_and(|v| !v.is_empty())),
                "key_hint": crate::secrets::hint(crate::secrets::SPEECH_KEY),
            },
        });
        if let (Some(o), Some(e)) = (out.as_object_mut(), self.extra.as_object()) {
            for (k, v) in e {
                o.insert(k.clone(), v.clone());
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
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
            ("board", json!("none")),
        ])
    }

    fn open_in(dir: &Path, file: Layer, flags: Layer) -> SettingsState {
        SettingsState::open(
            Some(dir),
            defaults(),
            file,
            flags,
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

    // REQ-CFG-038
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
    #[test]
    fn a_retired_setting_in_an_old_file_is_skipped() {
        let dir = TempDir::new("retired");
        std::fs::write(
            dir.0.join("settings.json"),
            r#"{"schema":1,"data":{"max_requests_per_day":40,"zone":"Europe/Berlin"}}"#,
        )
        .unwrap();
        let st = open_in(&dir.0, Layer::new(), Layer::new());
        assert_eq!(st.get("zone"), "Europe/Berlin");
    }

    // REQ-CFG-038
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
        assert!(!dir.0.join("settings.tmp").exists());
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
                defaults(),
                Layer::new(),
                Layer::new(),
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
            (json!({"board": "rpi"}), "auto"),
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
    #[test]
    fn live_settings_are_listed_for_the_assistant_and_restart_ones_are_reported() {
        let dir = TempDir::new("restart");
        let mut st = open_in(&dir.0, Layer::new(), Layer::new());
        assert_eq!(st.json()["restart_needed"], json!([]));
        st.change(&json!({"zone": "Europe/Berlin", "listen": "0.0.0.0:4000", "board": "auto"}))
            .unwrap();
        let j = st.json();
        assert_eq!(
            j["restart_needed"],
            json!(["listen", "public_url", "board"])
        );
        assert_eq!(j["fields"]["zone"]["live"], true);
        assert_eq!(j["fields"]["listen"]["live"], false);
        let live = st.live_values();
        assert_eq!(live["zone"], "Europe/Berlin");
        assert!(!live.contains_key("listen") && !live.contains_key("board"));
        // Going back to the running value needs no restart.
        st.change(&json!({"listen": null, "board": null})).unwrap();
        assert_eq!(st.json()["restart_needed"], json!([]));
    }

    // REQ-CFG-016
    #[test]
    fn the_public_address_follows_the_listen_address_until_set() {
        let mut st = SettingsState::in_memory(defaults(), false, env_with(&[]));
        assert_eq!(st.get("public_url"), "http://127.0.0.1:3335");
        st.change(&json!({"listen": "0.0.0.0:4000"})).unwrap();
        assert_eq!(st.get("public_url"), "http://clawdot.local:4000");
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
}
