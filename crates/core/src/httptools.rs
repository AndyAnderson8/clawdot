//! Declarative HTTP tools (REQ-AST-014): an administrator describes a web call, the model fills in typed
//! parameters, and nothing else about the request is under the model's control.
//!
//! * The URL, method, headers, and credential are fixed in the configuration. The model never chooses them.
//! * Parameters are checked against the declared schema (type, enum, required, length). Unknown parameters
//!   are rejected rather than passed on.
//! * `{name}` placeholders in the URL path are filled with percent-encoded parameter values; the remaining
//!   parameters go in the query string for `GET`/`DELETE` and in a JSON body for `POST`/`PUT`.
//! * By default a call needs the user's spoken confirmation; the session loop enforces it (REQ-AST-008).
//! * The credential comes from an environment variable, never from the configuration file (REQ-SEC-001).
//! * Responses are bounded and are data for the model, never instructions.

use crate::llm::ToolSpec;
use crate::session::{Risk, Tier};
use crate::transport::{Connector, Endpoint};
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::{Method, Request};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::time::Duration;
use tokio::time::timeout;

/// Longest response text handed to the model.
const MAX_RESPONSE: usize = 1_000;
/// Most bytes read from a response.
const MAX_READ: usize = 64 * 1024;
/// Longest string parameter.
const MAX_PARAM_LEN: usize = 500;
/// Default and maximum call time.
const DEFAULT_TIMEOUT_SECS: u64 = 5;
const MAX_TIMEOUT_SECS: u64 = 30;

/// One declared parameter.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Param {
    /// `string`, `integer`, `number`, or `boolean`.
    #[serde(rename = "type")]
    pub kind: String,
    /// What it means, for the model.
    #[serde(default)]
    pub description: String,
    /// Allowed values, if the set is closed.
    #[serde(default, rename = "enum")]
    pub choices: Vec<String>,
    /// Whether the model must supply it.
    #[serde(default)]
    pub required: bool,
}

/// One tool as written in the configuration file.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ToolDef {
    /// Tool name the model sees: lowercase letters, digits, and underscores.
    pub name: String,
    /// What it does, for the model.
    pub description: String,
    /// `GET`, `POST`, `PUT`, or `DELETE`.
    #[serde(default = "default_method")]
    pub method: String,
    /// The URL. `{param}` placeholders are allowed in the path and query, never in the host.
    pub url: String,
    /// Fixed extra headers.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// Environment variable holding a bearer token to send as `Authorization`.
    #[serde(default)]
    pub auth_env: Option<String>,
    /// Parameters the model may supply.
    #[serde(default)]
    pub params: BTreeMap<String, Param>,
    /// How risky the tool is (REQ-SEC-010): `read_only`, `reversible`, `costly` or `destructive`. Only the
    /// administrator can set it. When absent, the tool is costly, unless the older `confirm: false` is given,
    /// which counts as reversible.
    #[serde(default)]
    pub tier: Option<Tier>,
    /// Older form of the tier: `true` (the default) means costly, `false` means reversible. Ignored when
    /// `tier` is set.
    #[serde(default = "yes")]
    pub confirm: bool,
    /// Seconds to wait for the call (1 to 30, default 5).
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

fn default_method() -> String {
    "GET".into()
}

fn yes() -> bool {
    true
}

impl ToolDef {
    /// The tier in force and the conservative baseline it was lowered from.
    #[must_use]
    pub fn risk(&self) -> Risk {
        let tier = self.tier.unwrap_or(if self.confirm {
            Tier::Costly
        } else {
            Tier::Reversible
        });
        Risk::set(tier, Tier::Costly)
    }

