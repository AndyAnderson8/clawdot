//! Declarative adapters: an agent's native hook payload in, a normalised event out.
//!
//! An adapter is data (JSON), never code (ADR-0010, REQ-EXT-004), so supporting another agent means
//! writing a small mapping file. A rule fires when every `when` pointer equals the expected string; the
//! first matching rule wins. Field templates mix literal text with `{...}` expressions:
//!
//! ```text
//! {/cwd | basename}                       a JSON pointer, then filters
//! {/summary ?? /text ?? 'Update'}         first non-empty of several pointers or literals
//! ```
//!
//! Filters: `basename`, `trim`, `lower`, `truncate:N`. A literal `{` is written `{{`.
//!
//! An adapter file may also carry `setup`, a list of lines printed when `--create-endpoint` makes an endpoint with
//! this adapter (how to point the sending program at it). In them `{public}`, `{endpoint}` and `{token}` stand for
//! the public address, the endpoint's name and its token; nothing else in them is interpreted.

use super::event::{EventKind, MAX_REF, MAX_TEXT, MAX_TITLE, Priority, truncate_chars};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt;

/// The adapter definitions embedded in the binary (the files in `crates/core/adapters/`). Each is loaded the
/// same way and known by the `name` inside it, which endpoints saved on disk refer to; to ship another, add its
/// file here.
const BUILTIN: &[&str] = &[
    include_str!("../../adapters/claude-code.json"),
    include_str!("../../adapters/generic.json"),
];

/// An adapter definition that cannot be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterError(pub String);

impl fmt::Display for AdapterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "bad adapter: {}", self.0)
    }
}

