//! The scenario file format (REQ-AST-016) and loading it from disk.
//!
//! One JSON file per scenario. Unknown keys are errors, so a typo in an expectation cannot silently turn a
//! check off.

use clawdot_core::session::Tier;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// The format version this build reads.
pub const VERSION: u32 = 1;
/// Default start time: 2026-11-09 12:00 UTC.
pub const DEFAULT_NOW: i64 = 1_794_225_600;

/// The tools that run for real, through the product's own `Assistant`, against a fixed clock.
pub const BUILTIN_TOOLS: [&str; 8] = [
    "set_timer",
    "list_timers",
    "cancel_timers",
    "set_alarm",
    "cancel_alarms",
    "set_reminder",
    "set_reminder_at",
    "current_time",
];

/// One scenario.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    /// Format version (must be [`VERSION`]).
    pub version: u32,
    /// Unique name; also the file name without `.json`.
    pub id: String,
    /// `UC1` to `UC10`, `INJ` for the injection set, `POL` for policy checks.
    #[serde(default)]
    pub use_case: String,
    /// What the scenario shows.
    #[serde(default)]
    pub description: String,
    /// Free labels for filtering.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Clock, caps and switches.
    #[serde(default)]
    pub setup: Setup,
    /// Built-in tools that really run (see [`BUILTIN_TOOLS`]).
    #[serde(default)]
    pub builtin_tools: Vec<String>,
    /// Shared fixture files in `evals/tools/`, by name without `.json`.
    #[serde(default)]
    pub tool_sets: Vec<String>,
    /// Fixture tools; one with the same name as one in a tool set replaces it.
    #[serde(default)]
    pub tools: Vec<ToolFixture>,
    /// A recorded transcript supplying the model's replies for turns that give none.
    #[serde(default)]
    pub model: Option<ModelSource>,
    /// The conversation.
    pub turns: Vec<Turn>,
    /// Things that fail the scenario whenever they happen.
    #[serde(default)]
    pub forbidden: Forbidden,
    /// Budgets for the whole scenario.
    #[serde(default)]
    pub limits: Limits,
}

/// Clock, caps and switches.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Setup {
    /// Start time (Unix seconds).
    #[serde(default = "default_now")]
    pub now: i64,
    /// Seconds between turns unless a turn says otherwise.
    #[serde(default = "default_gap")]
    pub turn_gap_secs: i64,
    /// Local day number the caps count by.
    #[serde(default = "default_day")]
    pub day: i64,
    /// Dry-run mode (REQ-SEC-016).
    #[serde(default)]
    pub dry_run: bool,
    /// Daily cap on actions (REQ-SEC-015); default as shipped.
    #[serde(default)]
    pub max_actions_per_day: Option<u32>,
    /// Daily cap on costly actions; default as shipped.
    #[serde(default)]
    pub max_costly_per_day: Option<u32>,
    /// Offer destructive tools (off as shipped).
    #[serde(default)]
    pub allow_destructive: bool,
    /// Tool rounds per turn; default as shipped.
    #[serde(default)]
    pub max_tool_loops: Option<usize>,
    /// Replace the system prompt (default: the product's).
    #[serde(default)]
    pub system: Option<String>,
}

fn default_now() -> i64 {
    DEFAULT_NOW
}
fn default_gap() -> i64 {
    5
}
fn default_day() -> i64 {
    1
}

impl Default for Setup {
    fn default() -> Self {
        Self {
            now: DEFAULT_NOW,
            turn_gap_secs: 5,
            day: 1,
            dry_run: false,
            max_actions_per_day: None,
            max_costly_per_day: None,
            allow_destructive: false,
            max_tool_loops: None,
            system: None,
        }
    }
}

/// A fake tool: what the model is offered and what a call returns. Real servers are never called.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolFixture {
    /// Tool name as the model sees it.
    pub name: String,
    /// Description offered to the model.
    #[serde(default)]
    pub description: String,
    /// JSON schema of the parameters.
    #[serde(default = "empty_schema")]
    pub parameters: Value,
    /// The tier the administrator gave it (default costly, like an unclassified tool).
    #[serde(default = "costly")]
    pub tier: Tier,
    /// The tier it would have had before the administrator lowered it (REQ-SEC-012).
    #[serde(default)]
    pub lowered_from: Option<Tier>,
    /// A tier that depends on one argument, like `inbox_reply`'s `kind`.
    #[serde(default)]
    pub tier_by_arg: Option<TierByArg>,
    /// Whether its results are text written by someone else (default yes).
    #[serde(default = "yes")]
    pub untrusted: bool,
    /// The result of every call.
    #[serde(default)]
    pub result: Option<String>,
    /// Results in order; the last one repeats.
    #[serde(default)]
    pub results: Vec<String>,
    /// Fail every call with this message.
    #[serde(default)]
    pub error: Option<String>,
}

fn empty_schema() -> Value {
    json!({"type": "object", "properties": {}})
}
fn costly() -> Tier {
    Tier::Costly
}
fn yes() -> bool {
    true
}

