//! The embedded web UI and its JSON API (REQ-CFG-001, 003..006, 008, 018..024).
//!
//! * `GET /` and `GET /ui`: one static page. No external assets, works on a phone, built from `portal/` (Preact
//!   and TypeScript, bundled into the one file). It holds no data, so it needs no credentials; everything it
//!   shows comes from the API below.
//! * `GET  /api/auth`                          mode (setup, password, open) (no sign-in)
//! * `POST /api/setup`                         first run: `{"mode":"password","password":..}` or `{"mode":"open"}`
//! * `POST /api/login`                         `{"password":..}`; returns a session token
//! * `POST /api/logout`                        ends the session
//! * `POST /api/admin/password`                `{"password":..}`; sets a new password
//! * `GET  /api/overview`                      endpoints and recent agent events
//! * `GET  /api/schedule`, `POST /api/schedule`, `PUT|DELETE /api/schedule/{id}`   timers, alarms, reminders
//! * `GET  /api/tools`, `/api/settings`, `/api/about`   what the Tools, Connections, Settings tabs show
//! * `PUT  /api/settings`                      body `{key: value|null, ..}`; saves some settings (admin only)
//! * `GET  /api/audit`                         what the assistant did, newest first (filters and paging in `audit`)
//! * `GET  /api/status`                      what the assistant is doing (model, satellites, tools, usage, schedule)
//! * `POST /api/speak`                         body `{"text":..}`; the speaker says it ("test speak")
//! * `POST /api/endpoints`                     body `{"name":..,"adapter":..}`; the token is returned once
//! * `POST /api/endpoints/{name}/rotate`       a new token, returned once
//! * `DELETE /api/endpoints/{name}`
//!
//! The rest needs a session token as `Authorization: Bearer ...` (password mode) or a private-network peer
//! (open mode). Failed attempts, sign-ins, and setup tries share the
//! authentication rate limit. Secrets are write-only: tokens and the password are stored hashed.
//! Text from agents is untrusted: the page renders it as text only (Preact text children, never HTML injection), and the response carries a
//! strict Content-Security-Policy as a second line of defence.

use crate::admin::{self, AdminError};
use crate::app::{Core, Fail, Op, SchedOp};
use crate::extensions::{self, ExtOp};
use crate::http::{BODY_TIMEOUT, read_body};
use crate::inbox::endpoint::EndpointError;
use http_body_util::{BodyExt, Full, combinators::BoxBody};
use hyper::body::{Bytes, Incoming};
use hyper::header::{CONTENT_TYPE, HeaderMap, HeaderValue, WWW_AUTHENTICATE};
use hyper::{Method, Response, StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};
use std::convert::Infallible;
use std::net::IpAddr;
use std::sync::Arc;

#[cfg(test)]
mod tests;

type Resp = Response<BoxBody<Bytes, Infallible>>;

/// The page and a gzip copy of it. `ui.html` is generated from the TypeScript sources in `portal/` and
/// committed (ADR-0016); `build.rs` copies it and makes the gzip, so the program carries no compression code
/// (REQ-CFG-043).
const PAGE: &str = include_str!(concat!(env!("OUT_DIR"), "/ui-page.html"));
const PAGE_GZ: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/ui-page.html.gz"));
/// The release notes, shown on the About card (REQ-CFG-013, 025).
const CHANGELOG: &str = include_str!("../../../CHANGELOG.md");
/// Only the page's own inline script and style, and only calls back to this origin.
const CSP: &str = "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; connect-src 'self'; \
                   base-uri 'none'; form-action 'none'; frame-ancestors 'none'";
/// API bodies are tiny; refuse anything larger.
const MAX_BODY: usize = 4096;
/// A tool description with its parameters, and a skills file.
const MAX_TOOL_BODY: usize = 16 * 1024;
const MAX_SKILL_BODY: usize = 64 * 1024;
/// Events shown in the overview, newest first.
const MAX_EVENTS: usize = 50;
/// Longest event text sent to the page.
const MAX_TEXT: usize = 600;

