//! A Model Context Protocol client over the Streamable HTTP transport (REQ-AST-005, REQ-AST-008).
//!
//! External MCP servers add tools to the assistant without any change to the core. Only HTTP servers are
//! supported (no stdio, so the core never launches a process), and the protocol revision is pinned to
//! [`PROTOCOL_REVISION`]; a server that does not speak it is refused with a clear message.
//!
//! MCP tool names, descriptions, schemas, and results all come from a server the administrator chose but that
//! the core does not control, so they are treated as untrusted data:
//!
//! * names are prefixed with the server name and restricted to `[a-z0-9_-]`;
//! * descriptions are clipped and schemas size-limited and cleaned (see `pin::clean_schema`);
//! * a tool is offered only as the administrator approved it: a fingerprint of its name, description and
//!   schema is kept with the connection, and a tool that is new or changed waits for review (see `pin`);
//! * every tool needs spoken confirmation unless the administrator lists it in `no_confirm` (or turns
//!   confirmation off for a whole server);
//! * results are clipped, and non-text content is replaced with a placeholder.
//!
//! Each request uses its own connection. That costs a little latency and keeps the client tiny, with no
//! connection state to get wrong.

use crate::llm::ToolSpec;
use crate::session::{Risk, Tier, ToolHost};
use crate::transport::{Connector, Endpoint};
use http_body_util::{BodyExt, Full};
use hyper::Method;
use hyper::body::Bytes;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::time::timeout;

mod pin;
pub use pin::{Pins, Review, ToolPin};
/// The MCP specification revision this client implements and requires.
pub const PROTOCOL_REVISION: &str = "2025-06-18";

const MAX_RESPONSE: usize = 1024 * 1024;
const MAX_RESULT_TEXT: usize = 1_000;
const MAX_DESCRIPTION: usize = 300;
const MAX_SCHEMA_BYTES: usize = 8 * 1024;
const MAX_TOOLS_PER_SERVER: usize = 32;
const MAX_NAME: usize = 64;
const DEFAULT_TIMEOUT_SECS: u64 = 10;

/// One MCP server, as written in the configuration file.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ServerDef {
    /// Short name, used as a prefix on its tools: lowercase letters, digits, `-` and `_`.
    pub name: String,
    /// The server's MCP endpoint, for example `http://homeassistant.local:8123/mcp`.
    pub url: String,
    /// Environment variable holding a bearer token for the server.
    #[serde(default)]
    pub auth_env: Option<String>,
    /// Require spoken confirmation for this server's tools. Defaults to true.
    #[serde(default = "yes")]
    pub confirm: bool,
    /// Tools (by the server's own names) that never need confirmation, even when `confirm` is true.
    #[serde(default)]
    pub no_confirm: Vec<String>,
    /// If not empty, only these tools (by the server's own names) are offered to the model.
    #[serde(default)]
    pub only: Vec<String>,
    /// The administrator's tier for a tool (by the server's own name): `read_only`, `reversible`, `costly` or
    /// `destructive`. A tool not listed is costly (or reversible if `confirm` is off for it). What the
    /// server says about its own tools, such as a read-only hint, is never used (REQ-SEC-010).
    #[serde(default)]
    pub tiers: BTreeMap<String, Tier>,
    /// Seconds to wait for each request (1 to 60, default 10).
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

fn yes() -> bool {
    true
}

fn name_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 24
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

impl ServerDef {
    /// The risk of one of this server's tools, from the administrator's settings only.
    fn risk_of(&self, remote: &str) -> Risk {
        let tier = self.tiers.get(remote).copied().unwrap_or(
            if self.confirm && !self.no_confirm.iter().any(|n| n == remote) {
                Tier::Costly
            } else {
                Tier::Reversible
            },
        );
        Risk::set(tier, Tier::Costly)
    }

    /// Check a definition at startup.
    ///
    /// # Errors
    /// A message naming the server and the problem.
    pub fn validate(&self) -> Result<(), String> {
        let n = &self.name;
        if !name_ok(n) {
            return Err(format!(
                "mcp server `{n}`: names use lowercase letters, digits, - and _ (max 24)"
            ));
        }
        Endpoint::parse(&self.url).map_err(|e| format!("mcp server `{n}`: {e}"))?;
        if self.timeout_secs.is_some_and(|t| t == 0 || t > 60) {
            return Err(format!("mcp server `{n}`: timeout_secs must be 1 to 60"));
        }
        Ok(())
    }
}