/// Tier chosen by the value of one argument.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TierByArg {
    /// The argument's name.
    pub arg: String,
    /// Tier for each value; any other value uses the tool's own tier.
    pub values: std::collections::BTreeMap<String, Tier>,
}

/// Where the model's replies come from when a turn gives none.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSource {
    /// Path of a transcript file, relative to the scenario file.
    pub transcript: String,
}

/// A recorded transcript: the model's replies, turn by turn.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Transcript {
    /// Format version.
    pub version: u32,
    /// The replies of each turn.
    pub turns: Vec<Vec<Reply>>,
}

/// Who started a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    /// The user spoke.
    #[default]
    User,
    /// An event, a schedule or a rule; `say` is its (untrusted) text.
    Event,
}

/// One turn of the conversation.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Turn {
    /// The user's words, or the event's text.
    pub say: String,
    /// Who started it.
    #[serde(default)]
    pub trigger: Trigger,
    /// For an event: its source (an endpoint name), as the audit log records it.
    #[serde(default)]
    pub source: Option<String>,
    /// Seconds since the previous turn.
    #[serde(default)]
    pub after_secs: Option<i64>,
    /// The scripted model replies for this turn, in order (replay mode).
    #[serde(default)]
    pub model: Option<Vec<Reply>>,
    /// What must hold after this turn.
    #[serde(default)]
    pub expect: Expect,
}

/// One scripted model reply.
#[derive(Debug, Clone, Deserialize, serde::Serialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    /// Text to say (with calls: the model's words alongside them).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Tool calls requested in this round.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub calls: Vec<CallSpec>,
    /// Fail the request with this provider error instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Cost the provider reports for this request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

/// One scripted tool call.
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct CallSpec {
    /// Call id (default `call_<n>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Tool name; may name a tool that does not exist.
    pub name: String,
    /// Arguments as JSON.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Value>,
    /// Arguments as raw text, for malformed JSON.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args_raw: Option<String>,
}

/// How a turn must end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ends {
    /// The model answered.
    Answered,
    /// The host asked the user to confirm an action.
    Held,
    /// The tool loop hit its bound.
    ToolLoopLimit,
    /// A usage limit stopped it.
    Limit,
    /// The model could not be reached.
    Failed,
}

/// What must hold after a turn.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expect {
    /// How the turn ends.
    #[serde(default)]
    pub ends: Option<Ends>,
    /// The calls that ran this turn, in order, and no others. Arguments match as a subset.
    #[serde(default)]
    pub calls: Option<Vec<ExpectedCall>>,
    /// Tools that must not run this turn.
    #[serde(default)]
    pub must_not_call: Vec<String>,
    /// An action must be held for the user's yes, exactly this one.
    #[serde(default)]
    pub held: Option<ExpectedHeld>,
    /// Nothing may be held after this turn.
    #[serde(default)]
    pub no_held: bool,
    /// The speech must contain each of these (any case).
    #[serde(default)]
    pub speech_contains: Vec<String>,
    /// The speech must contain none of these (any case).
    #[serde(default)]
    pub speech_not_contains: Vec<String>,
    /// At most this many sentences.
    #[serde(default)]
    pub max_sentences: Option<usize>,
    /// At most this many characters.
    #[serde(default)]
    pub max_chars: Option<usize>,
    /// The speech ends with a question mark.
    #[serde(default)]
    pub speech_is_question: bool,
    /// Audit records for this turn: each listed one must appear, in this order (others may be between).
    #[serde(default)]
    pub audit: Vec<ExpectedAudit>,
    /// The model's last request this turn must contain each of these.
    #[serde(default)]
    pub model_saw: Vec<String>,
    /// The model's last request this turn must contain none of these.
    #[serde(default)]
    pub model_not_saw: Vec<String>,
    /// The audit log, as a whole, must not contain these (for secrets).
    #[serde(default)]
    pub audit_text_not_contains: Vec<String>,
}

/// A call that must have run.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedCall {
    /// Tool name.
    pub tool: String,
    /// Argument matcher (see `matcher`); omitted means any.
    #[serde(default)]
    pub args: Option<Value>,
}

/// An action that must be held.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedHeld {
    /// Tool name.
    pub tool: String,
    /// Argument matcher.
    #[serde(default)]
    pub args: Option<Value>,
    /// The host's question must contain these.
    #[serde(default)]
    pub speech_contains: Vec<String>,
}

/// An audit record that must exist.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedAudit {
    /// Tool name.
    pub tool: String,
    /// Decision string (for example `held`, `ran`, `blocked:cap`).
    pub decision: String,
    /// Argument matcher for the redacted arguments.
    #[serde(default)]
    pub args: Option<Value>,
}

/// Things that fail a scenario whenever they happen, in any turn.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Forbidden {
    /// Tools that must never run.
    #[serde(default)]
    pub tools: Vec<String>,
    /// Text the assistant must never say (any case).
    #[serde(default)]
    pub speech_contains: Vec<String>,
}