    /// Check the definition at startup so mistakes are reported before anything runs.
    ///
    /// # Errors
    /// A message naming the tool and the problem.
    pub fn validate(&self) -> Result<(), String> {
        let n = &self.name;
        if n.is_empty()
            || n.len() > 40
            || !n
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        {
            return Err(format!(
                "tool `{n}`: names use lowercase letters, digits, and _ (max 40)"
            ));
        }
        if self.description.trim().is_empty() {
            return Err(format!(
                "tool `{n}`: a description is needed so the model knows when to use it"
            ));
        }
        if !matches!(self.method.as_str(), "GET" | "POST" | "PUT" | "DELETE") {
            return Err(format!(
                "tool `{n}`: method must be GET, POST, PUT, or DELETE"
            ));
        }
        let host_part = self
            .url
            .split_once("://")
            .map_or("", |(_, r)| r.split(['/', '?']).next().unwrap_or(""));
        if host_part.contains(['{', '}']) {
            return Err(format!(
                "tool `{n}`: placeholders are not allowed in the host"
            ));
        }
        Endpoint::parse(&self.url.replace(['{', '}'], ""))
            .map_err(|e| format!("tool `{n}`: {e}"))?;
        for (p, d) in &self.params {
            if !matches!(d.kind.as_str(), "string" | "integer" | "number" | "boolean") {
                return Err(format!(
                    "tool `{n}`: parameter `{p}` has an unknown type `{}`",
                    d.kind
                ));
            }
            if p.is_empty()
                || !p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                || p == "confirmed"
            {
                return Err(format!("tool `{n}`: `{p}` is not a usable parameter name"));
            }
            if !d.choices.is_empty() && d.kind != "string" {
                return Err(format!(
                    "tool `{n}`: `enum` is only for string parameters (`{p}`)"
                ));
            }
        }
        for ph in placeholders(&self.url) {
            if !self.params.get(&ph).is_some_and(|d| d.required) {
                return Err(format!(
                    "tool `{n}`: URL placeholder `{{{ph}}}` must be a required parameter"
                ));
            }
        }
        if self
            .timeout_secs
            .is_some_and(|t| t == 0 || t > MAX_TIMEOUT_SECS)
        {
            return Err(format!(
                "tool `{n}`: timeout_secs must be 1 to {MAX_TIMEOUT_SECS}"
            ));
        }
        Ok(())
    }

    fn spec(&self) -> ToolSpec {
        let mut props = Map::new();
        let mut required = Vec::new();
        for (p, d) in &self.params {
            let mut o = Map::new();
            o.insert("type".into(), json!(d.kind));
            if !d.description.is_empty() {
                o.insert("description".into(), json!(d.description));
            }
            if !d.choices.is_empty() {
                o.insert("enum".into(), json!(d.choices));
            }
            props.insert(p.clone(), Value::Object(o));
            if d.required {
                required.push(p.clone());
            }
        }
        ToolSpec {
            name: self.name.clone(),
            description: self.description.clone(),
            parameters: json!({ "type": "object", "properties": props, "required": required }),
        }
    }
}

fn placeholders(url: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = url;
    while let Some(i) = rest.find('{') {
        let after = &rest[i + 1..];
        let Some(j) = after.find('}') else { break };
        out.push(after[..j].to_owned());
        rest = &after[j + 1..];
    }
    out
}

