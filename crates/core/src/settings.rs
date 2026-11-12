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

use crate::secrets::Secrets;
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
pub use crate::env::EnvFn;

/// The three layers a program has before the data directory is read, lowest first: the built-in defaults, the
/// configuration file, and the command-line flags.
#[derive(Debug, Default)]
pub struct Sources {
    pub defaults: Layer,
    pub file: Layer,
    pub flags: Layer,
}

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

/// The environment lookup a validator may use to check that a key variable exists.
type EnvRef<'a> = &'a dyn Fn(&str) -> Option<String>;

/// What a validator may look at besides the value: the environment, and the keys pasted into the portal.
pub struct Lookup<'a> {
    env: EnvRef<'a>,
    secrets: &'a Secrets,
}

impl std::fmt::Debug for Lookup<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Lookup").finish_non_exhaustive()
    }
}

/// A validator that needs more than a range or a length: it gets the value and the environment lookup, and
/// returns the value in the form that is stored.
pub type Validator = fn(&Value, &Lookup<'_>) -> Result<Value, String>;

/// How a value is checked. Every row names one; the error texts are what the portal shows.
#[derive(Debug, Clone, Copy)]
pub enum Check {
    /// Plain text of at most `max` characters, without control characters.
    Text {
        /// How the setting is named in the message.
        what: &'static str,
        /// Longest allowed, in characters.
        max: usize,
    },
    /// A whole number from `lo` to `hi`.
    Whole {
        /// How the setting is named in the message.
        what: &'static str,
        /// Smallest allowed.
        lo: u64,
        /// Largest allowed.
        hi: u64,
    },
    /// A number from `lo` to `hi`; `err` is the whole message when it is not.
    Number {
        /// Smallest allowed.
        lo: f64,
        /// Largest allowed.
        hi: f64,
        /// The message for a value outside the range.
        err: &'static str,
    },
    /// On or off.
    Flag,
    /// One of a fixed list of words; `err` is the whole message when it is not.
    Choice {
        /// How the setting is named in the message.
        what: &'static str,
        /// The allowed words.
        options: &'static [&'static str],
        /// The message for any other word.
        err: &'static str,
    },
    /// A named function, for rules of its own.
    Custom(Validator),
}

impl Check {
    fn run(self, key: &str, v: &Value, lk: &Lookup<'_>) -> Result<Value, String> {
        match self {
            Self::Text { what, max } => string(v, what, max).map(Value::from),
            Self::Whole { what, lo, hi } => whole(v, what, lo, hi),
            Self::Number { lo, hi, err } => v
                .as_f64()
                .filter(|n| n.is_finite() && (lo..=hi).contains(n))
                .map(|n| json!(n))
                .ok_or_else(|| err.to_owned()),
            Self::Flag => v
                .as_bool()
                .map(Value::from)
                .ok_or_else(|| format!("`{key}` must be on or off (true or false).")),
            Self::Choice { what, options, err } => {
                let t = text(v, what)?;
                options
                    .contains(&t.as_str())
                    .then(|| json!(t))
                    .ok_or_else(|| err.to_owned())
            }
            Self::Custom(f) => f(v, lk),
        }
    }
}

/// What a setting is when nothing sets it.
#[derive(Debug, Clone, Copy)]
pub enum Initial {
    /// Unset: the feature is off or the value is derived elsewhere.
    None,
    /// Text.
    Str(&'static str),
    /// On or off.
    Bool(bool),
    /// A whole number.
    Whole(u64),
    /// A number.
    Number(f64),
    /// Taken from the code that consumes the value, so the two cannot drift.
    From(fn() -> Value),
}

impl Initial {
    fn value(self) -> Option<Value> {
        match self {
            Self::None => None,
            Self::Str(s) => Some(json!(s)),
            Self::Bool(b) => Some(json!(b)),
            Self::Whole(n) => Some(json!(n)),
            Self::Number(n) => Some(json!(n)),
            Self::From(f) => Some(f()),
        }
    }
}

/// Settings that are applied together: when any member changes, the main loop redoes the one piece of work
/// that reads them all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    /// Applied by a handler of its own, keyed by the setting's name.
    Own,
    /// The language model address, model name and key variable.
    Llm,
    /// The voice and speech language.
    Speech,
    /// The agent limits and the speech around a request.
    Agent,
    /// The daily caps and the dry-run switch.
    Policy,
    /// The quiet hours window.
    Quiet,
    /// Realtime voice.
    Realtime,
}