/// Budgets for the whole scenario.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    /// Most model requests in all.
    #[serde(default)]
    pub max_model_requests: Option<u32>,
    /// Most tool calls run in all.
    #[serde(default)]
    pub max_tool_calls: Option<usize>,
}

/// A scenario with its files resolved.
#[derive(Debug, Clone)]
pub struct Loaded {
    /// The scenario, with tool sets merged into `tools` and the transcript merged into the turns.
    pub scenario: Scenario,
    /// Where it came from.
    pub path: PathBuf,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolSetFile {
    tools: Vec<ToolFixture>,
}

fn read(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Load one scenario file and resolve its tool sets and transcript.
///
/// # Errors
/// A message naming the file and what is wrong.
pub fn load_file(path: &Path) -> Result<Loaded, String> {
    let text = read(path)?;
    let mut s: Scenario =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let err = |m: String| format!("{}: {m}", path.display());
    if s.version != VERSION {
        return Err(err(format!(
            "version {} is not supported (this build reads {VERSION})",
            s.version
        )));
    }
    let stem = path.file_stem().and_then(|f| f.to_str()).unwrap_or("");
    if s.id != stem {
        return Err(err(format!(
            "id `{}` must match the file name `{stem}`",
            s.id
        )));
    }
    // Tool sets live in ../tools/ next to the scenarios directory.
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let mut tools: Vec<ToolFixture> = Vec::new();
    for set in &s.tool_sets {
        if !set
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(err(format!("tool set name `{set}` is not a plain name")));
        }
        let file = dir.join("..").join("tools").join(format!("{set}.json"));
        let parsed: ToolSetFile =
            serde_json::from_str(&read(&file)?).map_err(|e| format!("{}: {e}", file.display()))?;
        tools.extend(parsed.tools);
    }
    for t in std::mem::take(&mut s.tools) {
        tools.retain(|x| x.name != t.name);
        tools.push(t);
    }
    s.tools = tools;
    if let Some(m) = &s.model {
        let file = dir.join(&m.transcript);
        let t: Transcript =
            serde_json::from_str(&read(&file)?).map_err(|e| format!("{}: {e}", file.display()))?;
        if t.version != VERSION {
            return Err(err(format!(
                "transcript {} has version {}",
                m.transcript, t.version
            )));
        }
        for (i, turn) in s.turns.iter_mut().enumerate() {
            if turn.model.is_none() {
                turn.model = t.turns.get(i).cloned();
            }
        }
    }
    validate(&s).map_err(err)?;
    Ok(Loaded {
        scenario: s,
        path: path.to_owned(),
    })
}

/// Load every `*.json` in `dir`, sorted by name.
///
/// # Errors
/// The first problem found (unreadable directory, bad file, duplicate id).
pub fn load_dir(dir: &Path) -> Result<Vec<Loaded>, String> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter(|p| {
            !p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(".transcript.json"))
        })
        .collect();
    files.sort();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for f in files {
        let l = load_file(&f)?;
        if !seen.insert(l.scenario.id.clone()) {
            return Err(format!("duplicate scenario id `{}`", l.scenario.id));
        }
        out.push(l);
    }
    Ok(out)
}

/// Catch mistakes that would otherwise make a check meaningless.
fn validate(s: &Scenario) -> Result<(), String> {
    if s.turns.is_empty() {
        return Err("a scenario needs at least one turn".into());
    }
    for b in &s.builtin_tools {
        if !BUILTIN_TOOLS.contains(&b.as_str()) {
            return Err(format!("`{b}` is not a built-in tool that can run here"));
        }
    }
    let mut known: HashSet<&str> = s.builtin_tools.iter().map(String::as_str).collect();
    for t in &s.tools {
        if !known.insert(t.name.as_str()) {
            return Err(format!("tool `{}` is defined twice", t.name));
        }
        if t.result.is_some() as u8 + u8::from(!t.results.is_empty()) + u8::from(t.error.is_some())
            > 1
        {
            return Err(format!(
                "tool `{}`: give only one of result, results, error",
                t.name
            ));
        }
        if t.lowered_from.is_some_and(|b| b < t.tier) {
            return Err(format!(
                "tool `{}`: lowered_from must not be below its tier",
                t.name
            ));
        }
    }
    for (i, turn) in s.turns.iter().enumerate() {
        let at = |m: String| format!("turn {}: {m}", i + 1);
        if turn.say.trim().is_empty() {
            return Err(at("`say` is empty".into()));
        }
        if turn.trigger == Trigger::User && turn.source.is_some() {
            return Err(at("`source` is only for event turns".into()));
        }
        let e = &turn.expect;
        let mut names: Vec<&str> = Vec::new();
        names.extend(e.calls.iter().flatten().map(|c| c.tool.as_str()));
        names.extend(e.held.iter().map(|h| h.tool.as_str()));
        for n in names {
            if !known.contains(n) {
                return Err(at(format!("`{n}` is not a tool of this scenario")));
            }
        }
        if e.held.is_some() && e.no_held {
            return Err(at("`held` and `no_held` contradict each other".into()));
        }
    }
    Ok(())
}
