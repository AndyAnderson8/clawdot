//! The embedded web UI and its JSON API (REQ-CFG-001, 003, 004, 005, 006, 008).
//!
//! * `GET /` and `GET /ui`: one static page. No external assets, no frameworks, works on a phone. It holds no
//!   data, so it needs no credentials; everything it shows comes from the API below.
//! * `GET  /api/overview`                      endpoints and recent inbox events
//! * `POST /api/endpoints`                     body `{"name":..,"adapter":..}`; the token is returned once
//! * `POST /api/endpoints/{name}/rotate`       a new token, returned once
//! * `DELETE /api/endpoints/{name}`
//!
//! The API needs the administrator token as `Authorization: Bearer ...`. Failed attempts share the
//! authentication rate limit. Secrets are write-only: tokens are stored hashed and shown only when created.
//! Text from agents is untrusted: the page renders it with `textContent` only, and the response carries a
//! strict Content-Security-Policy as a second line of defence.

use crate::app::{Core, HookError};
use crate::inbox::endpoint::EndpointError;
use http_body_util::{BodyExt, Full, Limited, combinators::BoxBody};
use hyper::body::{Bytes, Incoming};
use hyper::header::{CONTENT_TYPE, HeaderMap, HeaderValue, WWW_AUTHENTICATE};
use hyper::{Method, Response, StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};
use std::convert::Infallible;

type Resp = Response<BoxBody<Bytes, Infallible>>;

const PAGE: &str = include_str!("ui.html");
/// Only the page's own inline script and style, and only calls back to this origin.
const CSP: &str = "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; connect-src 'self'; \
                   base-uri 'none'; form-action 'none'; frame-ancestors 'none'";
/// API bodies are tiny; refuse anything larger.
const MAX_BODY: usize = 4096;
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
pub fn page() -> Resp {
    let mut r = reply(StatusCode::OK, "text/html; charset=utf-8", PAGE.to_owned());
    r.headers_mut()
        .insert("content-security-policy", HeaderValue::from_static(CSP));
    r.headers_mut()
        .insert("x-frame-options", HeaderValue::from_static("DENY"));
    r.headers_mut()
        .insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    r
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    let v = headers.get(hyper::header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, rest) = v.split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then(|| rest.trim())
}

fn auth_error(e: &HookError) -> Resp {
    match e {
        HookError::RateLimited => {
            let mut r = error(StatusCode::TOO_MANY_REQUESTS, "too many attempts");
            r.headers_mut()
                .insert(hyper::header::RETRY_AFTER, HeaderValue::from_static("5"));
            r
        }
        _ => {
            let mut r = error(StatusCode::UNAUTHORIZED, "administrator token required");
            r.headers_mut()
                .insert(WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
            r
        }
    }
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
    core: &Core,
    method: &Method,
    segs: &[&str],
    headers: &HeaderMap,
    body: Incoming,
) -> Option<Resp> {
    let is_page = matches!(segs, [""] | ["ui"]);
    if is_page {
        return Some(match *method {
            Method::GET | Method::HEAD => page(),
            _ => error(StatusCode::METHOD_NOT_ALLOWED, "method not allowed"),
        });
    }
    let ["api", rest @ ..] = segs else {
        return None;
    };
    let known = matches!(
        rest,
        ["overview"] | ["endpoints"] | ["endpoints", _] | ["endpoints", _, "rotate"]
    );
    if !known {
        return Some(error(StatusCode::NOT_FOUND, "not found"));
    }
    // Authenticate before reading any body.
    let Some(token) = bearer(headers) else {
        return Some(auth_error(&HookError::Unauthorized));
    };
    if let Err(e) = core.check_admin(token) {
        return Some(auth_error(&e));
    }
    Some(match (method, rest) {
        (&Method::GET, ["overview"]) => overview(core),
        (&Method::POST, ["endpoints"]) => create(core, body).await,
        (&Method::POST, ["endpoints", name, "rotate"]) => match core.rotate_endpoint(name) {
            Ok(t) => json_reply(StatusCode::OK, &json!({ "name": name, "token": t })),
            Err(e) => endpoint_error(&e),
        },
        (&Method::DELETE, ["endpoints", name]) => match core.revoke_endpoint(name) {
            Ok(()) => json_reply(StatusCode::OK, &json!({ "deleted": name })),
            Err(e) => endpoint_error(&e),
        },
        _ => error(StatusCode::METHOD_NOT_ALLOWED, "method not allowed"),
    })
}

async fn create(core: &Core, body: Incoming) -> Resp {
    let bytes = match Limited::new(body, MAX_BODY).collect().await {
        Ok(c) => c.to_bytes(),
        Err(_) => return error(StatusCode::PAYLOAD_TOO_LARGE, "body too large"),
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