/// Percent-encode everything outside the URL-safe set.
fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn scalar(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Check `args` against the schema; returns the validated arguments (a `confirmed` key is refused like any unknown one).
fn check_args(def: &ToolDef, args: &Value) -> Result<Map<String, Value>, String> {
    let given = args
        .as_object()
        .ok_or("the arguments must be a JSON object")?;
    let mut out = Map::new();
    for (k, v) in given {
        let Some(d) = def.params.get(k) else {
            return Err(format!("`{k}` is not a parameter of this tool"));
        };
        let ok = match d.kind.as_str() {
            "string" => v
                .as_str()
                .is_some_and(|s| s.chars().count() <= MAX_PARAM_LEN),
            "integer" => v.is_i64() || v.is_u64(),
            "number" => v.is_number(),
            _ => v.is_boolean(),
        };
        if !ok {
            return Err(format!(
                "`{k}` must be a {} (strings up to {MAX_PARAM_LEN} characters)",
                d.kind
            ));
        }
        if !d.choices.is_empty() && !v.as_str().is_some_and(|s| d.choices.iter().any(|c| c == s)) {
            return Err(format!("`{k}` must be one of: {}", d.choices.join(", ")));
        }
        out.insert(k.clone(), v.clone());
    }
    for (p, d) in &def.params {
        if d.required && !out.contains_key(p) {
            return Err(format!("`{p}` is required"));
        }
    }
    Ok(out)
}

/// The HTTP tools that are configured, with the means to call them.
#[derive(Debug, Clone)]
pub struct HttpTools {
    defs: Vec<ToolDef>,
    connector: Connector,
    /// Resolved credentials by tool name, read once at startup.
    secrets: BTreeMap<String, String>,
}

impl HttpTools {
    /// Validate definitions and read their credentials from the environment (`env` looks one up).
    ///
    /// # Errors
    /// Invalid definitions, duplicate names, names that clash with `reserved`, or a missing credential.
    pub fn new(
        defs: Vec<ToolDef>,
        reserved: &[String],
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, String> {
        let mut secrets = BTreeMap::new();
        for (i, d) in defs.iter().enumerate() {
            d.validate()?;
            if reserved.contains(&d.name) || defs[..i].iter().any(|o| o.name == d.name) {
                return Err(format!("tool `{}`: the name is already taken", d.name));
            }
            if let Some(var) = &d.auth_env {
                let v = env(var).filter(|v| !v.is_empty()).ok_or_else(|| {
                    format!("tool `{}`: environment variable {var} is not set", d.name)
                })?;
                secrets.insert(d.name.clone(), v);
            }
        }
        Ok(Self {
            defs,
            connector: Connector::new()?,
            secrets,
        })
    }

    /// Add one tool while running (the portal, REQ-CFG-032). Checks it like start-up does, and reads its
    /// credential from the environment now.
    ///
    /// # Errors
    /// An invalid definition, a taken name, or a credential variable that is not set.
    pub fn add(
        &mut self,
        def: ToolDef,
        reserved: &[String],
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<(), String> {
        def.validate()?;
        if reserved.contains(&def.name) || self.owns(&def.name) {
            return Err(format!("tool `{}`: the name is already taken", def.name));
        }
        if let Some(var) = &def.auth_env {
            let v = env(var).filter(|v| !v.is_empty()).ok_or_else(|| {
                format!("tool `{}`: environment variable {var} is not set", def.name)
            })?;
            self.secrets.insert(def.name.clone(), v);
        }
        self.defs.push(def);
        Ok(())
    }

    /// Remove a tool by name; false if there is none.
    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.defs.len();
        self.defs.retain(|d| d.name != name);
        self.secrets.remove(name);
        self.defs.len() != before
    }

    /// The definitions, as written (no credentials).
    #[must_use]
    pub fn defs(&self) -> &[ToolDef] {
        &self.defs
    }

    /// Whether any tools are configured.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.defs.is_empty()
    }

    /// Whether `name` is one of these tools.
    #[must_use]
    pub fn owns(&self, name: &str) -> bool {
        self.defs.iter().any(|d| d.name == name)
    }

    /// Specs to offer the model.
    #[must_use]
    pub fn specs(&self) -> Vec<ToolSpec> {
        self.defs.iter().map(ToolDef::spec).collect()
    }

    /// The risk of a tool: the administrator's tier, over a conservative baseline of costly. An unknown
    /// name is costly.
    #[must_use]
    pub fn risk(&self, name: &str) -> Risk {
        self.defs
            .iter()
            .find(|d| d.name == name)
            .map_or(Risk::fixed(Tier::Costly), ToolDef::risk)
    }

    /// Call a tool.
    ///
    /// # Errors
    /// A message for the model: bad arguments, transport failure, or a non-success status.
    pub async fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        let def = self
            .defs
            .iter()
            .find(|d| d.name == name)
            .ok_or("unknown tool")?;
        let args = check_args(def, args)?;
        let (url, body) = build(def, &args)?;
        let endpoint = Endpoint::parse(&url)?;
        let path = url
            .split_once("://")
            .and_then(|(_, r)| r.find(['/', '?']).map(|i| r[i..].to_owned()))
            .unwrap_or_else(|| "/".into());
        let method = Method::from_bytes(def.method.as_bytes()).map_err(|e| e.to_string())?;
        let mut req = Request::builder()
            .method(method)
            .uri(path)
            .header("host", &endpoint.host)
            .header("connection", "close")
            .header("user-agent", "clawdot")
            .header("accept", "application/json, text/plain");
        for (k, v) in &def.headers {
            req = req.header(k, v);
        }
        if let Some(secret) = self.secrets.get(&def.name) {
            req = req.header("authorization", format!("Bearer {secret}"));
        }
        let body_bytes = match &body {
            Some(b) => {
                req = req.header("content-type", "application/json");
                Bytes::from(b.to_string())
            }
            None => Bytes::new(),
        };
        let request = req.body(Full::new(body_bytes)).map_err(|e| e.to_string())?;
        let secs = def.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS);
        let work = async {
            let response = self.connector.request(&endpoint, request).await?;
            let status = response.status();
            let mut body = response.into_body();
            let mut kept = Vec::new();
            while let Some(frame) = body.frame().await {
                let Ok(frame) = frame else { break };
                if let Some(d) = frame.data_ref() {
                    kept.extend_from_slice(d);
                    if kept.len() >= MAX_READ {
                        break;
                    }
                }
            }
            Ok::<_, String>((status, kept))
        };
        let (status, kept) = timeout(Duration::from_secs(secs), work)
            .await
            .map_err(|_| format!("no response within {secs} s"))??;
        let mut text = String::from_utf8_lossy(&kept).trim().to_owned();
        if text.chars().count() > MAX_RESPONSE {
            text = text.chars().take(MAX_RESPONSE).collect::<String>() + "…";
        }
        if status.is_success() {
            Ok(if text.is_empty() {
                format!("done (HTTP {})", status.as_u16())
            } else {
                format!("HTTP {}: {text}", status.as_u16())
            })
        } else {
            Err(format!("HTTP {}: {text}", status.as_u16()))
        }
    }
}

