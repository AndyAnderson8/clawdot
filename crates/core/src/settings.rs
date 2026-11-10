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
pub const FIELDS: [Field; 26] = [
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
        key: "llm_fallback_model",
        live: true,
    },
    Field {
        key: "summary_model",
        live: true,
    },
    Field {
        key: "llm_key_env",
        live: true,
    },
    Field {
        key: "voice",
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
        key: "max_requests_per_day",
        live: true,
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
        "llm_model" | "llm_fallback_model" | "summary_model" => {
            let what = match key {
                "llm_model" => "The model name",
                "llm_fallback_model" => "The backup model name",
                _ => "The summary model name",
            };
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
            if env(&n).is_none_or(|v| v.is_empty()) {
                return Err(format!(
                    "The environment variable {n} is not set where ClawDot runs. Set it there (for example in the service's environment file), restart ClawDot, then save this again."
                ));
            }
            json!(n)
        }
        "voice" => json!(string("The voice", 64)?),
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
        "max_requests_per_day" => {
            let n = v
                .as_u64()
                .filter(|n| (1..=1_000_000).contains(n))
                .ok_or("Requests per day must be a whole number from 1 to 1000000.")?;
            json!(n)
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
        "plan_speech" | "progress_speech" | "notes" | "dry_run" => {
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
    if !has("llm_url") && (has("llm_fallback_model") || has("summary_model")) {
        return Err(
            "The backup and summary models need a language model address and model name. Set those first, or clear these.".into(),
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
    if has("voice") && !speech_configured {
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
        if let Err(e) = check(&eff, self.speech_configured) {
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
        let mut out = json!({
            "fields": fields,
            "restart_needed": restart,
            "speech_configured": self.speech_configured,
            "llm": {
                "configured": !eff["llm_url"].0.is_null() && !eff["llm_model"].0.is_null(),
                "key_present": (self.env)(key_env).is_some_and(|v| !v.is_empty()),
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
            ("max_requests_per_day", json!(500)),
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
        assert_eq!(source(&st, "max_requests_per_day"), "default");
        st.change(&json!({"zone": "Europe/Berlin", "max_requests_per_day": 40}))
            .unwrap();
        assert_eq!(
            (st.get("zone"), source(&st, "zone")),
            (json!("Europe/Berlin"), "saved".into())
        );
        assert_eq!(
            (
                st.get("max_requests_per_day"),
                source(&st, "max_requests_per_day")
            ),
            (json!(40), "saved".into())
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
        st.change(&json!({"max_requests_per_day": ""})).unwrap();
        assert_eq!(source(&st, "max_requests_per_day"), "default");
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
            (json!({"max_requests_per_day": 0}), "1 to 1000000"),
            (json!({"max_requests_per_day": "many"}), "whole number"),
            (json!({"daily_spend_usd": -1}), "zero or more"),
            (json!({"board": "rpi"}), "auto"),
            (json!({"device_name": "a\nb"}), "control"),
            (json!({"llm_url": "ftp://x"}), "https://"),
            (
                json!({"llm_url": "http://example.com/v1", "llm_model": "m"}),
                "unencrypted",
            ),
            (json!({"llm_url": "https://x.example/v1"}), "both"),
            (json!({"llm_fallback_model": "b"}), "need"),
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
            "llm_fallback_model": "c/d", "summary_model": "e/f", "llm_key_env": "MY_KEY"
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
        st.change(&json!({"llm_url": null, "llm_model": null, "llm_fallback_model": null, "summary_model": null}))
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
