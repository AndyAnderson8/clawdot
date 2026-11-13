//! Timers, alarms and reminders (REQ-CFG-027).

use super::common::{Resp, error, fail_reply, json_reply, read_json};
use crate::app::{Core, Op, SchedOp};
use hyper::StatusCode;
use hyper::body::Incoming;
use serde_json::Value;

/// `GET /api/schedule`: what is scheduled.
pub(super) async fn list(core: &Core) -> Resp {
    schedule(core, SchedOp::List).await
}

/// `POST /api/schedule`: add a timer, alarm or reminder.
pub(super) async fn add(core: &Core, body: Incoming) -> Resp {
    match read_json::<Value>(body).await {
        Ok(v) => schedule(core, SchedOp::Add(v)).await,
        Err((s, m)) => error(s, m),
    }
}

/// `PUT /api/schedule/{id}`: change one.
pub(super) async fn edit(core: &Core, id: &str, body: Incoming) -> Resp {
    match (id.parse::<u64>(), read_json::<Value>(body).await) {
        (Ok(id), Ok(v)) => schedule(core, SchedOp::Edit(id, v)).await,
        (Err(_), _) => error(StatusCode::NOT_FOUND, "no such timer, alarm, or reminder"),
        (_, Err((s, m))) => error(s, m),
    }
}

/// `DELETE /api/schedule/{id}`: cancel one.
pub(super) async fn cancel(core: &Core, id: &str) -> Resp {
    match id.parse::<u64>() {
        Ok(id) => schedule(core, SchedOp::Cancel(id)).await,
        Err(_) => error(StatusCode::NOT_FOUND, "no such timer, alarm, or reminder"),
    }
}

/// Timers, alarms, and reminders: the main loop does the work, so both the voice path and the portal use
/// one scheduler (REQ-CFG-027).
async fn schedule(core: &Core, op: SchedOp) -> Resp {
    match core.portal_call(Op::Sched(op)).await {
        Ok(v) => json_reply(StatusCode::OK, &v),
        Err(f) => fail_reply(&f),
    }
}