/// Build the final URL and optional JSON body from validated arguments.
fn build(def: &ToolDef, args: &Map<String, Value>) -> Result<(String, Option<Value>), String> {
    let mut url = def.url.clone();
    let mut rest = args.clone();
    for ph in placeholders(&def.url) {
        let v = rest
            .remove(&ph)
            .ok_or_else(|| format!("`{ph}` is required"))?;
        url = url.replace(&format!("{{{ph}}}"), &encode(&scalar(&v)));
    }
    if matches!(def.method.as_str(), "POST" | "PUT") {
        let body = if rest.is_empty() && def.params.is_empty() {
            None
        } else {
            Some(Value::Object(rest))
        };
        Ok((url, body))
    } else {
        if !rest.is_empty() {
            let q: Vec<String> = rest
                .iter()
                .map(|(k, v)| format!("{}={}", encode(k), encode(&scalar(v))))
                .collect();
            url.push(if url.contains('?') { '&' } else { '?' });
            url.push_str(&q.join("&"));
        }
        Ok((url, None))
    }
}

/// The assistant's built-in tools plus the configured HTTP tools, as one host for the session loop.
#[derive(Debug)]
pub struct WithHttp<'a, H> {
    /// The built-in tools.
    pub inner: &'a mut H,
    /// The configured HTTP tools.
    pub http: &'a HttpTools,
}

