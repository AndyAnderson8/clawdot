//! What the portal shows about the running assistant: the overview, the hive peers, recent log lines, the diagnostics file and the audit record.

use super::common::{Resp, attachment, error, json_reply, query_decode};
use super::endpoints::endpoint_json;
use crate::app::Core;
use hyper::StatusCode;
use serde_json::{Value, json};

/// Events shown in the overview, newest first.
const MAX_EVENTS: usize = 50;
/// Longest event text sent to the page.
const MAX_TEXT: usize = 600;

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

pub(super) fn overview(core: &Core) -> Resp {
    let endpoints: Vec<Value> = core.endpoints().iter().map(endpoint_json).collect();
    let mut events = core.events();
    events.sort_by_key(|e| std::cmp::Reverse(e.id));
    let events: Vec<Value> = events
        .iter()
        .take(MAX_EVENTS)
        .map(|e| {
            json!({
                "id": e.id, "endpoint": e.endpoint, "source": e.source, "origin": e.origin_name(),
                "title": clip(&e.title),
                "text": clip(&e.text), "state": state_label(&e.state), "received_at": e.received_at,
            })
        })
        .collect();
    json_reply(
        StatusCode::OK,
        &json!({ "now": core.now(), "endpoints": endpoints, "events": events }),
    )
}

/// `GET /api/hive/peers`: this speaker's identity and the speakers found on the network (hive milestone H0).
/// 404 when the build has no hive support or it was not started. Nothing secret is in the answer.
pub(super) fn hive_peers(core: &Core) -> Resp {
    #[cfg(feature = "hive")]
    {
        let name = core.setting("device_name");
        if let Some(h) = core.hive() {
            return json_reply(
                StatusCode::OK,
                &h.peers_json(name.as_str().unwrap_or("ClawDot")),
            );
        }
    }
    let _ = core;
    error(
        StatusCode::NOT_FOUND,
        "the hive is not enabled on this speaker",
    )
}

/// `GET /api/logs?limit=`: the core's recent warnings and errors, newest first (REQ-CFG-064).
pub(super) fn logs(query: Option<&str>) -> Resp {
    let limit = query
        .unwrap_or("")
        .split('&')
        .find_map(|p| p.strip_prefix("limit="))
        .and_then(|n| n.parse::<usize>().ok())
        .unwrap_or(100)
        .clamp(1, crate::diag::MAX_LINES);
    json_reply(
        StatusCode::OK,
        &json!({
            "lines": crate::diag::recent(limit),
            "max_lines": crate::diag::MAX_LINES,
            "note": "Only ClawDot's own messages, kept in memory since it started. No audio and none of what you said.",
        }),
    )
}

/// `GET /api/diagnostics`: one document for a bug report (REQ-CFG-063). It holds the versions, the settings and
/// where each came from, the webhook names (never tokens), counts of tools and of audit records, and the
/// recent problems. It holds no secret, no audio, no words of the user, no note text and no event text.
pub(super) fn diagnostics(core: &Core) -> Resp {
    let mut settings = core.ui_settings();
    // The persona is the user's own text and can be long; its size is enough for a bug report.
    if let Some(p) = settings["fields"]["persona"].as_object_mut() {
        let chars = p
            .get("value")
            .and_then(Value::as_str)
            .map_or(0, |t| t.chars().count());
        p.insert("value".into(), json!(format!("({chars} characters)")));
    }
    let endpoints: Vec<Value> = core
        .endpoints()
        .iter()
        .map(|e| json!({ "name": e.name, "adapter": e.adapter, "enabled": e.enabled, "created_at": e.created_at }))
        .collect();
    let listing = core.ui_tools();
    let count = |source: &str| {
        listing["tools"]
            .as_array()
            .map_or(0, |t| t.iter().filter(|t| t["source"] == source).count())
    };
    let servers: Vec<Value> = listing["mcp"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|s| {
                    json!({
                        "name": s["name"], "connected": s["connected"], "enabled": s["enabled"],
                        "error": s["error"].as_str().map(crate::audit::redact_text),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    // The status snapshot lists what is scheduled with its label ("call the doctor"): the user's own words. A bug
    // report needs how many there are and of what kind, not what they say.
    let mut status = core.ui_status();
    if let Some(items) = status.get_mut("scheduled").and_then(Value::as_array_mut) {
        for item in items {
            if let Some(o) = item.as_object_mut() {
                o.remove("label");
            }
        }
    }
    let doc = json!({
        "format": "clawdot-diagnostics",
        "generated_at": core.now(),
        "version": env!("CARGO_PKG_VERSION"),
        "build": crate::diag::build_info(),
        "status": status,
        "settings": {
            "fields": settings["fields"], "restart_needed": settings["restart_needed"],
            "llm_key_present": settings["llm"]["key_present"],
        },
        "endpoints": endpoints,
        "tools": {
            "builtin": count("builtin"), "http": count("http"), "mcp": count("mcp"),
            "skills": listing["skills"].as_array().map_or(0, Vec::len),
            "connections": servers,
            "problems": listing["problems"],
        },
        "audit": core.audit().map(|a| json!({ "summary": a.summary(), "health": {
            "bytes": a.health().bytes, "corrupt_lines": a.health().corrupt_lines, "write_errors": a.health().write_errors,
        }, "policy": { "dry_run": a.policy().dry_run, "max_actions_per_day": a.policy().max_actions_per_day,
            "max_costly_per_day": a.policy().max_costly_per_day } })),
        "recent_problems": crate::diag::recent(50),
        "note": "No secrets, audio, spoken words, notes or event text are included.",
    });
    attachment(json_reply(StatusCode::OK, &doc), "clawdot-diagnostics.json")
}

/// `GET /api/audit`: the record of what the assistant did (REQ-SEC-014), newest first. Filters: `limit`
/// (default 50, at most 200), `before` (a record id, for the next page), `tool`, `tier`, `decision` (a value, or
/// a prefix ending in `:` such as `blocked:`), `trigger` (`voice`, `event`, `event:<source>`) and `since` (Unix
/// seconds). Argument and result text in the records comes from outside: show it as data.
pub(super) fn audit(core: &Core, query: Option<&str>) -> Resp {
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

fn parse_audit_query(raw: &str) -> Result<crate::audit::Query, String> {
    let mut q = crate::audit::Query::default();
    for pair in raw.split('&').filter(|p| !p.is_empty()).take(16) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let (Some(k), Some(v)) = (query_decode(k), query_decode(v)) else {
            return Err("The query has a broken % escape or is not text.".into());
        };
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
