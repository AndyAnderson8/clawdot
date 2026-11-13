//! The endpoints (webhook names with tokens) the portal creates, renames, rotates and revokes.

use super::common::{MAX_BODY, Resp, error, json_reply, read_json, saved};
use crate::app::Core;
use crate::http::{BODY_TIMEOUT, read_body};
use crate::inbox::endpoint::EndpointError;
use hyper::StatusCode;
use hyper::body::Incoming;
use serde::Deserialize;
use serde_json::{Value, json};

fn endpoint_error(e: &EndpointError) -> Resp {
    let status = match e {
        EndpointError::Exists => StatusCode::CONFLICT,
        EndpointError::NotFound => StatusCode::NOT_FOUND,
        EndpointError::InvalidName | EndpointError::InvalidSpokenName => StatusCode::BAD_REQUEST,
        EndpointError::NoRandomness => StatusCode::INTERNAL_SERVER_ERROR,
    };
    error(status, &e.to_string())
}

/// An endpoint as the portal lists it. `spoken_name` is `null` when none is set (the speaker then says `name`).
pub(super) fn endpoint_json(e: &crate::inbox::endpoint::Endpoint) -> Value {
    json!({
        "name": e.name, "adapter": e.adapter, "enabled": e.enabled, "created_at": e.created_at,
        "spoken_name": e.spoken_name,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewEndpoint {
    name: String,
    adapter: String,
    #[serde(default)]
    spoken_name: Option<String>,
}

/// Deserialises a field that must be present, even when it is `null` (a plain `Option` would accept its absence).
fn present<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Option::<String>::deserialize(d)
}

/// `PUT /api/endpoints/{name}`: `spoken_name` is text, or `null` (or blank) to clear it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EndpointUpdate {
    #[serde(deserialize_with = "present")]
    spoken_name: Option<String>,
}

pub(super) async fn create(core: &Core, body: Incoming) -> Resp {
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
    let mark = core.save_mark();
    match core.create_endpoint_with(&req.name, &req.adapter, req.spoken_name.as_deref()) {
        Ok((e, token)) => {
            if let Err(why) = core.saved_since(mark).await {
                // The token would be shown only in this answer, which says it failed. Take the endpoint back, so
                // that trying again makes it afresh instead of meeting "already exists".
                let _ = core.revoke_endpoint(&e.name);
                return crate::http::not_saved(why);
            }
            json_reply(
                StatusCode::CREATED,
                &json!({ "name": e.name, "adapter": e.adapter, "spoken_name": e.spoken_name, "token": token }),
            )
        }
        Err(e) => endpoint_error(&e),
    }
}

/// `PUT /api/endpoints/{name}`: set or clear the words the speaker names as the origin.
pub(super) async fn update(core: &Core, name: &str, body: Incoming) -> Resp {
    match read_json::<EndpointUpdate>(body).await {
        Ok(u) => {
            let mark = core.save_mark();
            let resp =
                match core.set_endpoint_spoken_name(name, u.spoken_name.as_deref().unwrap_or("")) {
                    Ok(e) => json_reply(StatusCode::OK, &endpoint_json(&e)),
                    Err(e) => endpoint_error(&e),
                };
            saved(core, mark, resp).await
        }
        Err(_) => error(
            StatusCode::BAD_REQUEST,
            "expected {\"spoken_name\": text or null}",
        ),
    }
}

/// `POST /api/endpoints/{name}/rotate`: a new token, returned once.
pub(super) async fn rotate(core: &Core, name: &str) -> Resp {
    let mark = core.save_mark();
    let resp = match core.rotate_endpoint(name) {
        Ok(t) => json_reply(StatusCode::OK, &json!({ "name": name, "token": t })),
        Err(e) => endpoint_error(&e),
    };
    saved(core, mark, resp).await
}

/// `DELETE /api/endpoints/{name}`.
pub(super) async fn delete(core: &Core, name: &str) -> Resp {
    let mark = core.save_mark();
    let resp = match core.revoke_endpoint(name) {
        Ok(()) => json_reply(StatusCode::OK, &json!({ "deleted": name })),
        Err(e) => endpoint_error(&e),
    };
    saved(core, mark, resp).await
}