fn reply(status: StatusCode, content_type: &'static str, body: String) -> Resp {
    let mut r = Response::new(Full::new(Bytes::from(body)).boxed());
    *r.status_mut() = status;
    r.headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
    r.headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    r.headers_mut().insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    r
}

fn json_reply(status: StatusCode, v: &Value) -> Resp {
    reply(status, "application/json", v.to_string())
}

fn error(status: StatusCode, msg: &str) -> Resp {
    json_reply(status, &json!({ "error": msg }))
}

/// The static page.
#[must_use]
pub fn page(accept_encoding: Option<&str>) -> Resp {
    let mut r = if accepts_gzip(accept_encoding) {
        let mut r = Response::new(Full::new(Bytes::from_static(PAGE_GZ)).boxed());
        r.headers_mut().insert(
            CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        );
        r.headers_mut()
            .insert("cache-control", HeaderValue::from_static("no-store"));
        r.headers_mut().insert(
            "x-content-type-options",
            HeaderValue::from_static("nosniff"),
        );
        r.headers_mut()
            .insert("content-encoding", HeaderValue::from_static("gzip"));
        r
    } else {
        reply(StatusCode::OK, "text/html; charset=utf-8", PAGE.to_owned())
    };
    r.headers_mut()
        .insert("vary", HeaderValue::from_static("Accept-Encoding"));
    r.headers_mut()
        .insert("content-security-policy", HeaderValue::from_static(CSP));
    r.headers_mut()
        .insert("x-frame-options", HeaderValue::from_static("DENY"));
    r.headers_mut()
        .insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    r
}

/// True when an `Accept-Encoding` header lists gzip (or `*`) with a quality above zero.
fn accepts_gzip(header: Option<&str>) -> bool {
    header.is_some_and(|h| {
        h.split(',').any(|item| {
            let mut parts = item.split(';');
            let name = parts.next().unwrap_or("").trim();
            let refused = parts.any(|p| {
                p.trim()
                    .strip_prefix("q=")
                    .is_some_and(|q| q.trim().parse::<f32>().is_ok_and(|q| q <= 0.0))
            });
            (name.eq_ignore_ascii_case("gzip") || name == "*") && !refused
        })
    })
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    let v = headers.get(hyper::header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, rest) = v.split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then(|| rest.trim())
}