/// A tool a server offers, ready to give to the model.
#[derive(Debug, Clone, PartialEq)]
pub struct McpTool {
    /// Index of the server in [`McpClient`].
    server: usize,
    /// The server's own name for the tool.
    remote: String,
    /// The name the model sees: `server__tool`.
    pub exposed: String,
    /// Clipped description.
    pub description: String,
    /// JSON Schema for the arguments.
    pub schema: Value,
    /// How risky the tool is: the administrator's tier over a baseline of costly. Never read from the server.
    pub risk: Risk,
    /// Fingerprint of what the server offered (see [`pin::fingerprint`]).
    pub pin: ToolPin,
}

impl McpTool {
    /// The server's own name for the tool.
    #[must_use]
    pub fn remote(&self) -> &str {
        &self.remote
    }
}

/// A tool the server offers that the administrator has not approved as it is now. It is not offered to the model
/// and cannot be called.
#[derive(Debug, Clone, PartialEq)]
struct Held {
    tool: McpTool,
    why: Review,
}

/// The bearer token of a server, read from the variable its definition names.
fn server_secret(
    d: &ServerDef,
    env: impl Fn(&str) -> Option<String>,
) -> Result<Option<String>, String> {
    crate::env::named_secret(
        d.auth_env.as_deref(),
        &format!("mcp server `{}`", d.name),
        env,
    )
}

struct Server {
    def: ServerDef,
    endpoint: Endpoint,
    secret: Option<String>,
    session: Mutex<Option<String>>,
}

impl std::fmt::Debug for Server {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Server")
            .field("name", &self.def.name)
            .finish_non_exhaustive()
    }
}

/// The connected MCP servers and the tools they offer.
#[derive(Debug, Clone)]
pub struct McpClient {
    servers: Vec<Arc<Server>>,
    tools: Vec<Arc<McpTool>>,
    connector: Connector,
    /// Why each server could not be used, parallel to `servers` (`None` = connected).
    errors: Vec<Option<String>>,
    /// Whether each server has not yet reported, parallel to `servers` (see [`McpClient::declare`]).
    connecting: Vec<bool>,
    /// Tools waiting for the administrator (new, or changed since approval). Never offered, never called.
    held: Vec<Held>,
}

/// A server that is being added: checked, not yet part of the client. It owns everything the network half
/// needs, so [`PendingServer::load`] can run in a task beside the main loop.
#[derive(Debug)]
pub struct PendingServer {
    server: Arc<Server>,
    probe: McpClient,
}

impl PendingServer {
    /// The server's name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.server.def.name
    }

    /// The definition as the administrator gave it.
    #[must_use]
    pub fn def(&self) -> &ServerDef {
        &self.server.def
    }

    /// Handshake and list the tools. This is the slow part (the network, up to the server's timeout for each
    /// request).
    ///
    /// # Errors
    /// Why the server cannot be used, clipped to 200 characters.
    pub async fn load(&self) -> Result<Vec<McpTool>, String> {
        self.probe.load(0).await.map_err(|e| clip(&e, 200))
    }
}

/// A server's address without credentials, query, or fragment, safe to show.
#[must_use]
pub fn display_url(url: &str) -> String {
    let base = url.split(['?', '#']).next().unwrap_or("");
    match base.split_once("://") {
        Some((scheme, rest)) => {
            let (auth, path) = rest.split_once('/').map_or((rest, ""), |(a, p)| (a, p));
            let host = auth.rsplit('@').next().unwrap_or(auth);
            if path.is_empty() {
                format!("{scheme}://{host}")
            } else {
                format!("{scheme}://{host}/{path}")
            }
        }
        None => String::new(),
    }
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    s.chars().take(max).collect::<String>() + "…"
}