impl std::error::Error for AdapterError {}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Filter {
    Basename,
    Trim,
    Lower,
    Truncate(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Alt {
    Pointer(String),
    Literal(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Expr {
    alts: Vec<Alt>,
    filters: Vec<Filter>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    Text(String),
    Expr(Expr),
}

/// A compiled field template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template(Vec<Part>);

impl Template {
    /// Compile a template string.
    ///
    /// # Errors
    /// [`AdapterError`] for an unclosed `{`, an unknown filter, or an empty expression.
    pub fn parse(s: &str) -> Result<Self, AdapterError> {
        let mut parts = Vec::new();
        let mut text = String::new();
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '{' {
                if chars.peek() == Some(&'{') {
                    chars.next();
                    text.push('{');
                    continue;
                }
                let mut body = String::new();
                let mut closed = false;
                for ch in chars.by_ref() {
                    if ch == '}' {
                        closed = true;
                        break;
                    }
                    body.push(ch);
                }
                if !closed {
                    return Err(AdapterError(format!("unclosed `{{` in `{s}`")));
                }
                if !text.is_empty() {
                    parts.push(Part::Text(std::mem::take(&mut text)));
                }
                parts.push(Part::Expr(parse_expr(&body)?));
            } else {
                text.push(c);
            }
        }
        if !text.is_empty() {
            parts.push(Part::Text(text));
        }
        Ok(Self(parts))
    }

    /// Fill the template from a payload.
    #[must_use]
    pub fn render(&self, payload: &Value) -> String {
        let mut out = String::new();
        for part in &self.0 {
            match part {
                Part::Text(t) => out.push_str(t),
                Part::Expr(e) => out.push_str(&e.eval(payload)),
            }
        }
        out
    }
}

fn parse_expr(body: &str) -> Result<Expr, AdapterError> {
    let mut pieces = body.split('|').map(str::trim);
    let head = pieces.next().unwrap_or("");
    let mut alts = Vec::new();
    for alt in head.split("??").map(str::trim) {
        if let Some(p) = alt.strip_prefix('/') {
            alts.push(Alt::Pointer(format!("/{p}")));
        } else if alt.len() >= 2
            && ((alt.starts_with('\'') && alt.ends_with('\''))
                || (alt.starts_with('"') && alt.ends_with('"')))
        {
            alts.push(Alt::Literal(alt[1..alt.len() - 1].to_owned()));
        } else {
            return Err(AdapterError(format!(
                "`{alt}` must be a JSON pointer starting with `/` or a quoted literal"
            )));
        }
    }
    let mut filters = Vec::new();
    for f in pieces {
        filters.push(match f.split_once(':') {
            None if f == "basename" => Filter::Basename,
            None if f == "trim" => Filter::Trim,
            None if f == "lower" => Filter::Lower,
            Some(("truncate", n)) => Filter::Truncate(
                n.trim()
                    .parse()
                    .map_err(|_| AdapterError(format!("truncate needs a number, got `{n}`")))?,
            ),
            _ => return Err(AdapterError(format!("unknown filter `{f}`"))),
        });
    }
    Ok(Expr { alts, filters })
}

fn value_text(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

impl Expr {
    fn eval(&self, payload: &Value) -> String {
        let mut value = String::new();
        for alt in &self.alts {
            let candidate = match alt {
                Alt::Pointer(p) => payload.pointer(p).map(value_text).unwrap_or_default(),
                Alt::Literal(l) => l.clone(),
            };
            if !candidate.trim().is_empty() {
                value = candidate;
                break;
            }
        }
        for f in &self.filters {
            value = match f {
                Filter::Basename => value
                    .trim_end_matches(['/', '\\'])
                    .rsplit(['/', '\\'])
                    .next()
                    .unwrap_or("")
                    .to_owned(),
                Filter::Trim => value.trim().to_owned(),
                Filter::Lower => value.to_lowercase(),
                Filter::Truncate(n) => truncate_chars(&value, *n),
            };
        }
        value
    }
}

#[derive(Debug, Deserialize)]
struct RuleSpec {
    #[serde(default)]
    when: BTreeMap<String, String>,
    kind: String,
    #[serde(default = "default_priority")]
    priority: String,
    title: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    session_ref: String,
}

fn default_priority() -> String {
    "normal".into()
}

#[derive(Debug, Deserialize)]
struct AdapterSpec {
    name: String,
    source: String,
    /// The lines printed when an endpoint is created with this adapter, with `{public}`, `{endpoint}` and
    /// `{token}` standing for the address, the endpoint's name and its token.
    #[serde(default)]
    setup: Vec<String>,
    rules: Vec<RuleSpec>,
}

#[derive(Debug, Clone)]
struct Rule {
    when: Vec<(String, String)>,
    kind: Template,
    priority: Template,
    title: Template,
    text: Template,
    status: Template,
    session_ref: Template,
}

/// A compiled adapter.
#[derive(Debug, Clone)]
pub struct Adapter {
    /// Name used to select it on an endpoint.
    pub name: String,
    /// Human-readable origin put on events.
    pub source: String,
    setup: Vec<String>,
    rules: Vec<Rule>,
}

/// `text` with each `{public}`, `{endpoint}` and `{token}` replaced. One pass over the template, so a value that
/// itself contains one of the placeholders is not expanded again.
fn fill_setup(text: &str, public: &str, endpoint: &str, token: &str) -> String {
    let mut out = String::with_capacity(text.len() + public.len() + endpoint.len() + token.len());
    let mut rest = text;
    while let Some(at) = rest.find('{') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let (value, len) = if rest.starts_with("{public}") {
            (public, "{public}".len())
        } else if rest.starts_with("{endpoint}") {
            (endpoint, "{endpoint}".len())
        } else if rest.starts_with("{token}") {
            (token, "{token}".len())
        } else {
            ("{", 1)
        };
        out.push_str(value);
        rest = &rest[len..];
    }
    out.push_str(rest);
    out
}

/// An event's fields as produced by an adapter, before an id and timestamps are assigned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mapped {
    /// Human-readable origin.
    pub source: String,
    /// What it is about.
    pub kind: EventKind,
    /// Headline.
    pub title: String,
    /// Full text.
    pub text: String,
    /// Outcome label.
    pub status: Option<String>,
    /// Urgency.
    pub priority: Priority,
    /// Agent session reference.
    pub session_ref: Option<String>,
    /// A spoken reply must not be sent to this event (a one-way announcement, REQ-INB-023).
    pub no_reply: bool,
    /// Drop it if it is still unspoken after this many seconds (REQ-INB-023).
    pub ttl_secs: Option<i64>,
}

impl Adapter {
    /// Compile an adapter from its JSON definition.
    ///
    /// # Errors
    /// [`AdapterError`] for malformed JSON, no rules, or a bad template.
    pub fn from_json(json: &str) -> Result<Self, AdapterError> {
        let spec: AdapterSpec =
            serde_json::from_str(json).map_err(|e| AdapterError(e.to_string()))?;
        if spec.rules.is_empty() {
            return Err(AdapterError(format!(
                "adapter `{}` has no rules",
                spec.name
            )));
        }
        let mut rules = Vec::new();
        for r in spec.rules {
            if let Some(bad) = r.when.keys().find(|k| !k.starts_with('/')) {
                return Err(AdapterError(format!(
                    "`when` key `{bad}` must be a JSON pointer"
                )));
            }
            rules.push(Rule {
                when: r.when.into_iter().collect(),
                kind: Template::parse(&r.kind)?,
                priority: Template::parse(&r.priority)?,
                title: Template::parse(&r.title)?,
                text: Template::parse(&r.text)?,
                status: Template::parse(&r.status)?,
                session_ref: Template::parse(&r.session_ref)?,
            });
        }
        Ok(Self {
            name: spec.name,
            source: spec.source,
            setup: spec.setup,
            rules,
        })
    }

    /// What to tell the person who has just created an endpoint with this adapter: how to point the sending
    /// program at `public`/hook/`endpoint`. `None` when the definition has no set-up text.
    #[must_use]
    pub fn setup_text(&self, public: &str, endpoint: &str, token: &str) -> Option<String> {
        (!self.setup.is_empty())
            .then(|| fill_setup(&self.setup.join("\n"), public, endpoint, token))
    }

    /// The built-in adapters, one result per embedded definition. A definition that does not parse is a defect
    /// of the build, which the tests catch; the caller decides what to do with the error at run time.
    pub fn builtins() -> impl Iterator<Item = Result<Self, AdapterError>> {
        BUILTIN.iter().map(|json| Self::from_json(json))
    }

    /// Map a payload to an event, or `None` if no rule matches. Fields are clamped to their limits.
    #[must_use]
    pub fn map(&self, payload: &Value) -> Option<Mapped> {
        let rule = self.rules.iter().find(|r| {
            r.when.iter().all(|(ptr, want)| {
                payload.pointer(ptr).map(value_text).as_deref() == Some(want.as_str())
            })
        })?;
        let non_empty = |s: String| {
            let s = s.trim().to_owned();
            (!s.is_empty()).then_some(s)
        };
        let title = non_empty(rule.title.render(payload)).unwrap_or_else(|| self.source.clone());
        Some(Mapped {
            source: truncate_chars(&self.source, MAX_REF),
            kind: EventKind::parse(&rule.kind.render(payload)),
            priority: Priority::parse(&rule.priority.render(payload)),
            title: truncate_chars(&title, MAX_TITLE),
            text: truncate_chars(rule.text.render(payload).trim(), MAX_TEXT),
            status: non_empty(rule.status.render(payload)).map(|s| truncate_chars(&s, MAX_REF)),
            session_ref: non_empty(rule.session_ref.render(payload))
                .map(|s| truncate_chars(&s, MAX_REF)),
            no_reply: false,
            ttl_secs: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The embedded adapter called `name`.
    fn builtin(name: &str) -> Adapter {
        Adapter::builtins()
            .map(Result::unwrap)
            .find(|a| a.name == name)
            .unwrap_or_else(|| panic!("no built-in adapter `{name}`"))
    }

    fn claude() -> Adapter {
        builtin("claude-code")
    }

    // A definition that does not parse would otherwise be left out at run time; it must fail here instead.
    #[test]
    fn every_built_in_adapter_parses_and_has_its_own_name() {
        let all: Vec<Adapter> = Adapter::builtins()
            .map(|a| a.unwrap_or_else(|e| panic!("{e}")))
            .collect();
        assert_eq!(all.len(), BUILTIN.len());
        let mut names: Vec<&str> = all.iter().map(|a| a.name.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), all.len(), "two built-ins share a name");
        // Endpoints saved on disk refer to these names, so they must not change.
        assert_eq!(names, ["claude-code", "generic"]);
    }

    #[test]
    fn a_stop_hook_becomes_a_done_event_with_a_session_ref() {
        let payload = json!({
            "session_id": "abc-123",
            "cwd": "/home/user/project",
            "hook_event_name": "Stop",
            "permission_mode": "default",
            "last_assistant_message": "All 12 tests pass. I added the timer tool."
        });
        let m = claude().map(&payload).unwrap();
        assert_eq!(m.kind, EventKind::Done);
        assert_eq!(m.source, "Claude Code");
        assert_eq!(m.title, "project finished");
        assert_eq!(m.text, "All 12 tests pass. I added the timer tool.");
        assert_eq!(m.session_ref.as_deref(), Some("abc-123"));
        assert_eq!(m.priority, Priority::Normal);
    }

    #[test]
    fn permission_prompts_are_urgent_and_want_a_reply() {
        let payload = json!({
            "session_id": "s1", "cwd": "/home/a/proj", "hook_event_name": "Notification",
            "notification_type": "permission_prompt", "message": "Claude needs your permission to run rm"
        });
        let m = claude().map(&payload).unwrap();
        assert_eq!(
            (m.kind, m.priority),
            (EventKind::NeedsInput, Priority::Urgent)
        );
        assert!(m.kind.wants_reply());
        assert_eq!(m.title, "proj needs permission");
    }

    #[test]
    fn idle_prompts_and_completions_map_sensibly() {
        let idle = json!({"hook_event_name":"Notification","notification_type":"idle_prompt","cwd":"/x/p","session_id":"s"});
        assert_eq!(claude().map(&idle).unwrap().kind, EventKind::NeedsInput);
        let done = json!({"hook_event_name":"Notification","notification_type":"agent_completed","cwd":"/x/p","message":"done","session_id":"s"});
        assert_eq!(claude().map(&done).unwrap().kind, EventKind::Done);
    }

    #[test]
    fn unknown_hook_events_fall_to_a_low_priority_info_rule() {
        let m = claude()
            .map(&json!({"hook_event_name": "PreCompact", "cwd": "/x/p"}))
            .unwrap();
        assert_eq!((m.kind, m.priority), (EventKind::Info, Priority::Low));
    }

    #[test]
    fn the_generic_adapter_takes_simple_fields() {
        let g = builtin("generic");
        let m = g
            .map(&json!({"title": "Backup finished", "summary": "12 GB copied", "kind": "done", "priority": "low", "source": "nightly"}))
            .unwrap();
        assert_eq!(
            (m.title.as_str(), m.text.as_str(), m.kind, m.priority),
            (
                "Backup finished",
                "12 GB copied",
                EventKind::Done,
                Priority::Low
            )
        );
        // Minimal payload still produces something speakable.
        let bare = g.map(&json!({"message": "front door opened"})).unwrap();
        assert_eq!(bare.text, "front door opened");
        assert!(!bare.title.is_empty());
    }

    #[test]
    fn hostile_payloads_are_clamped_not_trusted() {
        let huge = "x".repeat(100_000);
        let m = claude()
            .map(&json!({"hook_event_name":"Stop","cwd":"/a/b","session_id": huge.clone(),"last_assistant_message": huge}))
            .unwrap();
        assert!(m.text.chars().count() <= MAX_TEXT);
        assert!(m.session_ref.unwrap().chars().count() <= MAX_REF);
    }

    #[test]
    fn templates_support_fallbacks_filters_and_escapes() {
        let t = Template::parse("{/a ?? /b ?? 'none'} {/p | basename | upper_not}").unwrap_err();
        assert!(t.0.contains("unknown filter"));
        let t =
            Template::parse("{/a ?? /b ?? 'none'} - {/p | basename | lower} {{literal}}").unwrap();
        let v = json!({"b": "second", "p": "C:\\Dir\\FILE.TXT"});
        assert_eq!(t.render(&v), "second - file.txt {literal}}");
        assert_eq!(
            Template::parse("{/missing ?? 'fallback'}")
                .unwrap()
                .render(&json!({})),
            "fallback"
        );
        assert_eq!(
            Template::parse("{/n | truncate:3}")
                .unwrap()
                .render(&json!({"n": "abcdef"})),
            "ab\u{2026}"
        );
    }

    #[test]
    fn bad_adapters_are_rejected_at_load_time() {
        for bad in [
            "not json",
            r#"{"name":"x","source":"s","rules":[]}"#,
            r#"{"name":"x","source":"s","rules":[{"kind":"done","title":"{unterminated"}]}"#,
            r#"{"name":"x","source":"s","rules":[{"kind":"done","title":"{nonpointer}"}]}"#,
            r#"{"name":"x","source":"s","rules":[{"when":{"nopointer":"x"},"kind":"done","title":"t"}]}"#,
        ] {
            assert!(Adapter::from_json(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn set_up_text_fills_its_placeholders_once() {
        let a = Adapter::from_json(
            r#"{"name":"x","source":"S","setup":["{public}/hook/{endpoint}","{ \"t\": \"{token}\" } {nope}"],
                "rules":[{"kind":"info","title":"t"}]}"#,
        )
        .unwrap();
        // A value that looks like a placeholder is inserted as it is.
        assert_eq!(
            a.setup_text("http://h:1", "{token}", "T{public}").unwrap(),
            "http://h:1/hook/{token}\n{ \"t\": \"T{public}\" } {nope}"
        );
        let none = Adapter::from_json(
            r#"{"name":"x","source":"S","rules":[{"kind":"info","title":"t"}]}"#,
        )
        .unwrap();
        assert!(none.setup_text("p", "e", "t").is_none());
    }

    #[test]
    fn payload_without_a_matching_rule_maps_to_none() {
        let a = Adapter::from_json(
            r#"{"name":"x","source":"S","rules":[{"when":{"/a":"1"},"kind":"done","title":"t"}]}"#,
        )
        .unwrap();
        assert!(a.map(&json!({"a": "2"})).is_none());
        assert!(a.map(&json!({"a": "1"})).is_some());
    }
}