/// One setting: everything the Rust side needs to know about it, in one place. Adding a setting is adding a
/// row to [`FIELDS`] and the code that uses the value.
#[derive(Debug, Clone, Copy)]
pub struct Field {
    /// The key used in `settings.json` and the API.
    pub key: &'static str,
    /// True when the running assistant can switch immediately.
    pub live: bool,
    /// How a new value is checked and normalised.
    pub check: Check,
    /// The value when no layer sets one.
    pub default: Initial,
    /// The settings that are applied together with this one.
    pub group: Group,
}

impl Field {
    /// A setting that applies without a restart.
    const fn live(key: &'static str, check: Check, default: Initial) -> Self {
        Self {
            key,
            live: true,
            check,
            default,
            group: Group::Own,
        }
    }

    /// A setting that needs a restart to apply.
    const fn restart(key: &'static str, check: Check, default: Initial) -> Self {
        Self {
            live: false,
            ..Self::live(key, check, default)
        }
    }

    /// The same setting, applied together with the others of `group`.
    const fn in_group(self, group: Group) -> Self {
        Self { group, ..self }
    }
}

const fn text_of(what: &'static str, max: usize) -> Check {
    Check::Text { what, max }
}

const fn count(what: &'static str, lo: u64, hi: u64) -> Check {
    Check::Whole { what, lo, hi }
}

const PRICE_ERR: &str = "The price per minute of audio must be a number of dollars from 0 to 100.";

const PRICE: Check = Check::Number {
    lo: 0.0,
    hi: 100.0,
    err: PRICE_ERR,
};

/// The price of a million tokens of the language model, for the spend cap (REQ-AST-011).
const TOKEN_PRICE: Check = Check::Number {
    lo: 0.0,
    hi: 1000.0,
    err: "The price per million tokens must be a number of dollars from 0 to 1000. Leave it empty if you do not know it, and 0 if the model is free.",
};

