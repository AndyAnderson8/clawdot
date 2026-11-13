//! Signing in, first-run setup, and changing the administrator password (REQ-CFG-018, 024, 069).

use super::common::{Resp, error, json_reply, read_json};
use crate::admin::{self, AdminError};
use crate::app::Core;
use hyper::StatusCode;
use hyper::body::Incoming;
use hyper::header::{HeaderValue, WWW_AUTHENTICATE};
use serde::Deserialize;
use serde_json::json;
use std::net::IpAddr;
use std::sync::Arc;

pub(super) fn admin_error(e: AdminError) -> Resp {
    let status = match e {
        AdminError::Unauthorized => StatusCode::UNAUTHORIZED,
        AdminError::RateLimited => StatusCode::TOO_MANY_REQUESTS,
        AdminError::SetupRequired | AdminError::NotPrivate | AdminError::OpenRefused => {
            StatusCode::FORBIDDEN
        }
        AdminError::AlreadySetUp | AdminError::NoPassword => StatusCode::CONFLICT,
        // Not 401: the page signs the user out on a 401, and a mistyped current password is not a lost session.
        AdminError::WindowClosed | AdminError::WrongCurrent => StatusCode::FORBIDDEN,
        AdminError::BadPassword => StatusCode::BAD_REQUEST,
        AdminError::Storage | AdminError::NoRandomness => StatusCode::INTERNAL_SERVER_ERROR,
    };
    let mut r = error(status, &e.to_string());
    match e {
        AdminError::RateLimited => {
            r.headers_mut()
                .insert(hyper::header::RETRY_AFTER, HeaderValue::from_static("5"));
        }
        AdminError::Unauthorized => {
            r.headers_mut()
                .insert(WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        }
        _ => {}
    }
    r
}

/// Hashing is deliberately slow; keep it off the async thread.
async fn hash_in_background(password: String) -> Result<String, AdminError> {
    tokio::task::spawn_blocking(move || admin::hash_password(&password))
        .await
        .unwrap_or(Err(AdminError::NoRandomness))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Setup {
    mode: String,
    password: Option<String>,
}

/// First-run setup (REQ-CFG-018): a password or open mode, from a private address only.
pub(super) async fn setup(core: &Arc<Core>, peer: IpAddr, body: Incoming) -> Resp {
    let req: Setup = match read_json(body).await {
        Ok(r) => r,
        Err((s, m)) => return error(s, m),
    };
    // Refuse strangers before any expensive work.
    if let Err(e) = core.admin_check_setup(peer) {
        return admin_error(e);
    }
    match req.mode.as_str() {
        "open" => match core.admin_setup_open(peer) {
            Ok(()) => json_reply(StatusCode::OK, &json!({ "mode": "open" })),
            Err(e) => admin_error(e),
        },
        "password" => {
            let Some(pw) = req.password.filter(|p| admin::password_ok(p)) else {
                return admin_error(AdminError::BadPassword);
            };
            let hash = match hash_in_background(pw).await {
                Ok(h) => h,
                Err(e) => return admin_error(e),
            };
            match core.admin_setup_password(peer, hash) {
                Ok(token) => json_reply(
                    StatusCode::OK,
                    &json!({ "mode": "password", "token": token }),
                ),
                Err(e) => admin_error(e),
            }
        }
        _ => error(StatusCode::BAD_REQUEST, "mode must be password or open"),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Login {
    password: String,
}

/// Sign in with the password (or, after an upgrade, the old administrator token); returns a session token.
pub(super) async fn login(core: &Arc<Core>, peer: IpAddr, body: Incoming) -> Resp {
    let req: Login = match read_json(body).await {
        Ok(r) => r,
        Err((s, m)) => return error(s, m),
    };
    if core.admin_mode() == admin::Mode::Open {
        return error(StatusCode::CONFLICT, "no password is set");
    }
    let stored = match core.admin_login_begin(peer) {
        Ok(h) => h,
        Err(e) => return admin_error(e),
    };
    let ok = tokio::task::spawn_blocking(move || admin::verify_password(&req.password, &stored))
        .await
        .unwrap_or(false);
    if !ok {
        return admin_error(AdminError::Unauthorized);
    }
    match core.admin_login_finish(peer) {
        Ok(token) => json_reply(StatusCode::OK, &json!({ "token": token })),
        Err(e) => admin_error(e),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChangePassword {
    /// The new password.
    password: String,
    /// The password in force now (for an installation that still has the old administrator token, that token).
    /// Required in password mode; there is none to give in open mode.
    #[serde(default)]
    current: Option<String>,
}

/// Set a new administrator password (REQ-CFG-024). The caller is already signed in, and in password mode must also
/// give the current password: a session left open on a shared screen, or lifted from a script, must not be enough to
/// take the account over. The check spends an attempt from the caller's address like a sign-in does, and runs off
/// the async thread because it is deliberately slow.
pub(super) async fn change_password(core: &Arc<Core>, peer: IpAddr, body: Incoming) -> Resp {
    let req: ChangePassword = match read_json(body).await {
        Ok(r) => r,
        Err((s, m)) => return error(s, m),
    };
    if !admin::password_ok(&req.password) {
        return admin_error(AdminError::BadPassword);
    }
    let stored = match core.admin_password_begin(peer) {
        Ok(s) => s,
        Err(e) => return admin_error(e),
    };
    if let Some(stored) = &stored {
        let Some(current) = req.current else {
            return admin_error(AdminError::WrongCurrent);
        };
        let ok = tokio::task::spawn_blocking({
            let stored = stored.clone();
            move || admin::verify_password(&current, &stored)
        })
        .await
        .unwrap_or(false);
        if !ok {
            return admin_error(AdminError::WrongCurrent);
        }
    }
    let hash = match hash_in_background(req.password).await {
        Ok(h) => h,
        Err(e) => return admin_error(e),
    };
    match core.admin_set_password(peer, stored.as_deref(), hash) {
        Ok(token) => json_reply(
            StatusCode::OK,
            &json!({ "mode": "password", "token": token }),
        ),
        Err(e) => admin_error(e),
    }
}

/// A new password during the local reset window, from a private address, (REQ-CFG-069).
pub(super) async fn recover_window(core: &Arc<Core>, peer: IpAddr, body: Incoming) -> Resp {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Pw {
        password: String,
    }
    let req: Pw = match read_json(body).await {
        Ok(r) => r,
        Err((s, m)) => return error(s, m),
    };
    if !admin::password_ok(&req.password) {
        return admin_error(AdminError::BadPassword);
    }
    if let Err(e) = core.admin_window_begin(peer) {
        return admin_error(e);
    }
    let hash = match hash_in_background(req.password).await {
        Ok(h) => h,
        Err(e) => return admin_error(e),
    };
    match core.admin_window_finish(peer, hash) {
        Ok(token) => json_reply(
            StatusCode::OK,
            &json!({ "mode": "password", "token": token }),
        ),
        Err(e) => admin_error(e),
    }
}
