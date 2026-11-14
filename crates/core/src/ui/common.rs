//! Helpers every portal handler shares: building a reply, reading a bounded JSON body, and the body size limits.

use crate::app::{Core, Fail, SaveMark};
use crate::http::{BODY_TIMEOUT, read_body};
use http_body_util::{BodyExt, Full, combinators::BoxBody};
use hyper::body::{Bytes, Incoming};
use hyper::header::{CONTENT_TYPE, HeaderMap, HeaderValue};
use hyper::{Response, StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};
use std::convert::Infallible;

pub(super) type Resp = Response<BoxBody<Bytes, Infallible>>;

/// API bodies are tiny; refuse anything larger.
pub(super) const MAX_BODY: usize = 4096;
/// A tool description with its parameters, and a skills file.
pub(super) const MAX_TOOL_BODY: usize = 16 * 1024;
pub(super) const MAX_SKILL_BODY: usize = 64 * 1024;
/// A folder of skills sent as files to import.
pub(super) const MAX_SKILL_IMPORT_BODY: usize = 384 * 1024;

pub(super) fn reply(status: StatusCode, content_type: &'static str, body: String) -> Resp {
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

pub(super) fn json_reply(status: StatusCode, v: &Value) -> Resp {
    reply(status, "application/json", v.to_string())
}

pub(super) fn error(status: StatusCode, msg: &str) -> Resp {
    json_reply(status, &json!({ "error": msg }))
}

pub(super) fn bearer(headers: &HeaderMap) -> Option<&str> {
    let v = headers.get(hyper::header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, rest) = v.split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then(|| rest.trim())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct KeyBody {
    pub(super) key: String,
}

/// The value of `key` in a query string, with percent-escapes and `+` decoded; `None` if absent or not text.
pub(super) fn query_param(query: Option<&str>, key: &str) -> Option<String> {
    let raw = query?
        .split('&')
        .find_map(|p| p.strip_prefix(key)?.strip_prefix('='))?;
    query_decode(raw)
}

/// Mark a response as a file to save.
pub(super) fn attachment(mut r: Resp, name: &'static str) -> Resp {
    if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename=\"{name}\"")) {
        r.headers_mut().insert("content-disposition", v);
    }
    r
}

/// Percent-decode a query component (also `+` as a space). `None` when an escape is broken (`%` followed by two
/// characters that are not hex digits) or the result is not UTF-8: a request is refused rather than guessed at. A
/// `%` with fewer than two characters after it is not an escape and stays as typed.
pub(super) fn query_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while let Some(&c) = b.get(i) {
        match c {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                let digit = |x: u8| char::from(x).to_digit(16);
                let (h, l) = (digit(b[i + 1])?, digit(b[i + 2])?);
                out.push(u8::try_from(h * 16 + l).ok()?);
                i += 2;
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8(out).ok()
}

/// Map a failed portal request to a status code and a message the page can show as written.
pub(super) fn fail_reply(f: &Fail) -> Resp {
    let status = match f {
        Fail::NotFound(_) => StatusCode::NOT_FOUND,
        Fail::Invalid(_) => StatusCode::BAD_REQUEST,
        Fail::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
        Fail::NotSaved(_, true) => StatusCode::INSUFFICIENT_STORAGE,
        Fail::NotSaved(_, false) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    error(status, &f.to_string())
}

pub(super) async fn read_json<T: serde::de::DeserializeOwned>(
    body: Incoming,
) -> Result<T, (StatusCode, &'static str)> {
    read_json_limit(body, MAX_BODY).await
}

pub(super) async fn read_json_limit<T: serde::de::DeserializeOwned>(
    body: Incoming,
    max: usize,
) -> Result<T, (StatusCode, &'static str)> {
    let Some(bytes) = read_body(body, max, BODY_TIMEOUT).await else {
        return Err((StatusCode::PAYLOAD_TOO_LARGE, "body too large or too slow"));
    };
    serde_json::from_slice(&bytes).map_err(|_| (StatusCode::BAD_REQUEST, "bad request body"))
}

/// [`read_json`] with the failure already turned into the reply to send: `Err(reply)` is returned as is.
#[allow(clippy::result_large_err)] // the error is the reply itself, sent at once
pub(super) async fn json_or_reply<T: serde::de::DeserializeOwned>(
    body: Incoming,
) -> Result<T, Resp> {
    read_json(body).await.map_err(|(s, m)| error(s, m))
}

/// [`read_json_limit`] with the failure already turned into the reply to send.
#[allow(clippy::result_large_err)] // the error is the reply itself, sent at once
pub(super) async fn json_limit_or_reply<T: serde::de::DeserializeOwned>(
    body: Incoming,
    max: usize,
) -> Result<T, Resp> {
    read_json_limit(body, max)
        .await
        .map_err(|(s, m)| error(s, m))
}

/// Run a change to tools, connections or skills in the main loop (see [`Core::portal_call`]); the answer is the
/// new listing.
pub(super) async fn portal_json(core: &Core, op: crate::app::Op) -> Resp {
    match core.portal_call(op).await {
        Ok(v) => json_reply(StatusCode::OK, &v),
        Err(f) => fail_reply(&f),
    }
}

/// Turn a success into the truth when what it changed could not be saved (see [`Core::saved_since`]).
pub(super) async fn saved(core: &Core, mark: SaveMark, resp: Resp) -> Resp {
    if resp.status().is_success() {
        if let Err(e) = core.saved_since(mark).await {
            return crate::http::not_saved(e);
        }
    }
    resp
}
