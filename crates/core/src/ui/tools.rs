//! Changes to tools, connections (MCP servers) and skills: the main loop does the work, these read the request body and map the answer.

use super::common::{
    MAX_BODY, MAX_SKILL_BODY, MAX_SKILL_IMPORT_BODY, MAX_TOOL_BODY, Resp, error, fail_reply,
    json_limit_or_reply, json_or_reply, json_reply, portal_json, read_json,
};
use crate::app::{Core, Op};
use crate::extensions::{self, ExtOp};
use crate::http::{BODY_TIMEOUT, read_body};
use hyper::StatusCode;
use hyper::body::Incoming;
use serde::Deserialize;
use serde_json::Value;

/// Run a change to tools, connections, or skills in the main loop; the answer is the new listing.
pub(super) async fn ext(core: &Core, op: ExtOp) -> Resp {
    portal_json(core, Op::Ext(op)).await
}

/// `POST /api/tools`: add a tool.
pub(super) async fn tool_add(core: &Core, body: Incoming) -> Resp {
    match json_limit_or_reply::<Value>(body, MAX_TOOL_BODY).await {
        Ok(v) => ext(core, ExtOp::ToolAdd(v)).await,
        Err(r) => r,
    }
}

/// `POST /api/tools/{name}/enabled`.
pub(super) async fn tool_enable(core: &Core, name: &str, body: Incoming) -> Resp {
    match enabled_flag(body).await {
        Ok(on) => ext(core, ExtOp::ToolEnable(name.to_owned(), on)).await,
        Err((s, m)) => error(s, m),
    }
}

/// `POST /api/tools/{name}/tier`.
pub(super) async fn tool_tier(core: &Core, name: &str, body: Incoming) -> Resp {
    match tier_choice(body).await {
        Ok(t) => ext(core, ExtOp::ToolTier(name.to_owned(), t)).await,
        Err((s, m)) => error(s, m),
    }
}

/// `POST /api/connections/test`: try a connection without saving it.
pub(super) async fn connection_test(core: &Core, body: Incoming) -> Resp {
    match json_or_reply::<Value>(body).await {
        Ok(v) => match extensions::test_connection(&v, |n| core.env().get(n)).await {
            Ok(r) => json_reply(StatusCode::OK, &r),
            Err(f) => fail_reply(&f),
        },
        Err(r) => r,
    }
}

/// `POST /api/connections`: add a connection.
pub(super) async fn connection_add(core: &Core, body: Incoming) -> Resp {
    match json_or_reply::<Value>(body).await {
        Ok(v) => ext(core, ExtOp::McpAdd(v)).await,
        Err(r) => r,
    }
}

/// `POST /api/connections/{name}/enabled`.
pub(super) async fn connection_enable(core: &Core, name: &str, body: Incoming) -> Resp {
    match enabled_flag(body).await {
        Ok(on) => ext(core, ExtOp::McpEnable(name.to_owned(), on)).await,
        Err((s, m)) => error(s, m),
    }
}

/// `POST /api/connections/{name}/accept`.
pub(super) async fn connection_accept(core: &Core, name: &str, body: Incoming) -> Resp {
    match accept_choice(body).await {
        Ok(tool) => ext(core, ExtOp::McpAccept(name.to_owned(), tool)).await,
        Err((s, m)) => error(s, m),
    }
}

/// `POST /api/skills`: add a skill.
pub(super) async fn skill_add(core: &Core, body: Incoming) -> Resp {
    match json_limit_or_reply::<Value>(body, MAX_SKILL_BODY).await {
        Ok(v) => ext(core, ExtOp::SkillAdd(v)).await,
        Err(r) => r,
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

/// `POST /api/connections/{name}/accept`: an empty body or `{}` accepts every tool of the connection that waits
/// for review; `{"tool": "<name>"}` accepts that one (REQ-EXT-019).
async fn accept_choice(body: Incoming) -> Result<Option<String>, (StatusCode, &'static str)> {
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
async fn tier_choice(
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
    let v = match json_limit_or_reply::<Value>(body, limit).await {
        Ok(v) => v,
        Err(r) => return r,
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