/// Keep `[a-z0-9_-]`, lowercasing letters and replacing anything else with `_`.
fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            let c = c.to_ascii_lowercase();
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Pull the JSON-RPC message with id `want` out of a response body that is JSON or server-sent events.
fn find_response(content_type: &str, body: &str, want: u64) -> Result<Value, String> {
    let candidates: Vec<Value> = if content_type.contains("text/event-stream") {
        let mut out = Vec::new();
        let mut data = String::new();
        for line in body.lines().chain(std::iter::once("")) {
            if let Some(rest) = line.strip_prefix("data:") {
                if !data.is_empty() {
                    data.push('\n');
                }
                data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
            } else if line.is_empty() && !data.is_empty() {
                if let Ok(v) = serde_json::from_str::<Value>(&data) {
                    out.push(v);
                }
                data.clear();
            }
        }
        out
    } else {
        vec![serde_json::from_str(body).map_err(|_| "the server's reply is not JSON".to_owned())?]
    };
    for v in candidates {
        // A batch is allowed to be an array.
        let items = match v {
            Value::Array(a) => a,
            other => vec![other],
        };
        for m in items {
            if m["id"] == json!(want) {
                if let Some(err) = m.get("error") {
                    let msg = err["message"].as_str().unwrap_or("unknown error");
                    return Err(format!("the server returned an error: {}", clip(msg, 200)));
                }
                return m
                    .get("result")
                    .cloned()
                    .ok_or_else(|| "the reply has no result".to_owned());
            }
        }
    }
    Err("the server sent no reply to the request".into())
}

/// Why one MCP request failed. The retry in [`McpClient::call`] depends on the kind, never on the wording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RpcError {
    /// The server answered 404 to a request in a session: it forgot the session, so a new handshake can help.
    SessionLost,
    /// Anything else, with the message to show.
    Other(String),
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SessionLost => f.write_str("the server no longer knows this session; try again"),
            Self::Other(m) => f.write_str(m),
        }
    }
}

impl From<String> for RpcError {
    fn from(m: String) -> Self {
        Self::Other(m)
    }
}

impl From<RpcError> for String {
    fn from(e: RpcError) -> Self {
        e.to_string()
    }
}

