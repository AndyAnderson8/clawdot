//! Changes to tools, connections (MCP servers) and skills: the main loop does the work, these read the request body and map the answer.

use super::common::{
    MAX_BODY, MAX_SKILL_BODY, MAX_SKILL_IMPORT_BODY, Resp, error, fail_reply, json_reply,
    read_json, read_json_limit,
};
use crate::app::{Core, Op};
use crate::extensions::ExtOp;
use crate::http::{BODY_TIMEOUT, read_body};
use hyper::StatusCode;
use hyper::body::Incoming;
use serde::Deserialize;
use serde_json::Value;

/// Run a change to tools, connections, or skills in the main loop; the answer is the new listing.
pub(super) async fn ext(core: &Core, op: ExtOp) -> Resp {
    match core.portal_call(Op::Ext(op)).await {
        Ok(v) => json_reply(StatusCode::OK, &v),
        Err(f) => fail_reply(&f),
    }
}

/// `{"enabled": true|false}`.
pub(super) async fn enabled_flag(body: Incoming) -> Result<bool, (StatusCode, &'static str)> {
    match read_json::<Value>(body).await {
        Ok(v) => v["enabled"].as_bool().ok_or((
            StatusCode::BAD_REQUEST,
            "expected {\"enabled\": true or false}",
        )),
        Err(e) => Err(e),
    }
}

/// `POST /api/connections/{name}/accept`: an empty body or `{}` accepts every tool of the connection that waits
/// for review; `{"tool": "<name>"}` accepts that one (REQ-EXT-019).
pub(super) async fn accept_choice(
    body: Incoming,
) -> Result<Option<String>, (StatusCode, &'static str)> {
    let Some(bytes) = read_body(body, MAX_BODY, BODY_TIMEOUT).await else {
        return Err((StatusCode::PAYLOAD_TOO_LARGE, "body too large or too slow"));
    };
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(None);
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Choice {
        tool: Option<String>,
    }
    serde_json::from_slice::<Choice>(&bytes)
        .map(|c| c.tool)
        .map_err(|_| (StatusCode::BAD_REQUEST, "expected {} or {\"tool\": name}"))
}

/// `{"tier": "read_only"|"reversible"|"costly"|null}` (REQ-SEC-030).
pub(super) async fn tier_choice(
    body: Incoming,
) -> Result<Option<crate::session::Tier>, (StatusCode, &'static str)> {
    let v = read_json::<Value>(body).await?;
    match v.get("tier") {
        Some(Value::Null) => Ok(None),
        Some(t) => serde_json::from_value(t.clone()).map(Some).map_err(|_| {
            (
                StatusCode::BAD_REQUEST,
                "tier must be read_only, reversible, costly, or null",
            )
        }),
        None => Err((StatusCode::BAD_REQUEST, "expected {\"tier\": ..}")),
    }
}

/// Skills are addressed in the body, not the path, because their names are free text.
pub(super) async fn skill_action(core: &Core, action: &str, body: Incoming) -> Resp {
    let limit = if action == "import" {
        MAX_SKILL_IMPORT_BODY
    } else {
        MAX_SKILL_BODY
    };
    let v = match read_json_limit::<Value>(body, limit).await {
        Ok(v) => v,
        Err((s, m)) => return error(s, m),
    };
    let name = v["name"].as_str().unwrap_or("").to_owned();
    match action {
        "import" => ext(core, ExtOp::SkillImport(v)).await,
        "edit" => ext(core, ExtOp::SkillEdit(name, v["skill"].clone())).await,
        "delete" => ext(core, ExtOp::SkillDelete(name)).await,
        "accept" => ext(core, ExtOp::SkillAccept(name)).await,
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