fn admin_error(e: AdminError) -> Resp {
    let status = match e {
        AdminError::Unauthorized => StatusCode::UNAUTHORIZED,
        AdminError::RateLimited => StatusCode::TOO_MANY_REQUESTS,
        AdminError::SetupRequired | AdminError::NotPrivate | AdminError::OpenRefused => {
            StatusCode::FORBIDDEN
        }
        AdminError::AlreadySetUp => StatusCode::CONFLICT,
        AdminError::BadPassword => StatusCode::BAD_REQUEST,
        AdminError::Storage | AdminError::NoRandomness => StatusCode::INTERNAL_SERVER_ERROR,
    };
    let mut r = error(status, &e.to_string());
    match e {
        AdminError::RateLimited => {
            r.headers_mut()
                .insert(hyper::header::RETRY_AFTER, HeaderValue::from_static("5"));
        }
        AdminError::Unauthorized => {
            r.headers_mut()
                .insert(WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        }
        _ => {}
    }
    r
}

fn endpoint_error(e: &EndpointError) -> Resp {
    let status = match e {
        EndpointError::Exists => StatusCode::CONFLICT,
        EndpointError::NotFound => StatusCode::NOT_FOUND,
        EndpointError::InvalidName => StatusCode::BAD_REQUEST,
        EndpointError::NoRandomness => StatusCode::INTERNAL_SERVER_ERROR,
    };
    error(status, &e.to_string())
}

fn clip(s: &str) -> String {
    if s.chars().count() <= MAX_TEXT {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(MAX_TEXT).collect();
    out.push('…');
    out
}

fn state_label(s: &crate::inbox::event::EventState) -> &'static str {
    use crate::inbox::event::EventState::{Acknowledged, Announced, Dismissed, New, Snoozed};
    match s {
        New => "new",
        Announced => "announced",
        Acknowledged => "acknowledged",
        Dismissed => "dismissed",
        Snoozed { .. } => "snoozed",
    }
}

fn overview(core: &Core) -> Resp {
    let endpoints: Vec<Value> = core
        .endpoints()
        .iter()
        .map(|e| json!({ "name": e.name, "adapter": e.adapter, "enabled": e.enabled, "created_at": e.created_at }))
        .collect();
    let mut events = core.events();
    events.sort_by_key(|e| std::cmp::Reverse(e.id));
    let events: Vec<Value> = events
        .iter()
        .take(MAX_EVENTS)
        .map(|e| {
            json!({
                "id": e.id, "endpoint": e.endpoint, "source": e.source, "title": clip(&e.title),
                "text": clip(&e.text), "state": state_label(&e.state), "received_at": e.received_at,
            })
        })
        .collect();
    json_reply(
        StatusCode::OK,
        &json!({ "now": core.now(), "endpoints": endpoints, "events": events }),
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewEndpoint {
    name: String,
    adapter: String,
}

/// Handle `/`, `/ui`, and `/api/...`. Returns `None` for any other path so the caller can continue routing.
pub async fn handle(
    core: &Arc<Core>,
    method: &Method,
    segs: &[&str],
    query: Option<&str>,
    headers: &HeaderMap,
    peer: IpAddr,
    body: Incoming,
) -> Option<Resp> {
    let is_page = matches!(segs, [""] | ["ui"]);
    if is_page {
        return Some(match *method {
            Method::GET | Method::HEAD => page(
                headers
                    .get(hyper::header::ACCEPT_ENCODING)
                    .and_then(|v| v.to_str().ok()),
            ),
            _ => error(StatusCode::METHOD_NOT_ALLOWED, "method not allowed"),
        });
    }
    let ["api", rest @ ..] = segs else {
        return None;
    };
    // The few calls that work without being signed in: what to show, setup, and sign-in.
    match (method, rest) {
        (&Method::GET, ["auth"]) => {
            let mut state = core.admin_state();
            state["peer_private"] = json!(admin::is_private(peer));
            state["version"] = json!(env!("CARGO_PKG_VERSION"));
            return Some(json_reply(StatusCode::OK, &state));
        }
        (&Method::POST, ["setup"]) => return Some(setup(core, peer, body).await),
        (&Method::POST, ["login"]) => return Some(login(core, body).await),
        (&Method::POST, ["logout"]) => {
            if let Some(t) = bearer(headers) {
                core.admin_logout(t);
            }
            return Some(json_reply(StatusCode::OK, &json!({ "signed_out": true })));
        }
        _ => {}
    }
    let known = matches!(
        rest,
        ["overview"]
            | ["status"]
            | ["speak"]
            | ["endpoints"]
            | ["endpoints", _]
            | ["endpoints", _, "rotate"]
            | ["admin", "password"]
            | ["schedule"]
            | ["schedule", _]
            | ["tools"]
            | ["tools", _]
            | ["tools", _, "enabled"]
            | ["connections"]
            | ["connections", _]
            | ["connections", _, "enabled"]
            | ["skills"]
            | ["skills", _]
            | ["settings"]
            | ["audit"]
            | ["about"]
    );
    if !known {
        return Some(error(StatusCode::NOT_FOUND, "not found"));
    }
    // Authenticate before reading any body.
    if let Err(e) = core.check_admin(bearer(headers), peer) {
        return Some(admin_error(e));
    }
    Some(match (method, rest) {
        (&Method::GET, ["overview"]) => overview(core),
        (&Method::GET, ["status"]) => json_reply(
            StatusCode::OK,
            &json!({ "now": core.now(), "status": core.ui_status() }),
        ),
        (&Method::POST, ["speak"]) => speak(core, body).await,
        (&Method::POST, ["endpoints"]) => create(core, body).await,
        (&Method::POST, ["endpoints", name, "rotate"]) => match core.rotate_endpoint(name) {
            Ok(t) => json_reply(StatusCode::OK, &json!({ "name": name, "token": t })),
            Err(e) => endpoint_error(&e),
        },
        (&Method::DELETE, ["endpoints", name]) => match core.revoke_endpoint(name) {
            Ok(()) => json_reply(StatusCode::OK, &json!({ "deleted": name })),
            Err(e) => endpoint_error(&e),
        },
        (&Method::POST, ["admin", "password"]) => change_password(core, body).await,
        (&Method::GET, ["tools"] | ["skills"]) => json_reply(StatusCode::OK, &core.ui_tools()),
        (&Method::POST, ["tools"]) => match read_json_limit::<Value>(body, MAX_TOOL_BODY).await {
            Ok(v) => ext(core, ExtOp::ToolAdd(v)).await,
            Err((s, m)) => error(s, m),
        },
        (&Method::DELETE, ["tools", name]) => {
            ext(core, ExtOp::ToolDelete((*name).to_owned())).await
        }
        (&Method::POST, ["tools", name, "enabled"]) => match enabled_flag(body).await {
            Ok(on) => ext(core, ExtOp::ToolEnable((*name).to_owned(), on)).await,
            Err((s, m)) => error(s, m),
        },
        (&Method::POST, ["connections", "test"]) => match read_json::<Value>(body).await {
            Ok(v) => match extensions::test_connection(&v, |n| std::env::var(n).ok()).await {
                Ok(r) => json_reply(StatusCode::OK, &r),
                Err(f) => fail_reply(&f),
            },
            Err((s, m)) => error(s, m),
        },
        (&Method::POST, ["connections"]) => match read_json::<Value>(body).await {
            Ok(v) => ext(core, ExtOp::McpAdd(v)).await,
            Err((s, m)) => error(s, m),
        },
        (&Method::DELETE, ["connections", name]) => {
            ext(core, ExtOp::McpDelete((*name).to_owned())).await
        }
        (&Method::POST, ["connections", name, "enabled"]) => match enabled_flag(body).await {
            Ok(on) => ext(core, ExtOp::McpEnable((*name).to_owned(), on)).await,
            Err((s, m)) => error(s, m),
        },
        (&Method::POST, ["skills", action]) => skill_action(core, action, body).await,
        (&Method::POST, ["skills"]) => match read_json_limit::<Value>(body, MAX_SKILL_BODY).await {
            Ok(v) => ext(core, ExtOp::SkillAdd(v)).await,
            Err((s, m)) => error(s, m),
        },
        (&Method::GET, ["settings"]) => json_reply(StatusCode::OK, &core.ui_settings()),
        (&Method::PUT, ["settings"]) => match read_json_limit::<Value>(body, MAX_TOOL_BODY).await {
            Ok(v) => change_settings(core, &v).await,
            Err((s, m)) => error(s, m),
        },
        (&Method::GET, ["audit"]) => audit(core, query),
        (&Method::GET, ["about"]) => json_reply(
            StatusCode::OK,
            &json!({ "version": env!("CARGO_PKG_VERSION"), "changelog": CHANGELOG }),
        ),
        (&Method::GET, ["schedule"]) => schedule(core, SchedOp::List).await,
        (&Method::POST, ["schedule"]) => match read_json::<Value>(body).await {
            Ok(v) => schedule(core, SchedOp::Add(v)).await,
            Err((s, m)) => error(s, m),
        },
        (&Method::PUT, ["schedule", id]) => {
            match (id.parse::<u64>(), read_json::<Value>(body).await) {
                (Ok(id), Ok(v)) => schedule(core, SchedOp::Edit(id, v)).await,
                (Err(_), _) => error(StatusCode::NOT_FOUND, "no such timer, alarm, or reminder"),
                (_, Err((s, m))) => error(s, m),
            }
        }
        (&Method::DELETE, ["schedule", id]) => match id.parse::<u64>() {
            Ok(id) => schedule(core, SchedOp::Cancel(id)).await,
            Err(_) => error(StatusCode::NOT_FOUND, "no such timer, alarm, or reminder"),
        },
        _ => error(StatusCode::METHOD_NOT_ALLOWED, "method not allowed"),
    })
}

/// `GET /api/audit`: the record of what the assistant did (REQ-SEC-014), newest first. Filters: `limit`
/// (default 50, at most 200), `before` (a record id, for the next page), `tool`, `tier`, `decision` (a value, or
/// a prefix ending in `:` such as `blocked:`), `trigger` (`voice`, `event`, `event:<source>`) and `since` (Unix
/// seconds). Argument and result text in the records comes from outside: show it as data.
fn audit(core: &Core, query: Option<&str>) -> Resp {
    let Some(log) = core.audit() else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "The audit log is not available.",
        );
    };
    let q = match parse_audit_query(query.unwrap_or("")) {
        Ok(q) => q,
        Err(m) => return error(StatusCode::BAD_REQUEST, &m),
    };
    let page = log.query(&q);
    let h = log.health();
    let p = log.policy();
    json_reply(
        StatusCode::OK,
        &json!({
            "records": page.records,
            "next_before": page.next_before,
            "log": {
                "bytes": h.bytes,
                "max_file_bytes": h.max_file_bytes,
                "corrupt_lines": h.corrupt_lines,
                "write_errors": h.write_errors,
            },
            "policy": {
                "dry_run": p.dry_run,
                "max_actions_per_day": p.max_actions_per_day,
                "max_costly_per_day": p.max_costly_per_day,
            },
            "note": "Argument and result text can come from outside ClawDot. Show it as data.",
        }),
    )
}

/// Percent-decode a query component (also `+` as a space). Invalid escapes are kept as written.
fn query_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while let Some(&c) = b.get(i) {
        let hex = |x: Option<&u8>| x.and_then(|d| char::from(*d).to_digit(16));
        if c == b'%' {
            if let (Some(h), Some(l)) = (hex(b.get(i + 1)), hex(b.get(i + 2))) {
                out.push(u8::try_from(h * 16 + l).unwrap_or(b'?'));
                i += 3;
                continue;
            }
        }
        out.push(if c == b'+' { b' ' } else { c });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn parse_audit_query(raw: &str) -> Result<crate::audit::Query, String> {
    let mut q = crate::audit::Query::default();
    for pair in raw.split('&').filter(|p| !p.is_empty()).take(16) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let (k, v) = (query_decode(k), query_decode(v));
        let num = |name: &str| {
            v.parse::<u64>()
                .map_err(|_| format!("`{name}` must be a whole number."))
        };
        if v.is_empty() {
            continue;
        }
        match k.as_str() {
            "limit" => q.limit = Some(usize::try_from(num("limit")?).unwrap_or(usize::MAX)),
            "before" => q.before = Some(num("before")?),
            "since" => q.since = Some(i64::try_from(num("since")?).unwrap_or(i64::MAX)),
            "tool" => q.tool = Some(v),
            "tier" => q.tier = Some(v),
            "decision" => q.decision = Some(v),
            "trigger" => q.trigger = Some(v),
            _ => {}
        }
    }
    Ok(q)
}

/// Map a failed portal request to a status code and a message the page can show as written.
fn fail_reply(f: &Fail) -> Resp {
    let status = match f {
        Fail::NotFound(_) => StatusCode::NOT_FOUND,
        Fail::Invalid(_) => StatusCode::BAD_REQUEST,
        Fail::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
    };
    error(status, &f.to_string())
}

/// Timers, alarms, and reminders: the main loop does the work, so both the voice path and the portal use
/// one scheduler (REQ-CFG-027).
async fn schedule(core: &Core, op: SchedOp) -> Resp {
    match core.portal_call(Op::Sched(op)).await {
        Ok(v) => json_reply(StatusCode::OK, &v),
        Err(f) => fail_reply(&f),
    }
}

/// Save a change to settings, then tell the main loop to apply what can apply now (REQ-CFG-037..040).
async fn change_settings(core: &Core, patch: &Value) -> Resp {
    let (mut listing, live) = match core.change_settings(patch) {
        Ok(r) => r,
        Err(f) => return fail_reply(&f),
    };
    if let Err(f) = core.portal_call(Op::Settings(live)).await {
        listing["note"] = json!(format!(
            "Saved. {f} The live settings will apply the next time ClawDot starts."
        ));
    }
    json_reply(StatusCode::OK, &listing)
}

/// Run a change to tools, connections, or skills in the main loop; the answer is the new listing.
async fn ext(core: &Core, op: ExtOp) -> Resp {
    match core.portal_call(Op::Ext(op)).await {
        Ok(v) => json_reply(StatusCode::OK, &v),
        Err(f) => fail_reply(&f),
    }
}

/// `{"enabled": true|false}`.
async fn enabled_flag(body: Incoming) -> Result<bool, (StatusCode, &'static str)> {
    match read_json::<Value>(body).await {
        Ok(v) => v["enabled"].as_bool().ok_or((
            StatusCode::BAD_REQUEST,
            "expected {\"enabled\": true or false}",
        )),
        Err(e) => Err(e),
    }
}

/// Skills are addressed in the body, not the path, because their names are free text.
async fn skill_action(core: &Core, action: &str, body: Incoming) -> Resp {
    let v = match read_json_limit::<Value>(body, MAX_SKILL_BODY).await {
        Ok(v) => v,
        Err((s, m)) => return error(s, m),
    };
    let name = v["name"].as_str().unwrap_or("").to_owned();
    match action {
        "import" => ext(core, ExtOp::SkillImport(v)).await,
        "edit" => ext(core, ExtOp::SkillEdit(name, v["skill"].clone())).await,
        "delete" => ext(core, ExtOp::SkillDelete(name)).await,
        "enabled" => match v["enabled"].as_bool() {
            Some(on) => ext(core, ExtOp::SkillEnable(name, on)).await,
            None => error(
                StatusCode::BAD_REQUEST,
                "expected {\"name\":..,\"enabled\": true or false}",
            ),
        },
        _ => error(StatusCode::NOT_FOUND, "not found"),
    }
}

async fn read_json<T: serde::de::DeserializeOwned>(
    body: Incoming,
) -> Result<T, (StatusCode, &'static str)> {
    read_json_limit(body, MAX_BODY).await
}

async fn read_json_limit<T: serde::de::DeserializeOwned>(
    body: Incoming,
    max: usize,
) -> Result<T, (StatusCode, &'static str)> {
    let Some(bytes) = read_body(body, max, BODY_TIMEOUT).await else {
        return Err((StatusCode::PAYLOAD_TOO_LARGE, "body too large or too slow"));
    };
    serde_json::from_slice(&bytes).map_err(|_| (StatusCode::BAD_REQUEST, "bad request body"))
}

/// Hashing is deliberately slow; keep it off the async thread.
async fn hash_in_background(password: String) -> Result<String, AdminError> {
    tokio::task::spawn_blocking(move || admin::hash_password(&password))
        .await
        .unwrap_or(Err(AdminError::NoRandomness))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Setup {
    mode: String,
    password: Option<String>,
}

/// First-run setup (REQ-CFG-018): a password or open mode, from a private address only.
async fn setup(core: &Arc<Core>, peer: IpAddr, body: Incoming) -> Resp {
    let req: Setup = match read_json(body).await {
        Ok(r) => r,
        Err((s, m)) => return error(s, m),
    };
    // Refuse strangers before any expensive work.
    if let Err(e) = core.admin_check_setup(peer) {
        return admin_error(e);
    }
    match req.mode.as_str() {
        "open" => match core.admin_setup_open(peer) {
            Ok(()) => json_reply(StatusCode::OK, &json!({ "mode": "open" })),
            Err(e) => admin_error(e),
        },
        "password" => {
            let Some(pw) = req.password.filter(|p| admin::password_ok(p)) else {
                return admin_error(AdminError::BadPassword);
            };
            let hash = match hash_in_background(pw).await {
                Ok(h) => h,
                Err(e) => return admin_error(e),
            };
            match core.admin_setup_password(peer, hash) {
                Ok(token) => json_reply(
                    StatusCode::OK,
                    &json!({ "mode": "password", "token": token }),
                ),
                Err(e) => admin_error(e),
            }
        }
        _ => error(StatusCode::BAD_REQUEST, "mode must be password or open"),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Login {
    password: String,
}

/// Sign in with the password (or, after an upgrade, the old administrator token); returns a session token.
async fn login(core: &Arc<Core>, body: Incoming) -> Resp {
    let req: Login = match read_json(body).await {
        Ok(r) => r,
        Err((s, m)) => return error(s, m),
    };
    if core.admin_mode() == admin::Mode::Open {
        return error(StatusCode::CONFLICT, "no password is set");
    }
    let stored = match core.admin_login_begin() {
        Ok(h) => h,
        Err(e) => return admin_error(e),
    };
    let ok = tokio::task::spawn_blocking(move || admin::verify_password(&req.password, &stored))
        .await
        .unwrap_or(false);
    if !ok {
        return admin_error(AdminError::Unauthorized);
    }
    match core.admin_login_finish() {
        Ok(token) => json_reply(StatusCode::OK, &json!({ "token": token })),
        Err(e) => admin_error(e),
    }
}

/// Set a new administrator password (REQ-CFG-024). The caller is already authenticated.
async fn change_password(core: &Arc<Core>, body: Incoming) -> Resp {
    let req: Login = match read_json(body).await {
        Ok(r) => r,
        Err((s, m)) => return error(s, m),
    };
    if !admin::password_ok(&req.password) {
        return admin_error(AdminError::BadPassword);
    }
    let hash = match hash_in_background(req.password).await {
        Ok(h) => h,
        Err(e) => return admin_error(e),
    };
    match core.admin_set_password(hash) {
        Ok(token) => json_reply(
            StatusCode::OK,
            &json!({ "mode": "password", "token": token }),
        ),
        Err(e) => admin_error(e),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Speak {
    text: String,
}

/// "Test speak": queue a phrase for the speaker to say.
async fn speak(core: &Core, body: Incoming) -> Resp {
    let Some(bytes) = read_body(body, MAX_BODY, BODY_TIMEOUT).await else {
        return error(StatusCode::PAYLOAD_TOO_LARGE, "body too large or too slow");
    };
    let Ok(req) = serde_json::from_slice::<Speak>(&bytes) else {
        return error(StatusCode::BAD_REQUEST, "expected {\"text\":..}");
    };
    if core.queue_speech(&req.text) {
        json_reply(StatusCode::ACCEPTED, &json!({ "queued": true }))
    } else {
        error(
            StatusCode::BAD_REQUEST,
            "give some text of up to 200 characters, and wait for the last phrase to finish",
        )
    }
}

async fn create(core: &Core, body: Incoming) -> Resp {
    let Some(bytes) = read_body(body, MAX_BODY, BODY_TIMEOUT).await else {
        return error(StatusCode::PAYLOAD_TOO_LARGE, "body too large or too slow");
    };
    let Ok(req) = serde_json::from_slice::<NewEndpoint>(&bytes) else {
        return error(
            StatusCode::BAD_REQUEST,
            "expected {\"name\":..,\"adapter\":..}",
        );
    };
    if !core.has_adapter(&req.adapter) {
        return error(StatusCode::BAD_REQUEST, "unknown adapter");
    }
    match core.create_endpoint(&req.name, &req.adapter) {
        Ok((e, token)) => json_reply(
            StatusCode::CREATED,
            &json!({ "name": e.name, "adapter": e.adapter, "token": token }),
        ),
        Err(e) => endpoint_error(&e),
    }
}
