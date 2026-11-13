//! Settings: saving a change and exporting the saved settings (REQ-CFG-037..040, 065).

use super::common::{Resp, attachment, error, fail_reply, json_reply};
use crate::app::{Core, Op};
use hyper::StatusCode;
use serde_json::{Value, json};

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
pub(super) async fn change_settings(core: &Core, patch: &Value) -> Resp {
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