/// Every setting the portal can change (REQ-CFG-037, REQ-CFG-039), in the order the API lists them.
pub const FIELDS: &[Field] = &[
    Field::live(
        "device_name",
        text_of("The device name", 40),
        Initial::Str("ClawDot"),
    ),
    Field::live("zone", Check::Custom(zone_value), Initial::Str("UTC")),
    Field::live(
        "persona",
        Check::Custom(persona_value),
        Initial::Str(crate::assistant::DEFAULT_PERSONA),
    ),
    Field::live("llm_url", Check::Custom(llm_url_value), Initial::None).in_group(Group::Llm),
    Field::live("llm_model", Check::Custom(llm_model_value), Initial::None).in_group(Group::Llm),
    Field::live(
        "llm_key_env",
        Check::Custom(llm_key_env_value),
        Initial::Str("OPENROUTER_API_KEY"),
    )
    .in_group(Group::Llm),
    Field::live("voice", text_of("The voice", 64), Initial::None).in_group(Group::Speech),
    Field::live(
        "speech_language",
        Check::Custom(speech_language_value),
        Initial::None,
    )
    .in_group(Group::Speech),
    // Which service's wire format the speech client speaks; the clients are built at start (REQ-VOI-021).
    Field::restart(
        "speech_provider",
        Check::Custom(speech_provider_value),
        Initial::Str("openai"),
    ),
    Field::restart(
        "listen",
        Check::Custom(listen_value),
        Initial::Str(DEFAULT_LISTEN),
    ),
    Field::restart("public_url", Check::Custom(public_url_value), Initial::None),
    Field::live(
        "daily_spend_usd",
        Check::Number {
            lo: 0.0,
            hi: 100_000.0,
            err: "The daily spend limit must be a number of dollars, zero or more. Leave it empty for no limit.",
        },
        Initial::None,
    ),
    // Dollars per million tokens, so the spend cap works with a provider that reports no cost. Unset is not
    // 0: unset means "price it with the built-in prices", 0 means "free" (usage.rs).
    Field::live("llm_usd_per_mtok_in", TOKEN_PRICE, Initial::None),
    Field::live("llm_usd_per_mtok_out", TOKEN_PRICE, Initial::None),
    Field::live(
        "agent_max_rounds",
        count("Rounds per request", 1, 20),
        Initial::From(|| crate::session::TurnConfig::default().max_tool_loops.into()),
    )
    .in_group(Group::Agent),
    Field::live(
        "agent_max_tool_calls",
        count("Tool calls per request", 1, 20),
        Initial::From(|| crate::session::TurnConfig::default().max_tool_calls.into()),
    )
    .in_group(Group::Agent),
    Field::live(
        "agent_deadline_secs",
        count("Seconds per request", 5, 300),
        Initial::From(|| crate::session::TurnConfig::default().deadline_secs.into()),
    )
    .in_group(Group::Agent),
    Field::live(
        "agent_tool_timeout_secs",
        count("Seconds per tool call", 1, 120),
        Initial::From(|| {
            crate::session::TurnConfig::default()
                .tool_timeout
                .as_secs()
                .into()
        }),
    )
    .in_group(Group::Agent),
    Field::live(
        "plan_speech",
        Check::Flag,
        Initial::From(|| crate::session::TurnConfig::default().plan_speech.into()),
    )
    .in_group(Group::Agent),
    Field::live(
        "progress_speech",
        Check::Flag,
        Initial::From(|| crate::session::TurnConfig::default().progress_speech.into()),
    )
    .in_group(Group::Agent),
    Field::live("notes", Check::Flag, Initial::Bool(false)),
    Field::live("update_check", Check::Flag, Initial::Bool(true)),
    Field::live(
        "max_actions_per_day",
        count("Actions per day", 0, 100_000),
        Initial::From(|| {
            crate::audit::ActionPolicy::default()
                .max_actions_per_day
                .into()
        }),
    )
    .in_group(Group::Policy),
    Field::live(
        "max_costly_per_day",
        count("Actions that ask first, per day", 0, 100_000),
        Initial::From(|| {
            crate::audit::ActionPolicy::default()
                .max_costly_per_day
                .into()
        }),
    )
    .in_group(Group::Policy),
    Field::live(
        "dry_run",
        Check::Flag,
        Initial::From(|| crate::audit::ActionPolicy::default().dry_run.into()),
    )
    .in_group(Group::Policy),
    Field::live(
        "quiet_start",
        Check::Custom(quiet_time_value),
        Initial::None,
    )
    .in_group(Group::Quiet),
    Field::live("quiet_end", Check::Custom(quiet_time_value), Initial::None).in_group(Group::Quiet),
    Field::live(
        "realtime_enabled",
        Check::Custom(realtime_enabled_value),
        Initial::Bool(false),
    )
    .in_group(Group::Realtime),
    Field::live(
        "realtime_url",
        Check::Custom(realtime_url_value),
        Initial::None,
    )
    .in_group(Group::Realtime),
    Field::live(
        "realtime_model",
        Check::Custom(realtime_model_value),
        Initial::None,
    )
    .in_group(Group::Realtime),
    Field::live(
        "realtime_voice",
        text_of("The realtime voice", 64),
        Initial::Str(crate::rtvoice::DEFAULT_VOICE),
    )
    .in_group(Group::Realtime),
    Field::live(
        "realtime_key_env",
        Check::Custom(realtime_key_env_value),
        Initial::Str(crate::rtvoice::DEFAULT_KEY_ENV),
    )
    .in_group(Group::Realtime),
    Field::live(
        "realtime_transcription_model",
        text_of("The transcription model", 100),
        Initial::Str(crate::rtvoice::DEFAULT_TRANSCRIPTION),
    )
    .in_group(Group::Realtime),
    Field::live(
        "realtime_usd_per_min_in",
        PRICE,
        Initial::Number(crate::rtvoice::DEFAULT_USD_IN),
    )
    .in_group(Group::Realtime),
    Field::live(
        "realtime_usd_per_min_out",
        PRICE,
        Initial::Number(crate::rtvoice::DEFAULT_USD_OUT),
    )
    .in_group(Group::Realtime),
    Field::live(
        "realtime_max_session_secs",
        count("Seconds per realtime conversation", 10, 3600),
        Initial::Whole(crate::rtvoice::DEFAULT_MAX_SESSION_SECS),
    )
    .in_group(Group::Realtime),
];