impl McpClient {
    /// Validate the definitions, connect to each server, and list its tools.
    ///
    /// A server that cannot be reached is skipped with a warning in the returned list, so one broken server
    /// does not stop the assistant from starting. Invalid definitions are errors.
    ///
    /// # Errors
    /// Invalid or duplicate definitions, a missing credential, or a name that clashes with `reserved`.
    pub async fn connect(
        defs: Vec<ServerDef>,
        reserved: &[String],
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<(Self, Vec<String>), String> {
        let (mut client, _) = Self::declare(defs, env)?;
        let mut warnings = Vec::new();
        for i in 0..client.servers.len() {
            let loaded = client.load(i).await.map_err(|e| clip(&e, 200));
            let name = client.servers[i].def.name.clone();
            warnings.extend(client.settle(&name, loaded, reserved, None));
        }
        Ok((client, warnings))
    }

    /// The quick, local half of [`McpClient::connect`]: validate the definitions and read the credentials, and
    /// return a client that lists every server as still connecting, with a [`PendingServer`] for each whose
    /// [`PendingServer::load`] can run in a task. The result of each goes back through [`McpClient::settle`].
    /// Nothing touches the network (ADR-0021).
    ///
    /// # Errors
    /// Invalid or duplicate definitions, or a missing credential.
    pub fn declare(
        defs: Vec<ServerDef>,
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<(Self, Vec<PendingServer>), String> {
        let mut servers = Vec::new();
        for (i, d) in defs.iter().enumerate() {
            d.validate()?;
            if defs[..i].iter().any(|o| o.name == d.name) {
                return Err(format!("mcp server `{}` is listed twice", d.name));
            }
            let secret = server_secret(d, &env)?;
            servers.push(Arc::new(Server {
                endpoint: Endpoint::parse(&d.url)?,
                def: d.clone(),
                secret,
                session: Mutex::new(None),
            }));
        }
        let connector = Connector::new()?;
        let client = Self {
            connecting: vec![true; servers.len()],
            errors: vec![None; servers.len()],
            servers,
            tools: Vec::new(),
            connector,
            held: Vec::new(),
        };
        let pending = client
            .servers
            .iter()
            .map(|s| client.pending_for(s))
            .collect();
        Ok((client, pending))
    }

    /// A one-server client that shares the server (and so its session) and the connector, so the slow half of
    /// a connection can run without `&self`.
    fn pending_for(&self, server: &Arc<Server>) -> PendingServer {
        PendingServer {
            server: Arc::clone(server),
            probe: Self {
                servers: vec![Arc::clone(server)],
                tools: Vec::new(),
                connector: self.connector.clone(),
                errors: vec![None],
                connecting: vec![false],
                held: Vec::new(),
            },
        }
    }

    /// Record how the connection to a declared server ended: `Ok` gives its tools (those that clash with
    /// `reserved` or with tools already on offer are skipped), `Err` marks it failed with the reason. The server
    /// stays in the client either way, so a source that cannot be reached is still listed and reported. Returns
    /// the warnings to print. A name that is no longer in the client (it was removed meanwhile) is ignored.
    ///
    /// `approved` is what the administrator approved for this server: a tool that is not in it, or differs from
    /// it, is held for review instead of offered. `None` means nothing was approved yet, so every tool is taken
    /// as found (the caller then records them, see `Extensions::pin_first_connect`).
    pub fn settle(
        &mut self,
        name: &str,
        result: Result<Vec<McpTool>, String>,
        reserved: &[String],
        approved: Option<&Pins>,
    ) -> Vec<String> {
        let Some(i) = self.servers.iter().position(|s| s.def.name == name) else {
            return Vec::new();
        };
        let mut warnings = Vec::new();
        self.connecting[i] = false;
        match result {
            Ok(mut tools) => {
                self.errors[i] = None;
                let (kept, said) = self.admit(tools, reserved);
                tools = kept;
                warnings.extend(said);
                for t in &mut tools {
                    t.server = i;
                }
                let (offer, held) = split_by_approval(tools, approved);
                for h in &held {
                    warnings.push(format!(
                        "mcp tool `{}` waits for review: {}",
                        h.tool.exposed,
                        h.why.plain()
                    ));
                }
                self.held.extend(held);
                self.tools.extend(offer.into_iter().map(Arc::new));
            }
            Err(e) => {
                let e = clip(&e, 200);
                warnings.push(format!("mcp server `{name}` is unavailable: {e}"));
                self.errors[i] = Some(e);
            }
        }
        warnings
    }

    /// The tools that may be offered, in order, and a warning for each one left out. A tool is left out when its
    /// exposed name is reserved, is already on offer, or was already taken earlier in `tools`: names are made
    /// lowercase and stripped of odd characters (`Foo` and `foo`, `a.b` and `a_b`), so a server can list two tools
    /// that end up with one name, and only the first could ever be called. The first one listed stays.
    fn admit(&self, tools: Vec<McpTool>, reserved: &[String]) -> (Vec<McpTool>, Vec<String>) {
        let mut kept: Vec<McpTool> = Vec::with_capacity(tools.len());
        let mut warnings = Vec::new();
        for t in tools {
            let clash = reserved.contains(&t.exposed)
                || self.tools.iter().any(|o| o.exposed == t.exposed)
                || kept.iter().any(|o| o.exposed == t.exposed);
            if clash {
                warnings.push(format!(
                    "mcp tool `{}` clashes with an existing tool and was skipped",
                    t.exposed
                ));
            } else {
                kept.push(t);
            }
        }
        (kept, warnings)
    }

    /// Whether this server was declared and has not yet reported (see [`McpClient::declare`]).
    #[must_use]
    pub fn is_connecting(&self, name: &str) -> bool {
        self.servers
            .iter()
            .zip(&self.connecting)
            .any(|(s, c)| *c && s.def.name == name)
    }

    async fn rpc(
        &self,
        i: usize,
        id: Option<u64>,
        method: &str,
        params: Value,
    ) -> Result<Value, RpcError> {
        let s = &self.servers[i];
        let mut msg = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        if let Some(id) = id {
            msg["id"] = json!(id);
        }
        let path = if s.endpoint.base.is_empty() {
            "/"
        } else {
            s.endpoint.base.as_str()
        };
        let mut req = s
            .endpoint
            .request_builder(Method::POST, path, "clawdot")
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream");
        if method != "initialize" {
            req = req.header("mcp-protocol-version", PROTOCOL_REVISION);
        }
        if let Some(sid) = s
            .session
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
        {
            req = req.header("mcp-session-id", sid);
        }
        if let Some(secret) = &s.secret {
            req = req.header("authorization", format!("Bearer {secret}"));
        }
        let request = req
            .body(Full::new(Bytes::from(msg.to_string())))
            .map_err(|e| e.to_string())?;
        let secs = s.def.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS);
        let work = async {
            let response = self.connector.request(&s.endpoint, request).await?;
            let status = response.status();
            let sid = response
                .headers()
                .get("mcp-session-id")
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            let ctype = response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_owned();
            let mut body = response.into_body();
            let mut all = Vec::new();
            while let Some(frame) = body.frame().await {
                let frame = frame.map_err(|e| format!("read: {e}"))?;
                if let Some(d) = frame.data_ref() {
                    all.extend_from_slice(d);
                    if all.len() > MAX_RESPONSE {
                        return Err("response too large".to_owned());
                    }
                }
            }
            Ok::<_, String>((
                status,
                sid,
                ctype,
                String::from_utf8_lossy(&all).into_owned(),
            ))
        };
        let (status, sid, ctype, body) = timeout(Duration::from_secs(secs), work)
            .await
            .map_err(|_| format!("no response within {secs} s"))??;
        if method == "initialize" {
            if let Some(sid) = sid {
                *s.session.lock().unwrap_or_else(PoisonError::into_inner) = Some(sid);
            }
        }
        if status.as_u16() == 404 && method != "initialize" {
            // The session expired; forget it so the next call starts a new one.
            *s.session.lock().unwrap_or_else(PoisonError::into_inner) = None;
            return Err(RpcError::SessionLost);
        }
        if !status.is_success() {
            return Err(RpcError::Other(format!("HTTP {}", status.as_u16())));
        }
        match id {
            Some(id) => find_response(&ctype, &body, id).map_err(RpcError::Other),
            None => Ok(Value::Null),
        }
    }

    async fn handshake(&self, i: usize) -> Result<(), String> {
        let init = self
            .rpc(
                i,
                Some(1),
                "initialize",
                json!({
                    "protocolVersion": PROTOCOL_REVISION,
                    "capabilities": {},
                    "clientInfo": { "name": "clawdot", "version": env!("CARGO_PKG_VERSION") },
                }),
            )
            .await?;
        let theirs = init["protocolVersion"].as_str().unwrap_or("");
        if theirs != PROTOCOL_REVISION {
            return Err(format!(
                "it speaks MCP revision `{theirs}`; this core requires `{PROTOCOL_REVISION}`"
            ));
        }
        self.rpc(i, None, "notifications/initialized", json!({}))
            .await?;
        Ok(())
    }

    async fn load(&self, i: usize) -> Result<Vec<McpTool>, String> {
        self.handshake(i).await?;
        let listed = self.rpc(i, Some(2), "tools/list", json!({})).await?;
        let def = &self.servers[i].def;
        let mut out = Vec::new();
        for t in listed["tools"].as_array().map_or(&[][..], Vec::as_slice) {
            let Some(remote) = t["name"].as_str() else {
                continue;
            };
            if !def.only.is_empty() && !def.only.iter().any(|o| o == remote) {
                continue;
            }
            let exposed = format!("{}__{}", def.name, sanitize(remote));
            let offered = t
                .get("inputSchema")
                .cloned()
                .unwrap_or_else(|| json!({"type": "object"}));
            if exposed.len() > MAX_NAME
                || offered.to_string().len() > MAX_SCHEMA_BYTES
                || offered["type"] != "object"
                || out.len() >= MAX_TOOLS_PER_SERVER
            {
                continue;
            }
            // The fingerprint is of what the server said; the model is given the cleaned schema.
            let pin = pin::fingerprint(t["description"].as_str().unwrap_or(""), &offered);
            let schema = pin::clean_schema(&offered);
            out.push(McpTool {
                server: i,
                remote: remote.to_owned(),
                exposed,
                description: clip(t["description"].as_str().unwrap_or(remote), MAX_DESCRIPTION),
                schema,
                risk: def.risk_of(remote),
                pin,
            });
        }
        Ok(out)
    }

    /// Each configured server with its connection status and the tools it offers, for the portal. Holds no
    /// secrets. Everything a server says (names, descriptions) is untrusted text and is shown as text only.
    #[must_use]
    pub fn report(&self) -> Value {
        let list: Vec<Value> = self
            .servers
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let tools: Vec<Value> = self
                    .tools
                    .iter()
                    .filter(|t| t.server == i)
                    .map(|t| json!({ "name": t.exposed, "description": t.description, "confirm": t.risk.tier >= Tier::Costly, "tier": t.risk.tier }))
                    .collect();
                let review: Vec<Value> = self
                    .held
                    .iter()
                    .filter(|h| h.tool.server == i)
                    .map(|h| {
                        json!({
                            "name": h.tool.exposed, "description": h.tool.description,
                            "reason": h.why.label(), "reason_text": h.why.plain(),
                            "arguments": pin::argument_names(&h.tool.schema),
                        })
                    })
                    .collect();
                let error = self.errors.get(i).cloned().flatten();
                let connecting = self.connecting.get(i).copied().unwrap_or(false);
                json!({
                    "name": s.def.name, "url": display_url(&s.def.url), "connected": error.is_none() && !connecting,
                    "connecting": connecting,
                    "error": error, "confirm": s.def.confirm, "token_env": s.def.auth_env, "tools": tools,
                    "needs_review": review,
                })
            })
            .collect();
        Value::Array(list)
    }

    /// Whether a server with this name is configured (connected or not).
    #[must_use]
    pub fn has_server(&self, name: &str) -> bool {
        self.servers.iter().any(|s| s.def.name == name)
    }

    /// Connect one more server while running (the portal, REQ-CFG-033). On failure nothing is added.
    ///
    /// # Errors
    /// An invalid or duplicate definition, a credential variable that is not set, or a server that cannot be
    /// used, each with a message that says what to fix.
    pub async fn add_server(
        &mut self,
        def: ServerDef,
        reserved: &[String],
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<usize, String> {
        let pending = self.prepare_server(def, env)?;
        let tools = pending.load().await?;
        self.install_server(&pending, tools, reserved)
    }

    /// The quick, local first half of adding a server: check the definition and the name, read the credential.
    /// Nothing is added and nothing touches the network. The network half is [`PendingServer::load`], which
    /// owns what it needs and can run in a task; [`McpClient::install_server`] finishes the job (ADR-0021).
    ///
    /// # Errors
    /// An invalid or duplicate definition, or a credential variable that is not set.
    pub fn prepare_server(
        &self,
        def: ServerDef,
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<PendingServer, String> {
        def.validate()?;
        if self.has_server(&def.name) {
            return Err(format!("mcp server `{}` is listed twice", def.name));
        }
        let secret = server_secret(&def, &env)?;
        let server = Arc::new(Server {
            endpoint: Endpoint::parse(&def.url)?,
            def,
            secret,
            session: Mutex::new(None),
        });
        Ok(self.pending_for(&server))
    }

    /// The quick, local second half: add a server whose tools were listed. Checked again here, because the
    /// client may have changed while the connection was made. Tools that clash with `reserved` or with tools
    /// already on offer are left out. Returns how many were kept.
    ///
    /// # Errors
    /// A server of that name was added in the meantime.
    pub fn install_server(
        &mut self,
        pending: &PendingServer,
        mut tools: Vec<McpTool>,
        reserved: &[String],
    ) -> Result<usize, String> {
        let name = &pending.server.def.name;
        if self.has_server(name) {
            return Err(format!("mcp server `{name}` is listed twice"));
        }
        self.servers.push(Arc::clone(&pending.server));
        self.errors.push(None);
        self.connecting.push(false);
        let i = self.servers.len() - 1;
        let (kept, said) = self.admit(tools, reserved);
        tools = kept;
        for w in said {
            crate::diag::warn(&w);
        }
        for t in &mut tools {
            t.server = i;
        }
        let n = tools.len();
        self.tools.extend(tools.into_iter().map(Arc::new));
        Ok(n)
    }

    /// Forget a server and its tools; false if there is none.
    pub fn remove_server(&mut self, name: &str) -> bool {
        let Some(i) = self.servers.iter().position(|s| s.def.name == name) else {
            return false;
        };
        self.servers.remove(i);
        self.errors.remove(i);
        self.connecting.remove(i);
        self.tools.retain(|t| t.server != i);
        self.held.retain(|h| h.tool.server != i);
        for t in &mut self.tools {
            if t.server > i {
                Arc::make_mut(t).server -= 1;
            }
        }
        for h in &mut self.held {
            if h.tool.server > i {
                h.tool.server -= 1;
            }
        }
        true
    }

    /// Whether the server answered and listed its tools (not connecting, not failed).
    #[must_use]
    pub fn is_connected(&self, name: &str) -> bool {
        self.servers
            .iter()
            .enumerate()
            .any(|(i, s)| s.def.name == name && self.errors[i].is_none() && !self.connecting[i])
    }

    fn server_index(&self, name: &str) -> Option<usize> {
        self.servers.iter().position(|s| s.def.name == name)
    }

    /// The fingerprints of every tool the server listed now, offered or held, by the server's own tool names.
    /// This is what an administrator who adds or accepts the connection approves.
    #[must_use]
    pub fn pins_of(&self, name: &str) -> Pins {
        let Some(i) = self.server_index(name) else {
            return Pins::new();
        };
        self.tools
            .iter()
            .map(|t| &**t)
            .chain(self.held.iter().map(|h| &h.tool))
            .filter(|t| t.server == i)
            .map(|t| (t.remote.clone(), t.pin.clone()))
            .collect()
    }

    /// The tools of a server waiting for review, as `(server's tool name, fingerprint)`; only `tool` (the
    /// exposed name) when given.
    #[must_use]
    pub fn held_pins(&self, name: &str, tool: Option<&str>) -> Vec<(String, ToolPin)> {
        let Some(i) = self.server_index(name) else {
            return Vec::new();
        };
        self.held
            .iter()
            .filter(|h| h.tool.server == i && tool.is_none_or(|t| h.tool.exposed == t))
            .map(|h| (h.tool.remote.clone(), h.tool.pin.clone()))
            .collect()
    }

    /// The administrator accepted these held tools: offer them. Returns the exposed names released. A tool whose
    /// name has been taken meanwhile (by `reserved` or another tool on offer) stays out.
    pub fn release(&mut self, name: &str, tool: Option<&str>, reserved: &[String]) -> Vec<String> {
        let Some(i) = self.server_index(name) else {
            return Vec::new();
        };
        let mut released = Vec::new();
        let waiting = std::mem::take(&mut self.held);
        for h in waiting {
            let wanted = h.tool.server == i && tool.is_none_or(|t| h.tool.exposed == t);
            let clash = reserved.contains(&h.tool.exposed)
                || self.tools.iter().any(|o| o.exposed == h.tool.exposed);
            if wanted && !clash {
                released.push(h.tool.exposed.clone());
                self.tools.push(Arc::new(h.tool));
            } else {
                self.held.push(h);
            }
        }
        released
    }

    /// The tools on offer.
    #[must_use]
    pub fn tools(&self) -> &[Arc<McpTool>] {
        &self.tools
    }

    /// Whether any tools are on offer.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    fn find(&self, name: &str) -> Option<&McpTool> {
        self.tools.iter().map(|t| &**t).find(|t| t.exposed == name)
    }

    /// Whether `name` is one of these tools.
    #[must_use]
    pub fn owns(&self, name: &str) -> bool {
        self.find(name).is_some()
    }

    /// The name of the server that offers the tool `exposed`. This is not read from the tool's name: server names
    /// may contain `__`, so `a__b__x` can be tool `b__x` of `a` or tool `x` of `a__b`.
    #[must_use]
    pub fn server_of(&self, exposed: &str) -> Option<&str> {
        let t = self.find(exposed)?;
        self.servers.get(t.server).map(|s| s.def.name.as_str())
    }

    /// The risk of a tool. An unknown name is costly.
    #[must_use]
    pub fn risk(&self, name: &str) -> Risk {
        self.find(name)
            .map_or(Risk::fixed(Tier::Costly), |t| t.risk)
    }

    /// Specs to offer the model, as the server declared them. The model is given no confirmation flag:
    /// confirmation is held by the host (REQ-SEC-011).
    #[must_use]
    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools
            .iter()
            .map(|t| ToolSpec {
                name: t.exposed.clone(),
                description: t.description.clone(),
                parameters: t.schema.clone(),
            })
            .collect()
    }

    /// Call a tool.
    ///
    /// # Errors
    /// A message for the model: unknown tool, transport failure, or the server reporting the call failed.
    pub async fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        let t = self.find(name).ok_or("unknown tool")?;
        let arguments = args.as_object().cloned().unwrap_or_default();
        let params = json!({ "name": t.remote, "arguments": arguments });
        let mut result = self
            .rpc(t.server, Some(3), "tools/call", params.clone())
            .await;
        if result.as_ref().err() == Some(&RpcError::SessionLost) {
            self.handshake(t.server).await?;
            result = self.rpc(t.server, Some(3), "tools/call", params).await;
        }
        let result = result?;
        let mut text = String::new();
        for part in result["content"].as_array().map_or(&[][..], Vec::as_slice) {
            if !text.is_empty() {
                text.push('\n');
            }
            match part["type"].as_str() {
                Some("text") => text.push_str(part["text"].as_str().unwrap_or("")),
                Some(other) => text.push_str(&format!("[{other} content omitted]")),
                None => {}
            }
        }
        let text = clip(text.trim(), MAX_RESULT_TEXT);
        if result["isError"] == json!(true) {
            return Err(if text.is_empty() {
                "the tool reported an error".into()
            } else {
                text
            });
        }
        Ok(if text.is_empty() { "done".into() } else { text })
    }
}

