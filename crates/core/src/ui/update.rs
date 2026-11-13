//! Looking for and installing a newer version (REQ-UPD-005, 006). Builds without TLS cannot reach the release server, so they answer that plainly.

use super::common::{Resp, error};
#[cfg(feature = "tls")]
use super::common::{json_reply, read_json};
use crate::app::Core;
use hyper::StatusCode;
use hyper::body::Incoming;
#[cfg(feature = "tls")]
use serde_json::Value;

/// `GET /api/update`: whether a newer version exists, when it was last looked for, and whether it can be installed
/// here (REQ-UPD-005).
#[cfg(feature = "tls")]
pub(super) fn update_status(core: &Core) -> Resp {
    match core.updater() {
        Some(u) => json_reply(StatusCode::OK, &u.status()),
        None => error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Updates are not running on this device.",
        ),
    }
}

#[cfg(not(feature = "tls"))]
pub(super) fn update_status(_core: &Core) -> Resp {
    error(
        StatusCode::SERVICE_UNAVAILABLE,
        "This build does not check for updates.",
    )
}

/// `POST /api/update/check`: look for a newer version now. The answer is the same document as `GET /api/update`;
/// a failed look is in its `error` field, not an HTTP error.
#[cfg(feature = "tls")]
pub(super) async fn update_check(core: &Core) -> Resp {
    let Some(u) = core.updater() else {
        return update_status(core);
    };
    let _ = u.check().await;
    json_reply(StatusCode::OK, &u.status())
}

#[cfg(not(feature = "tls"))]
pub(super) async fn update_check(core: &Core) -> Resp {
    update_status(core)
}

/// `POST /api/update/apply` with `{"version": "1.2.3"}`: install the version the person confirmed (REQ-UPD-006).
/// It starts at once and the page watches `GET /api/update`.
#[cfg(feature = "tls")]
pub(super) async fn update_apply(core: &Core, body: Incoming) -> Resp {
    let Some(u) = core.updater() else {
        return update_status(core);
    };
    let v = match read_json::<Value>(body).await {
        Ok(v) => v,
        Err((s, m)) => return error(s, m),
    };
    let Some(version) = v["version"].as_str() else {
        return error(StatusCode::BAD_REQUEST, "Say which version to install.");
    };
    match u.start_apply(version) {
        Ok(()) => json_reply(StatusCode::ACCEPTED, &u.status()),
        Err(why) => error(StatusCode::CONFLICT, &why),
    }
}

#[cfg(not(feature = "tls"))]
pub(super) async fn update_apply(core: &Core, _body: Incoming) -> Resp {
    update_status(core)
}
