//! Settings: saving a change and exporting the saved settings (REQ-CFG-037..040, 065).

use super::common::{
    MAX_TOOL_BODY, Resp, attachment, error, fail_reply, json_limit_or_reply, json_reply,
};
use crate::app::{Core, Op};
use hyper::StatusCode;
use hyper::body::Incoming;
use serde_json::{Value, json};

/// `GET /api/settings`: every setting with its value, where the value came from, and what it may be.
pub(super) fn get(core: &Core) -> Resp {
    json_reply(StatusCode::OK, &core.ui_settings())
}

/// `PUT /api/settings`: save the changes in the body (an object of `{key: value|null}`).
pub(super) async fn put(core: &Core, body: Incoming) -> Resp {
    match json_limit_or_reply::<Value>(body, MAX_TOOL_BODY).await {
        Ok(v) => change_settings(core, &v).await,
        Err(r) => r,
    }
}

/// `GET /api/settings/export`: the saved settings as a file the page offers for download (REQ-CFG-065).
pub(super) fn export_settings(core: &Core) -> Resp {
    let v = core.export_settings();
    if v.is_null() {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Settings are not available yet.",
        );
    }
    attachment(json_reply(StatusCode::OK, &v), "clawdot-settings.json")
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