/// Split freshly listed tools into those to offer and those to hold. With `approved` of `None` every tool is
/// offered. Otherwise a tool is offered only when its fingerprint equals the approved one.
fn split_by_approval(tools: Vec<McpTool>, approved: Option<&Pins>) -> (Vec<McpTool>, Vec<Held>) {
    let Some(approved) = approved else {
        return (tools, Vec::new());
    };
    let mut offer = Vec::new();
    let mut held = Vec::new();
    for t in tools {
        match pin::review_of(approved.get(&t.remote), &t.pin) {
            None => offer.push(t),
            Some(why) => held.push(Held { tool: t, why }),
        }
    }
    (offer, held)
}

/// Another layer of tools on top of a host, for the session loop.
#[derive(Debug)]
pub struct WithMcp<'a, H> {
    /// The tools underneath.
    pub inner: &'a mut H,
    /// The MCP tools.
    pub mcp: &'a McpClient,
}

impl<H: ToolHost> ToolHost for WithMcp<'_, H> {
    fn tool_specs(&self) -> Vec<ToolSpec> {
        let mut v = self.inner.tool_specs();
        v.extend(self.mcp.specs());
        v
    }

    fn risk(&self, name: &str, args: &Value) -> Risk {
        if self.mcp.owns(name) {
            self.mcp.risk(name)
        } else {
            self.inner.risk(name, args)
        }
    }

    fn returns_untrusted_text(&self, name: &str) -> bool {
        self.mcp.owns(name) || self.inner.returns_untrusted_text(name)
    }

    fn describe_action(&self, name: &str, args: &Value) -> String {
        if self.mcp.owns(name) {
            crate::session::describe_generic(name, args)
        } else {
            self.inner.describe_action(name, args)
        }
    }

    fn expected_slow(&self, name: &str) -> bool {
        self.mcp.owns(name) || self.inner.expected_slow(name)
    }

    fn clean_args(&self, name: &str, args: &mut Value) {
        if !self.mcp.owns(name) {
            self.inner.clean_args(name, args);
        }
    }

    async fn pin_args(&mut self, name: &str, args: Value) -> Value {
        if self.mcp.owns(name) {
            args
        } else {
            self.inner.pin_args(name, args).await
        }
    }

    fn begin_turn(&mut self, user_text: &str) {
        self.inner.begin_turn(user_text);
    }

    async fn call_tool(&mut self, name: &str, args: &Value) -> Result<String, String> {
        if self.mcp.owns(name) {
            self.mcp.call(name, args).await
        } else {
            self.inner.call_tool(name, args).await
        }
    }
}

/// Group tool names by server, for status output.
#[must_use]
pub fn summary(c: &McpClient) -> BTreeMap<String, usize> {
    let mut m = BTreeMap::new();
    for t in c.tools() {
        let server = c.server_of(&t.exposed).unwrap_or("").to_owned();
        *m.entry(server).or_insert(0) += 1;
    }
    m
}

#[cfg(test)]
mod tests;
