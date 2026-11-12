//! `POST /api/announce`: the HTTP side of the one-way announce path (REQ-INB-021, REQ-INB-025).
//!
//! Credentials are checked before the body is read. A token that starts with `cdt_` is an endpoint token;
//! anything else (or nothing, in open mode) is treated as an administrator session. The response never
//! repeats the text and carries nothing the caller could mistake for a decision.

use crate::admin::AdminError;
use crate::app::{AnnounceError, Announcement, Caller, Core, HookError, outcome_json};
use crate::http::{read_body, token_from};
use http_body_util::{BodyExt, Full, combinators::BoxBody};
use hyper::body::{Bytes, Incoming};
use hyper::header::{
    CONTENT_LENGTH, CONTENT_TYPE, HeaderMap, HeaderValue, RETRY_AFTER, WWW_AUTHENTICATE,
};
use hyper::{Response, StatusCode};
use serde_json::{Value, json};
use std::convert::Infallible;
use std::net::IpAddr;
use std::time::Duration;

pub(crate) type Resp = Response<BoxBody<Bytes, Infallible>>;

/// A text of 2,000 characters in JSON escapes stays well under this.
const MAX_BODY: usize = 32 * 1024;

pub(crate) fn reply(status: StatusCode, v: &Value) -> Resp {
    let mut r = Response::new(Full::new(Bytes::from(v.to_string())).boxed());
    *r.status_mut() = status;
    r.headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    r.headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    r.headers_mut().insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    r
}

pub(crate) fn fail(status: StatusCode, msg: &str) -> Resp {
    reply(status, &json!({ "error": msg }))
}

pub(crate) fn unauthorized() -> Resp {
    let mut r = fail(StatusCode::UNAUTHORIZED, "unauthorized");
    r.headers_mut()
        .insert(WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    r
}

pub(crate) fn limited() -> Resp {
    let mut r = fail(StatusCode::TOO_MANY_REQUESTS, "rate limited");
    r.headers_mut()
        .insert(RETRY_AFTER, HeaderValue::from_static("1"));
    r
}

fn hook_error(e: &HookError) -> Resp {
    match e {
        HookError::RateLimited => limited(),
        _ => unauthorized(),
    }
}

/// Every refusal looks the same to a caller without credentials, whatever state the portal is in
/// (REQ-INB-021): 401, or 429 when the shared failure budget is spent.
fn admin_error(e: AdminError) -> Resp {
    match e {
        AdminError::RateLimited => limited(),
        _ => unauthorized(),
    }
}

/// Handle one request (the method was checked by the router).
pub(crate) async fn handle(
    core: &Core,
    headers: &HeaderMap,
    peer: IpAddr,
    body: Incoming,
    body_timeout: Duration,
) -> Resp {
    let declared = headers
        .get(CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok());
    if declared.is_some_and(|n| n > MAX_BODY) {
        return fail(StatusCode::PAYLOAD_TOO_LARGE, "body too large");
    }
    let token = token_from(headers);
    // Endpoint tokens and administrator sessions look alike, so ask which one it is. Authenticate before
    // spending effort on the body.
    let endpoint_token = token.as_deref().filter(|t| core.is_endpoint_token(t));
    if endpoint_token.is_none() {
        if let Err(e) = core.check_admin(token.as_deref(), peer) {
            return admin_error(e);
        }
    }
    let Some(bytes) = read_body(body, MAX_BODY, body_timeout).await else {
        return fail(
            StatusCode::PAYLOAD_TOO_LARGE,
            "body too large, too slow or unreadable",
        );
    };
    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
        return fail(StatusCode::BAD_REQUEST, "body is not valid JSON");
    };
    let announcement = match Announcement::from_json(&value) {
        Ok(a) => a,
        Err(m) => return fail(StatusCode::BAD_REQUEST, &m),
    };
    let caller = endpoint_token.map_or(Caller::Admin, |t| Caller::Token(t, peer));
    match core.announce(caller, &announcement) {
        Ok(outcome) => reply(StatusCode::ACCEPTED, &outcome_json(outcome)),
        Err(AnnounceError::Hook(e)) => hook_error(&e),
        Err(AnnounceError::Endpoint(m)) => fail(StatusCode::BAD_REQUEST, &m),
    }
}