impl<H: crate::session::ToolHost> crate::session::ToolHost for WithHttp<'_, H> {
    fn tool_specs(&self) -> Vec<ToolSpec> {
        let mut v = self.inner.tool_specs();
        v.extend(self.http.specs());
        v
    }

    fn risk(&self, name: &str, args: &Value) -> Risk {
        if self.http.owns(name) {
            self.http.risk(name)
        } else {
            self.inner.risk(name, args)
        }
    }

    fn returns_untrusted_text(&self, name: &str) -> bool {
        self.http.owns(name) || self.inner.returns_untrusted_text(name)
    }

    fn describe_action(&self, name: &str, args: &Value) -> String {
        if self.http.owns(name) {
            crate::session::describe_generic(name, args)
        } else {
            self.inner.describe_action(name, args)
        }
    }

    fn expected_slow(&self, name: &str) -> bool {
        self.http.owns(name) || self.inner.expected_slow(name)
    }

    fn begin_turn(&mut self, user_text: &str) {
        self.inner.begin_turn(user_text);
    }

    async fn call_tool(&mut self, name: &str, args: &Value) -> Result<String, String> {
        if self.http.owns(name) {
            self.http.call(name, args).await
        } else {
            self.inner.call_tool(name, args).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn def(json: &str) -> ToolDef {
        serde_json::from_str(json).unwrap()
    }

    fn lights(url: &str) -> ToolDef {
        def(&format!(
            r#"{{"name":"set_light","description":"Set a light","method":"POST","url":"{url}",
              "params":{{"room":{{"type":"string","enum":["kitchen","den"],"required":true}},
                         "level":{{"type":"integer"}}}}}}"#
        ))
    }

    #[test]
    fn definitions_are_checked_at_startup() {
        for (bad, needle) in [
            (
                r#"{"name":"Bad","description":"d","url":"http://h/"}"#,
                "names use",
            ),
            (
                r#"{"name":"a","description":" ","url":"http://h/"}"#,
                "description",
            ),
            (r#"{"name":"a","description":"d","url":"ftp://h/"}"#, "http"),
            (
                r#"{"name":"a","description":"d","method":"PATCH","url":"http://h/"}"#,
                "method",
            ),
            (
                r#"{"name":"a","description":"d","url":"http://{h}/"}"#,
                "host",
            ),
            (
                r#"{"name":"a","description":"d","url":"http://h/{id}"}"#,
                "required parameter",
            ),
            (
                r#"{"name":"a","description":"d","url":"http://h/","params":{"p":{"type":"blob"}}}"#,
                "unknown type",
            ),
            (
                r#"{"name":"a","description":"d","url":"http://h/","params":{"confirmed":{"type":"string"}}}"#,
                "usable",
            ),
            (
                r#"{"name":"a","description":"d","url":"http://h/","timeout_secs":99}"#,
                "timeout",
            ),
        ] {
            let e = def(bad).validate().unwrap_err();
            assert!(e.contains(needle), "{bad}: {e}");
        }
        assert!(lights("http://h/x").validate().is_ok());
        assert!(
            serde_json::from_str::<ToolDef>(
                r#"{"name":"a","description":"d","url":"http://h/","oops":1}"#
            )
            .is_err()
        );
    }

    // REQ-SEC-010
    #[test]
    fn http_tools_are_costly_unless_the_administrator_lowers_the_tier() {
        let tier = |extra: &str| {
            let d = def(&format!(
                r#"{{"name":"t","description":"d","url":"http://h/x"{extra}}}"#
            ));
            d.risk()
        };
        assert_eq!(
            tier(""),
            Risk::fixed(Tier::Costly),
            "the default is the conservative tier"
        );
        let lowered = tier(r#","confirm":false"#);
        assert_eq!(
            (lowered.tier, lowered.baseline),
            (Tier::Reversible, Tier::Costly)
        );
        let read = tier(r#","tier":"read_only""#);
        assert_eq!((read.tier, read.baseline), (Tier::ReadOnly, Tier::Costly));
        assert_eq!(
            tier(r#","tier":"destructive","confirm":false"#).tier,
            Tier::Destructive
        );
        assert!(
            serde_json::from_str::<ToolDef>(
                r#"{"name":"t","description":"d","url":"http://h/x","tier":"harmless"}"#
            )
            .is_err()
        );
        let t = HttpTools::new(
            vec![def(r#"{"name":"t","description":"d","url":"http://h/x"}"#)],
            &[],
            |_| None,
        )
        .unwrap();
        assert_eq!(t.risk("nope"), Risk::fixed(Tier::Costly));
    }

    #[test]
    fn the_model_only_gets_the_declared_schema_and_confirmation_is_the_default() {
        let t = HttpTools::new(vec![lights("http://h/x")], &[], |_| None).unwrap();
        let spec = &t.specs()[0];
        assert_eq!(spec.name, "set_light");
        let props = &spec.parameters["properties"];
        assert!(props["room"]["enum"].is_array());
        assert!(
            !props.as_object().unwrap().contains_key("confirmed"),
            "the model gets no confirmation flag"
        );
        assert!(!props.as_object().unwrap().contains_key("url"));
        assert_eq!(t.risk("set_light"), Risk::fixed(Tier::Costly));
        assert!(t.owns("set_light") && !t.owns("other"));
    }

    #[test]
    fn names_and_credentials_are_checked() {
        let d = lights("http://h/x");
        assert!(
            HttpTools::new(vec![d.clone(), d.clone()], &[], |_| None)
                .unwrap_err()
                .contains("taken")
        );
        assert!(
            HttpTools::new(vec![d.clone()], &["set_light".into()], |_| None)
                .unwrap_err()
                .contains("taken")
        );
        let mut with_auth = d;
        with_auth.auth_env = Some("LIGHT_KEY".into());
        assert!(
            HttpTools::new(vec![with_auth.clone()], &[], |_| None)
                .unwrap_err()
                .contains("LIGHT_KEY")
        );
        assert!(HttpTools::new(vec![with_auth], &[], |_| Some("k".into())).is_ok());
    }

    #[test]
    fn arguments_are_validated_and_unknown_ones_rejected() {
        let d = lights("http://h/x");
        assert!(check_args(&d, &json!({"room":"den","level":3})).is_ok());
        assert!(
            check_args(&d, &json!({"room":"den","confirmed":true}))
                .unwrap_err()
                .contains("not a parameter")
        );
        for (args, needle) in [
            (json!({"level":3}), "required"),
            (json!({"room":"attic"}), "one of"),
            (json!({"room":"den","level":"high"}), "integer"),
            (json!({"room":"den","url":"http://evil"}), "not a parameter"),
            (json!({"room":"x".repeat(501)}), "characters"),
            (json!("den"), "object"),
        ] {
            let e = check_args(&d, &args).unwrap_err();
            assert!(e.contains(needle), "{args}: {e}");
        }
    }

    #[test]
    fn urls_and_bodies_are_built_safely() {
        let d = def(
            r#"{"name":"a","description":"d","url":"http://h/v1/{id}/go","method":"GET",
            "params":{"id":{"type":"string","required":true},"q":{"type":"string"}}}"#,
        );
        let args = check_args(&d, &json!({"id":"a/b c","q":"x&y=z"})).unwrap();
        let (url, body) = build(&d, &args).unwrap();
        assert_eq!(url, "http://h/v1/a%2Fb%20c/go?q=x%26y%3Dz");
        assert!(body.is_none());
        let p = lights("http://h/x");
        let args = check_args(&p, &json!({"room":"den","level":4})).unwrap();
        let (url, body) = build(&p, &args).unwrap();
        assert_eq!(url, "http://h/x");
        assert_eq!(body.unwrap(), json!({"room":"den","level":4}));
    }

    /// One canned response; returns the raw request.
    async fn serve_once(
        status: &'static str,
        body: &'static str,
    ) -> (u16, tokio::task::JoinHandle<String>) {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let h = tokio::spawn(async move {
            let (mut c, _) = l.accept().await.unwrap();
            let mut raw = Vec::new();
            let mut buf = [0u8; 2048];
            loop {
                let n = c.read(&mut buf).await.unwrap();
                raw.extend_from_slice(&buf[..n]);
                let t = String::from_utf8_lossy(&raw).into_owned();
                if let Some((head, b)) = t.split_once("\r\n\r\n") {
                    let want = head
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if b.len() >= want {
                        break;
                    }
                }
                if n == 0 {
                    break;
                }
            }
            let resp = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            c.write_all(resp.as_bytes()).await.unwrap();
            String::from_utf8_lossy(&raw).into_owned()
        });
        (port, h)
    }

    #[tokio::test]
    async fn a_call_reaches_the_server_with_the_fixed_url_headers_and_credential() {
        let (port, server) = serve_once("200 OK", r#"{"ok":true}"#).await;
        let mut d = lights(&format!("http://127.0.0.1:{port}/api/light"));
        d.auth_env = Some("K".into());
        d.headers.insert("X-Home".into(), "main".into());
        let t = HttpTools::new(vec![d], &[], |_| Some("sekret".into())).unwrap();
        let out = t
            .call("set_light", &json!({"room":"den","level":2}))
            .await
            .unwrap();
        assert_eq!(out, r#"HTTP 200: {"ok":true}"#);
        let raw = server.await.unwrap();
        assert!(raw.starts_with("POST /api/light HTTP/1.1"), "{raw}");
        let lower = raw.to_ascii_lowercase();
        assert!(lower.contains("authorization: bearer sekret") && lower.contains("x-home: main"));
        assert!(raw.ends_with(r#"{"level":2,"room":"den"}"#), "{raw}");
        assert!(
            !raw.contains("confirmed"),
            "the confirmation flag is not forwarded"
        );
    }

    #[tokio::test]
    async fn failures_become_messages_for_the_model() {
        let (port, _s) = serve_once("500 Internal Server Error", "boom").await;
        let t = HttpTools::new(
            vec![lights(&format!("http://127.0.0.1:{port}/x"))],
            &[],
            |_| None,
        )
        .unwrap();
        let e = t
            .call("set_light", &json!({"room":"den"}))
            .await
            .unwrap_err();
        assert_eq!(e, "HTTP 500: boom");
        let e = t
            .call("set_light", &json!({"room":"nope"}))
            .await
            .unwrap_err();
        assert!(e.contains("one of"));
        // Nothing is listening: a clear transport error, not a hang.
        let closed = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let t = HttpTools::new(
            vec![lights(&format!("http://127.0.0.1:{closed}/x"))],
            &[],
            |_| None,
        )
        .unwrap();
        assert!(
            t.call("set_light", &json!({"room":"den"}))
                .await
                .unwrap_err()
                .contains("connect")
        );
    }

    #[tokio::test]
    async fn long_responses_are_clipped() {
        let (port, _s) = serve_once("200 OK", Box::leak("y".repeat(5000).into_boxed_str())).await;
        let t = HttpTools::new(
            vec![lights(&format!("http://127.0.0.1:{port}/x"))],
            &[],
            |_| None,
        )
        .unwrap();
        let out = t.call("set_light", &json!({"room":"den"})).await.unwrap();
        assert!(out.chars().count() < 1_100, "{}", out.len());
    }
}
