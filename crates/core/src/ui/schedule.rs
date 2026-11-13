//! Timers, alarms and reminders (REQ-CFG-027).

use super::common::{Resp, fail_reply, json_reply};
use crate::app::{Core, Op, SchedOp};
use hyper::StatusCode;

/// Timers, alarms, and reminders: the main loop does the work, so both the voice path and the portal use
/// one scheduler (REQ-CFG-027).
pub(super) async fn schedule(core: &Core, op: SchedOp) -> Resp {
    match core.portal_call(Op::Sched(op)).await {
        Ok(v) => json_reply(StatusCode::OK, &v),
        Err(f) => fail_reply(&f),
    }
}
