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
//! * descriptions are clipped and schemas size-limited;
//! * every tool needs spoken confirmation unless the administrator lists it in `no_confirm` (or turns
//!   confirmation off for a whole server);
//! * results are clipped, and non-text content is replaced with a placeholder.
//!
//! Each request uses its own connection. That costs a little latency and keeps the client tiny, with no
//! connection state to get wrong.

use crate::llm::ToolSpec;
use crate::session::ToolHost;
use crate::transport::{Connector, Endpoint};
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::{Method, Request};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;
use tokio::time::timeout;

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
#[derive(Debug, Clone, Deserialize, PartialEq)]
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
    /// Whether spoken confirmation is required.
    pub confirm: bool,
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
#[derive(Debug)]
pub struct McpClient {
    servers: Vec<Server>,
    tools: Vec<McpTool>,
    connector: Connector,
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
        let mut servers = Vec::new();
        for (i, d) in defs.iter().enumerate() {
            d.validate()?;
            if defs[..i].iter().any(|o| o.name == d.name) {
                return Err(format!("mcp server `{}` is listed twice", d.name));
            }
            let secret = match &d.auth_env {
                Some(var) => Some(env(var).filter(|v| !v.is_empty()).ok_or_else(|| {
                    format!(
                        "mcp server `{}`: environment variable {var} is not set",
                        d.name
                    )
                })?),
                None => None,
            };
            servers.push(Server {
                endpoint: Endpoint::parse(&d.url)?,
                def: d.clone(),
                secret,
                session: Mutex::new(None),
            });
        }
        let mut client = Self {
            servers,
            tools: Vec::new(),
            connector: Connector::new()?,
        };
        let mut warnings = Vec::new();
        for i in 0..client.servers.len() {
            match client.load(i).await {
                Ok(mut tools) => {
                    tools.retain(|t| {
                        let clash = reserved.contains(&t.exposed);
                        if clash {
                            warnings.push(format!(
                                "mcp tool `{}` clashes with an existing tool and was skipped",
                                t.exposed
                            ));
                        }
                        !clash
                    });
                    client.tools.extend(tools);
                }
                Err(e) => warnings.push(format!(
                    "mcp server `{}` is unavailable: {e}",
                    client.servers[i].def.name
                )),
            }
        }
        Ok((client, warnings))
    }

    async fn rpc(
        &self,
        i: usize,
        id: Option<u64>,
        method: &str,
        params: Value,
    ) -> Result<Value, String> {
        let s = &self.servers[i];
        let mut msg = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        if let Some(id) = id {
            msg["id"] = json!(id);
        }
        let mut req = Request::builder()
            .method(Method::POST)
            .uri(if s.endpoint.base.is_empty() {
                "/".to_owned()
            } else {
                s.endpoint.base.clone()
            })
            .header("host", &s.endpoint.host)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .header("connection", "close")
            .header("user-agent", "clawdot");
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
            return Err("the server no longer knows this session; try again".into());
        }
        if !status.is_success() {
            return Err(format!("HTTP {}", status.as_u16()));
        }
        match id {
            Some(id) => find_response(&ctype, &body, id),
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
            let schema = t
                .get("inputSchema")
                .cloned()
                .unwrap_or_else(|| json!({"type": "object"}));
            if exposed.len() > MAX_NAME
                || schema.to_string().len() > MAX_SCHEMA_BYTES
                || schema["type"] != "object"
                || out.len() >= MAX_TOOLS_PER_SERVER
            {
                continue;
            }
            out.push(McpTool {
                server: i,
                remote: remote.to_owned(),
                exposed,
                description: clip(t["description"].as_str().unwrap_or(remote), MAX_DESCRIPTION),
                schema,
                confirm: def.confirm && !def.no_confirm.iter().any(|n| n == remote),
            });
        }
        Ok(out)
    }

    /// The tools on offer.
    #[must_use]
    pub fn tools(&self) -> &[McpTool] {
        &self.tools
    }

    /// Whether any tools are on offer.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    fn find(&self, name: &str) -> Option<&McpTool> {
        self.tools.iter().find(|t| t.exposed == name)
    }

    /// Whether `name` is one of these tools.
    #[must_use]
    pub fn owns(&self, name: &str) -> bool {
        self.find(name).is_some()
    }

    /// Whether a call needs spoken confirmation.
    #[must_use]
    pub fn needs_confirmation(&self, name: &str) -> bool {
        self.find(name).is_some_and(|t| t.confirm)
    }

    /// Specs to offer the model. Confirming tools get a `confirmed` flag added to their schema.
    #[must_use]
    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools
            .iter()
            .map(|t| {
                let mut schema = t.schema.clone();
                if t.confirm {
                    if let Some(props) = schema.get_mut("properties").and_then(Value::as_object_mut) {
                        props.insert(
                            "confirmed".into(),
                            json!({"type": "boolean", "description": "Set to true only after the user has said yes out loud."}),
                        );
                    } else {
                        schema["properties"] = json!({"confirmed": {"type": "boolean"}});
                    }
                }
                ToolSpec {
                    name: t.exposed.clone(),
                    description: t.description.clone(),
                    parameters: schema,
                }
            })
            .collect()
    }

    /// Call a tool.
    ///
    /// # Errors
    /// A message for the model: unknown tool, transport failure, or the server reporting the call failed.
    pub async fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        let t = self.find(name).ok_or("unknown tool")?;
        let mut arguments = args.as_object().cloned().unwrap_or_default();
        arguments.remove("confirmed");
        let params = json!({ "name": t.remote, "arguments": arguments });
        let mut result = self
            .rpc(t.server, Some(3), "tools/call", params.clone())
            .await;
        if result
            .as_ref()
            .err()
            .is_some_and(|e| e.contains("no longer knows this session"))
        {
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

    fn needs_confirmation(&self, name: &str) -> bool {
        if self.mcp.owns(name) {
            self.mcp.needs_confirmation(name)
        } else {
            self.inner.needs_confirmation(name)
        }
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
        let server = t.exposed.split("__").next().unwrap_or("").to_owned();
        *m.entry(server).or_insert(0) += 1;
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[derive(Debug, Clone)]
    struct Seen {
        method: String,
        headers: String,
    }

    /// How the fake server behaves.
    #[derive(Clone, Copy)]
    struct Mode {
        revision: &'static str,
        sse_list: bool,
        expire_session_once: bool,
        is_error: bool,
    }

    const OK: Mode = Mode {
        revision: PROTOCOL_REVISION,
        sse_list: true,
        expire_session_once: false,
        is_error: false,
    };

    async fn fake_server(mode: Mode) -> (u16, Arc<Mutex<Vec<Seen>>>, tokio::task::JoinHandle<()>) {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        let expired = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let h = tokio::spawn(async move {
            loop {
                let Ok((mut c, _)) = l.accept().await else {
                    return;
                };
                let log = Arc::clone(&log);
                let expired = Arc::clone(&expired);
                tokio::spawn(async move {
                    let mut raw = Vec::new();
                    let mut buf = [0u8; 8192];
                    let (head, body) = loop {
                        let Ok(n) = c.read(&mut buf).await else {
                            return;
                        };
                        if n == 0 {
                            return;
                        }
                        raw.extend_from_slice(&buf[..n]);
                        if let Some(p) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                            let head = String::from_utf8_lossy(&raw[..p]).into_owned();
                            let want: usize = head
                                .lines()
                                .find_map(|l| {
                                    l.to_ascii_lowercase()
                                        .strip_prefix("content-length:")
                                        .map(|v| v.trim().parse().unwrap())
                                })
                                .unwrap_or(0);
                            if raw.len() >= p + 4 + want {
                                break (head, raw[p + 4..p + 4 + want].to_vec());
                            }
                        }
                    };
                    let msg: Value = serde_json::from_slice(&body).unwrap();
                    let method = msg["method"].as_str().unwrap_or("").to_owned();
                    log.lock().unwrap().push(Seen {
                        method: method.clone(),
                        headers: head.to_ascii_lowercase(),
                    });
                    let id = msg["id"].clone();
                    let (status, ctype, extra, payload) = match method.as_str() {
                        "initialize" => (
                            "200 OK",
                            "application/json",
                            "Mcp-Session-Id: sess-1\r\n",
                            json!({"jsonrpc":"2.0","id":id,"result":{"protocolVersion":mode.revision,"capabilities":{"tools":{}},"serverInfo":{"name":"fake","version":"1"}}}).to_string(),
                        ),
                        "notifications/initialized" => ("202 Accepted", "application/json", "", String::new()),
                        "tools/list" => {
                            let result = json!({"tools":[
                                {"name":"Turn On","description":"Turn a light on","inputSchema":{"type":"object","properties":{"room":{"type":"string"}},"required":["room"]}},
                                {"name":"status","description":"What is on","inputSchema":{"type":"object","properties":{}}},
                                {"name":"huge","description":"x","inputSchema":{"type":"object","properties":{"p":{"type":"string","description":"y".repeat(9000)}}}},
                                {"name":"weird/name","description":"d","inputSchema":{"type":"object"}},
                                {"name":"not_object","description":"d","inputSchema":{"type":"string"}}
                            ]});
                            let m = json!({"jsonrpc":"2.0","id":id,"result":result}).to_string();
                            if mode.sse_list {
                                ("200 OK", "text/event-stream", "", format!(": hello\n\nevent: message\ndata: {{\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\"}}\n\ndata: {m}\n\n"))
                            } else {
                                ("200 OK", "application/json", "", m)
                            }
                        }
                        "tools/call" => {
                            if mode.expire_session_once && !expired.swap(true, std::sync::atomic::Ordering::SeqCst) {
                                ("404 Not Found", "application/json", "", String::new())
                            } else {
                                let name = msg["params"]["name"].as_str().unwrap_or("");
                                let text = format!("called {name} with {}", msg["params"]["arguments"]);
                                let content = json!([{"type":"text","text":text},{"type":"image","data":"AAAA","mimeType":"image/png"}]);
                                ("200 OK", "application/json", "", json!({"jsonrpc":"2.0","id":id,"result":{"content":content,"isError":mode.is_error}}).to_string())
                            }
                        }
                        _ => ("400 Bad Request", "application/json", "", String::new()),
                    };
                    let resp = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                        payload.len()
                    );
                    let _ = c.write_all(resp.as_bytes()).await;
                });
            }
        });
        (port, seen, h)
    }

    fn def(port: u16) -> ServerDef {
        serde_json::from_value(
            json!({"name": "home", "url": format!("http://127.0.0.1:{port}/mcp")}),
        )
        .unwrap()
    }

    #[test]
    fn definitions_are_checked() {
        let bad = |v: Value| {
            serde_json::from_value::<ServerDef>(v)
                .unwrap()
                .validate()
                .unwrap_err()
        };
        assert!(bad(json!({"name":"Bad Name","url":"http://h/mcp"})).contains("names use"));
        assert!(bad(json!({"name":"a","url":"ftp://h"})).contains("http"));
        assert!(bad(json!({"name":"a","url":"http://h","timeout_secs":0})).contains("timeout"));
        assert!(
            serde_json::from_value::<ServerDef>(json!({"name":"a","url":"http://h","oops":1}))
                .is_err()
        );
    }

    #[tokio::test]
    async fn it_connects_lists_tools_and_applies_safety_limits() {
        let (port, seen, _srv) = fake_server(OK).await;
        let (c, warnings) = McpClient::connect(vec![def(port)], &[], |_| None)
            .await
            .unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        let names: Vec<&str> = c.tools().iter().map(|t| t.exposed.as_str()).collect();
        // "Turn On" is sanitised, the oversized and non-object schemas are dropped, and a SSE reply works.
        assert_eq!(
            names,
            vec!["home__turn_on", "home__status", "home__weird_name"]
        );
        assert_eq!(summary(&c).get("home"), Some(&3));

        let log = seen.lock().unwrap();
        let methods: Vec<&str> = log.iter().map(|s| s.method.as_str()).collect();
        assert_eq!(
            methods,
            vec!["initialize", "notifications/initialized", "tools/list"]
        );
        assert!(
            !log[0].headers.contains("mcp-protocol-version"),
            "not sent before the version is agreed"
        );
        assert!(
            log[0]
                .headers
                .contains("accept: application/json, text/event-stream")
        );
        for later in &log[1..] {
            assert!(
                later
                    .headers
                    .contains(&format!("mcp-protocol-version: {PROTOCOL_REVISION}")),
                "{}",
                later.headers
            );
            assert!(
                later.headers.contains("mcp-session-id: sess-1"),
                "{}",
                later.headers
            );
        }
    }

    #[tokio::test]
    async fn tools_need_confirmation_unless_the_administrator_says_otherwise() {
        let (port, _seen, _srv) = fake_server(OK).await;
        let mut d = def(port);
        d.no_confirm = vec!["status".into()];
        let (c, _) = McpClient::connect(vec![d], &[], |_| None).await.unwrap();
        assert!(
            c.needs_confirmation("home__turn_on"),
            "confirm is the default"
        );
        assert!(!c.needs_confirmation("home__status"), "listed as harmless");
        let specs = c.specs();
        let on = specs.iter().find(|s| s.name == "home__turn_on").unwrap();
        assert!(on.parameters["properties"]["confirmed"].is_object());
        assert!(on.parameters["properties"]["room"].is_object());
        let status = specs.iter().find(|s| s.name == "home__status").unwrap();
        assert!(status.parameters["properties"].get("confirmed").is_none());

        let (port, _seen, _srv) = fake_server(OK).await;
        let mut d = def(port);
        d.confirm = false;
        let (c, _) = McpClient::connect(vec![d], &[], |_| None).await.unwrap();
        assert!(!c.needs_confirmation("home__turn_on"));
    }

    #[tokio::test]
    async fn the_only_list_limits_what_the_model_can_see() {
        let (port, _seen, _srv) = fake_server(OK).await;
        let mut d = def(port);
        d.only = vec!["status".into()];
        let (c, _) = McpClient::connect(vec![d], &["x".into()], |_| None)
            .await
            .unwrap();
        assert_eq!(c.tools().len(), 1);
        assert_eq!(c.tools()[0].exposed, "home__status");
    }

    #[tokio::test]
    async fn a_call_sends_the_arguments_not_the_confirmation_flag_and_clips_the_result() {
        let (port, seen, _srv) = fake_server(OK).await;
        let (c, _) = McpClient::connect(vec![def(port)], &[], |_| None)
            .await
            .unwrap();
        let out = c
            .call("home__turn_on", &json!({"room":"den","confirmed":true}))
            .await
            .unwrap();
        assert!(
            out.contains("called Turn On"),
            "the server's own tool name is used: {out}"
        );
        assert!(
            out.contains(r#""room":"den""#) && !out.contains("confirmed"),
            "{out}"
        );
        assert!(out.contains("[image content omitted]"), "{out}");
        assert!(seen.lock().unwrap().last().unwrap().method == "tools/call");
        assert!(
            c.call("home__nope", &json!({}))
                .await
                .unwrap_err()
                .contains("unknown")
        );
    }

    #[tokio::test]
    async fn a_tool_that_reports_an_error_is_an_error_for_the_model() {
        let (port, _seen, _srv) = fake_server(Mode {
            is_error: true,
            ..OK
        })
        .await;
        let (c, _) = McpClient::connect(vec![def(port)], &[], |_| None)
            .await
            .unwrap();
        assert!(
            c.call("home__status", &json!({}))
                .await
                .unwrap_err()
                .contains("called status")
        );
    }

    #[tokio::test]
    async fn an_expired_session_is_renewed_once() {
        let (port, seen, _srv) = fake_server(Mode {
            expire_session_once: true,
            ..OK
        })
        .await;
        let (c, _) = McpClient::connect(vec![def(port)], &[], |_| None)
            .await
            .unwrap();
        let out = c.call("home__status", &json!({})).await.unwrap();
        assert!(out.contains("called status"));
        let methods: Vec<String> = seen
            .lock()
            .unwrap()
            .iter()
            .map(|s| s.method.clone())
            .collect();
        assert_eq!(
            methods.iter().filter(|m| *m == "initialize").count(),
            2,
            "{methods:?}"
        );
    }

    #[tokio::test]
    async fn a_server_on_another_revision_or_a_dead_one_is_skipped_with_a_warning() {
        let (port, _seen, _srv) = fake_server(Mode {
            revision: "2024-11-05",
            ..OK
        })
        .await;
        let closed = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let mut dead = def(closed);
        dead.name = "dead".into();
        let (c, warnings) = McpClient::connect(vec![def(port), dead], &[], |_| None)
            .await
            .unwrap();
        assert!(c.is_empty());
        assert_eq!(warnings.len(), 2);
        assert!(
            warnings[0].contains("2024-11-05") && warnings[0].contains(PROTOCOL_REVISION),
            "{warnings:?}"
        );
        assert!(
            warnings[1].contains("dead") && warnings[1].contains("connect"),
            "{warnings:?}"
        );
    }

    #[tokio::test]
    async fn the_bearer_token_comes_from_the_environment_and_is_required() {
        let (port, seen, _srv) = fake_server(OK).await;
        let mut d = def(port);
        d.auth_env = Some("HOME_TOKEN".into());
        assert!(
            McpClient::connect(vec![d.clone()], &[], |_| None)
                .await
                .unwrap_err()
                .contains("HOME_TOKEN")
        );
        McpClient::connect(vec![d], &[], |_| Some("tok".into()))
            .await
            .unwrap();
        assert!(
            seen.lock().unwrap()[0]
                .headers
                .contains("authorization: bearer tok")
        );
    }

    #[tokio::test]
    async fn plain_json_replies_work_too_and_duplicates_are_refused() {
        let (port, _seen, _srv) = fake_server(Mode {
            sse_list: false,
            ..OK
        })
        .await;
        let (c, _) = McpClient::connect(vec![def(port)], &[], |_| None)
            .await
            .unwrap();
        assert_eq!(c.tools().len(), 3);
        let e = McpClient::connect(vec![def(port), def(port)], &[], |_| None)
            .await
            .unwrap_err();
        assert!(e.contains("twice"));
    }

    #[tokio::test]
    async fn the_host_layer_routes_by_name() {
        struct Base;
        impl ToolHost for Base {
            fn tool_specs(&self) -> Vec<ToolSpec> {
                vec![]
            }
            fn needs_confirmation(&self, _: &str) -> bool {
                false
            }
            async fn call_tool(&mut self, name: &str, _: &Value) -> Result<String, String> {
                Ok(format!("base {name}"))
            }
        }
        let (port, _seen, _srv) = fake_server(OK).await;
        let (c, _) = McpClient::connect(vec![def(port)], &[], |_| None)
            .await
            .unwrap();
        let mut base = Base;
        let mut host = WithMcp {
            inner: &mut base,
            mcp: &c,
        };
        assert_eq!(host.tool_specs().len(), 3);
        assert_eq!(
            host.call_tool("timer", &json!({})).await.unwrap(),
            "base timer"
        );
        assert!(
            host.call_tool("home__status", &json!({}))
                .await
                .unwrap()
                .contains("called status")
        );
        assert!(!host.needs_confirmation("timer") && host.needs_confirmation("home__turn_on"));
    }

    #[test]
    fn replies_are_found_by_id_in_json_or_event_streams() {
        let sse = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":9,\"result\":{\"a\":1}}\n\n";
        assert_eq!(
            find_response("text/event-stream", sse, 9).unwrap(),
            json!({"a": 1})
        );
        assert!(find_response("text/event-stream", sse, 5).is_err());
        let err = r#"{"jsonrpc":"2.0","id":2,"error":{"code":-32601,"message":"no such method"}}"#;
        assert!(
            find_response("application/json", err, 2)
                .unwrap_err()
                .contains("no such method")
        );
        assert!(
            find_response("application/json", "nope", 2)
                .unwrap_err()
                .contains("not JSON")
        );
    }
}