/// The row for `key`.
#[must_use]
pub fn field(key: &str) -> Option<&'static Field> {
    FIELDS.iter().find(|f| f.key == key)
}

/// The smallest and largest value of a whole-number setting, from its row. `None` for any other setting.
#[must_use]
pub fn whole_range(key: &str) -> Option<(u64, u64)> {
    match field(key)?.check {
        Check::Whole { lo, hi, .. } => Some((lo, hi)),
        _ => None,
    }
}

/// Whether a change to `key` applies without a restart. `None` for a key that is not a setting.
#[must_use]
pub fn is_live(key: &str) -> Option<bool> {
    field(key).map(|f| f.live)
}

/// The built-in defaults, one entry for each setting that has one.
#[must_use]
pub fn built_in_defaults() -> Layer {
    FIELDS
        .iter()
        .filter_map(|f| f.default.value().map(|v| (f.key.to_owned(), v)))
        .collect()
}

/// Whether any setting of `group` differs between `now` and `before`.
#[must_use]
pub fn group_changed(group: Group, now: &Map<String, Value>, before: &Map<String, Value>) -> bool {
    FIELDS
        .iter()
        .any(|f| f.group == group && now.get(f.key) != before.get(f.key))
}

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

/// Settings that were removed. A file saved by an older version, or an export taken from one, may still hold one,
/// and it is skipped quietly. (`board` was a choice of `auto` or `none` that nothing ever read; what a device is
/// called and which parts it has now come from the `device` section of the configuration file.)
pub const RETIRED: [&str; 4] = [
    "max_requests_per_day",
    "board",
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
fn normalize(key: &str, v: &Value, lk: &Lookup<'_>) -> Result<Value, String> {
    field(key).map_or_else(
        || Err(format!("`{key}` is not a setting.")),
        |f| f.check.run(key, v, lk),
    )
}

fn string(v: &Value, what: &str, max: usize) -> Result<String, String> {
    plain(&text(v, what)?, what, max)
}

fn zone_value(v: &Value, _: &Lookup<'_>) -> Result<Value, String> {
    let z = string(v, "The time zone", 64)?;
    check_zone(&z)?;
    Ok(json!(z))
}

fn persona_value(v: &Value, _: &Lookup<'_>) -> Result<Value, String> {
    let s = text(v, "The persona")?;
    if s.chars().count() > 4000 {
        return Err("The persona is too long: use at most 4000 characters.".into());
    }
    if s.chars().any(|c| c.is_control() && c != '\n' && c != '\t') {
        return Err("The persona cannot contain control characters.".into());
    }
    Ok(json!(s))
}

fn llm_url_value(v: &Value, _: &Lookup<'_>) -> Result<Value, String> {
    let u = string(v, "The language model address", 300)?;
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
    Ok(json!(u))
}

fn llm_model_value(v: &Value, _: &Lookup<'_>) -> Result<Value, String> {
    let what = "The model name";
    let s = string(v, what, 200)?;
    if s.contains(char::is_whitespace) {
        return Err(format!("{what} cannot contain spaces."));
    }
    Ok(json!(s))
}

/// The name of an environment variable that holds a key. The variable must be set where ClawDot runs unless a
/// key was pasted into the page (`pasted`), which needs no variable at all.
fn key_env_name(v: &Value, env: EnvRef<'_>, what: &str, pasted: bool) -> Result<Value, String> {
    let n = string(v, what, 64)?;
    if !is_env_name(&n) {
        return Err(
            "Enter the NAME of an environment variable (letters, digits, and underscores), not the key itself.".into(),
        );
    }
    if !pasted && env(&n).is_none_or(|v| v.is_empty()) {
        return Err(format!(
            "The environment variable {n} is not set where ClawDot runs. Set it there (in the environment ClawDot is started with, for example its service or supervisor's environment file), restart ClawDot, then save this again."
        ));
    }
    Ok(json!(n))
}

fn llm_key_env_value(v: &Value, lk: &Lookup<'_>) -> Result<Value, String> {
    let pasted = lk.secrets.get(crate::secrets::LLM_KEY).is_some();
    key_env_name(v, lk.env, "The key variable name", pasted)
}

fn realtime_key_env_value(v: &Value, lk: &Lookup<'_>) -> Result<Value, String> {
    let pasted = lk.secrets.get(crate::secrets::REALTIME_KEY).is_some();
    key_env_name(v, lk.env, "The realtime key variable name", pasted)
}

fn realtime_enabled_value(v: &Value, _: &Lookup<'_>) -> Result<Value, String> {
    Ok(json!(v.as_bool().ok_or(
        "Realtime voice must be on or off (true or false)."
    )?))
}

fn realtime_url_value(v: &Value, _: &Lookup<'_>) -> Result<Value, String> {
    let u = string(v, "The realtime voice address", 300)?;
    if u.contains(char::is_whitespace) || !(u.starts_with("wss://") || u.starts_with("ws://")) {
        return Err("The realtime voice address must start with wss:// (or ws:// on your own network), for example wss://api.openai.com/v1/realtime.".into());
    }
    Ok(json!(u))
}

fn realtime_model_value(v: &Value, _: &Lookup<'_>) -> Result<Value, String> {
    let s = string(v, "The realtime model name", 100)?;
    if s.is_empty()
        || !s
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.:".contains(c))
    {
        return Err("The realtime model name can use only letters, digits, and - _ . :".into());
    }
    Ok(json!(s))
}

fn speech_provider_value(v: &Value, _: &Lookup<'_>) -> Result<Value, String> {
    let t = string(v, "The speech provider", 20)?;
    Ok(json!(crate::speech::Dialect::parse(&t)?.name()))
}

fn speech_language_value(v: &Value, _: &Lookup<'_>) -> Result<Value, String> {
    let t = string(v, "The speech language", 12)?;
    if !t.chars().all(|c| c.is_ascii_alphabetic() || c == '-') {
        return Err("The speech language is a language code such as en, de or pt-BR.".into());
    }
    Ok(json!(t))
}

fn listen_value(v: &Value, _: &Lookup<'_>) -> Result<Value, String> {
    let l = string(v, "The listen address", 64)?;
    let a: SocketAddr = l.parse().map_err(|_| {
        "The listen address must look like 127.0.0.1:3335 or 0.0.0.0:3335 (an IP address, a colon, and a port).".to_owned()
    })?;
    Ok(json!(a.to_string()))
}

fn public_url_value(v: &Value, _: &Lookup<'_>) -> Result<Value, String> {
    let u = string(v, "The public address", 200)?;
    if u.contains(char::is_whitespace) || !(u.starts_with("http://") || u.starts_with("https://")) {
        return Err(
            "The public address must start with http:// or https:// and have no spaces, for example http://clawdot.local:3335.".into(),
        );
    }
    Ok(json!(u.trim_end_matches('/')))
}

fn quiet_time_value(v: &Value, _: &Lookup<'_>) -> Result<Value, String> {
    let t = string(v, "The quiet hours time", 5)?;
    minute_of_day(&t).ok_or("Quiet hours times look like 22:00 or 07:30 (24-hour, local time).")?;
    Ok(json!(t))
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
    secrets: Secrets,
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
        sources: Sources,
        speech_configured: bool,
        env: EnvFn,
    ) -> Result<(Self, Vec<String>), String> {
        let Sources {
            defaults,
            file,
            flags,
        } = sources;
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
                if field(&k).is_none() {
                    return Err(format!(
                        "{} has an unknown setting `{k}`. Remove that line, or restore a copy of the file.",
                        s.path().display()
                    ));
                }
                // The key variable may not be set yet (it is the service's job); only the shape matters here.
                let lk = Lookup {
                    env: &|_| Some("x".to_owned()),
                    secrets: &Secrets::none(),
                };
                match normalize(&k, &v, &lk) {
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
            secrets: Secrets::none(),
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
            secrets: Secrets::none(),
        };
        st.running = st.values_of(&st.effective());
        st
    }

    /// Where the keys pasted into the portal are looked up (the owning core hands over its own).
    pub fn set_secrets(&mut self, secrets: Secrets) {
        self.secrets = secrets;
    }

    /// Facts shown next to the settings but not changeable here (for example the speech provider's models).
    pub fn set_extra(&mut self, extra: Value) {
        self.extra = extra;
    }

    fn effective(&self) -> BTreeMap<&'static str, (Value, Source)> {
        let mut out = BTreeMap::new();
        for f in FIELDS {
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
            // An export from an older version may carry a removed setting; it is dropped, not an error.
            if RETIRED.contains(&k.as_str()) {
                continue;
            }
            if field(k).is_none() {
                return Err(format!("`{k}` is not a setting."));
            }
            if v.is_null() || v.as_str().is_some_and(|s| s.trim().is_empty()) {
                saved.remove(k);
            } else {
                let lk = Lookup {
                    env: &*self.env,
                    secrets: &self.secrets,
                };
                saved.insert(k.clone(), normalize(k, v, &lk)?);
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
        if !patch
            .keys()
            .any(|k| field(k).is_some_and(|f| f.group == Group::Realtime))
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
        if self.secrets.get(crate::secrets::REALTIME_KEY).is_none()
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
        for f in FIELDS {
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
                "key_present": self.secrets.get(crate::secrets::LLM_KEY).is_some()
                    || (self.env)(key_env).is_some_and(|v| !v.is_empty()),
                // The first and last few characters of the key pasted into the page, never the key.
                "key_hint": self.secrets.hint(crate::secrets::LLM_KEY),
            },
            // Never the key: whether realtime voice can run, the provider, and the masked end of a pasted key.
            "realtime": {
                "configured": rt.configured_with(&self.secrets, &*self.env),
                "built": crate::rtvoice::RtSettings::built(),
                "provider": crate::rtvoice::PROVIDER,
                "key_hint": self.secrets.hint(crate::secrets::REALTIME_KEY),
            },
            // The speech service key follows the same rules: only a masked hint, never the key (REQ-VOI-023).
            "speech": {
                "key_present": self.secrets.get(crate::secrets::SPEECH_KEY).is_some()
                    || self.extra["speech_key_env"]
                        .as_str()
                        .is_some_and(|n| (self.env)(n).is_some_and(|v| !v.is_empty())),
                "key_hint": self.secrets.hint(crate::secrets::SPEECH_KEY),
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
