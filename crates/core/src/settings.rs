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
    // The same rule the session applies when it connects (REQ-SEC-001).
    crate::netpolicy::check_realtime_url(&u)
        .map_err(|e| format!("The realtime voice address is refused: {e}."))?;
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
    // A provider with its own address (ElevenLabs, Deepgram) needs no `speech` section (REQ-VOI-021).
    let own_service = eff
        .get("speech_provider")
        .and_then(|(v, _)| v.as_str())
        .and_then(|p| crate::speech::Dialect::parse(p).ok())
        .is_some_and(|d| d.default_url().is_some());
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
mod tests;
