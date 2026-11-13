//! The embedded web UI and its JSON API (REQ-CFG-001, 003..006, 008, 018..024).
//!
//! * `GET /` and `GET /ui`: one static page. No external assets, works on a phone, built from `portal/` (Preact
//!   and TypeScript, bundled into the one file). It holds no data, so it needs no credentials; everything it
//!   shows comes from the API below.
//! * `GET  /api/auth`                          mode (setup, password, open) (no sign-in)
//! * `POST /api/setup`                         first run: `{"mode":"password","password":..}` or `{"mode":"open"}`
//! * `POST /api/login`                         `{"password":..}`; returns a session token
//! * `POST /api/logout`                        ends the session
//! * `POST /api/admin/password`                `{"password":..}`; sets a new password
//! * `POST /api/recover/window`                `{"password":..}`; during the local reset window (REQ-CFG-069)
//! * `GET  /api/overview`                      endpoints and recent agent events
//! * `GET  /api/schedule`, `POST /api/schedule`, `PUT|DELETE /api/schedule/{id}`   timers, alarms, reminders
//! * `POST /api/connections/{name}/accept`   body `{}` or `{"tool":..}`; approves what a server now offers (REQ-EXT-019)
//! * `POST /api/tools/{name}/tier`            body `{"tier":..|null}`; the administrator's risk tier for a tool (REQ-SEC-030)
//! * `GET  /api/tools`, `/api/settings`, `/api/about`   what the Tools, Connections, Settings tabs show
//! * `PUT  /api/settings`                      body `{key: value|null, ..}`; saves some settings (admin only)
//! * `GET  /api/audit`                         what the assistant did, newest first (filters and paging in `audit`)
//! * `GET  /api/notes`, `POST /api/notes`, `DELETE /api/notes[/{id}]`   the notes the user asked to keep (REQ-CFG-044, 061)
//! * `GET  /api/status`                      what the assistant is doing (model, satellites, tools, usage, schedule)
//! * `POST /api/speak`                         body `{"text":..}`; the speaker says it ("test speak"). `202 {"ok":true}`
//!   when queued; otherwise `{"ok":false,"reason":"no_voice|no_speaker|muted|busy|quiet_hours","message":..}` (REQ-CFG-070)
//! * `POST /api/endpoints`                     body `{"name":..,"adapter":..,"spoken_name"?:..}`; the token is returned once
//! * `PUT  /api/endpoints/{name}`              body `{"spoken_name": text|null}`; the words the speaker names as the origin
//! * `POST /api/endpoints/{name}/rotate`       a new token, returned once
//! * `DELETE /api/endpoints/{name}`
//!
//! The rest needs a session token as `Authorization: Bearer ...` (password mode) or a private-network peer
//! (open mode). Failed attempts, sign-ins, and setup tries share the
//! authentication rate limit. Secrets are write-only: tokens and the password are stored hashed.
//! Text from agents is untrusted: the page renders it as text only (Preact text children, never HTML injection), and the response carries a
//! strict Content-Security-Policy as a second line of defence.

use crate::admin;
use crate::app::{Core, SchedOp};
use crate::extensions::{self, ExtOp};
use hyper::body::Incoming;
use hyper::header::HeaderMap;
use hyper::{Method, StatusCode};
use serde_json::{Value, json};
use std::net::IpAddr;
use std::sync::Arc;

use self::auth::{admin_error, change_password, login, recover_window, setup};
use self::common::{
    MAX_SKILL_BODY, MAX_TOOL_BODY, Resp, bearer, error, fail_reply, json_reply, query_param,
    read_json, read_json_limit, saved,
};
use self::endpoints::{EndpointUpdate, create, endpoint_error, endpoint_json};
use self::models::{llm_key_clear, llm_key_set, models, models_with_key};
use self::notes::{notes_add, notes_edit, notes_forget, notes_list, notes_search};
use self::page::page;
use self::schedule::schedule;
use self::settings::{change_settings, export_settings};
use self::speech::{
    realtime_key_clear, realtime_key_set, speak, speech_key_clear, speech_key_set, voices,
};
use self::status::{audit, diagnostics, hive_peers, logs, overview};
use self::tools::{accept_choice, enabled_flag, ext, skill_action, tier_choice};
use self::update::{update_apply, update_check, update_status};

mod auth;
mod common;
mod endpoints;
mod models;
mod notes;
mod page;
mod schedule;
mod settings;
mod speech;
mod status;
mod tools;
mod update;

#[cfg(test)]
mod rekey_tests;
#[cfg(test)]
mod tests;

/// The release notes, shown on the About card (REQ-CFG-013, 025).
const CHANGELOG: &str = include_str!("../../../CHANGELOG.md");
/// What a route does. The dispatch in `handle` matches on this with no catch-all arm, so a handler without a route,
/// or an id nothing handles, does not compile (an id missing from the table is a dead-code error).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Id {
    AuthState,
    Setup,
    Login,
    RecoverWindow,
    Logout,
    Overview,
    Status,
    Speak,
    EndpointCreate,
    EndpointUpdate,
    EndpointRotate,
    EndpointDelete,
    AdminPassword,
    ToolsList,
    ToolAdd,
    ToolDelete,
    ToolEnable,
    ToolTier,
    ConnectionTest,
    ConnectionAdd,
    ConnectionDelete,
    ConnectionEnable,
    ConnectionAccept,
    SkillAction,
    SkillAdd,
    SettingsGet,
    SettingsPut,
    SettingsExport,
    Diagnostics,
    Logs,
    Audit,
    NotesList,
    NoteAdd,
    NotesForgetAll,
    NoteEdit,
    NoteForget,
    ModelsGet,
    ModelsAsk,
    LlmKeySet,
    LlmKeyClear,
    SpeechKeySet,
    SpeechKeyClear,
    RealtimeKeySet,
    RealtimeKeyClear,
    Voices,
    HivePeers,
    UpdateStatus,
    UpdateCheck,
    UpdateApply,
    About,
    ScheduleList,
    ScheduleAdd,
    ScheduleEdit,
    ScheduleCancel,
}

use crate::routes::{Auth, Row, find, fit};
use Auth::{Open, SignedIn};

type Route = Row<Id>;

const fn r(method: &'static str, path: &'static str, auth: Auth, id: Id) -> Route {
    Row::new(method, path, auth, id)
}

/// The portal's routes under `/api` (the rest are in `http::ROUTES`; see `routes.rs` for why there are two). This table
/// is the only place these routes are written in code. The 404 for an unknown path, the 405 for a known path with
/// the wrong method, the sign-in requirement and the test of it, and the check against `docs/api/openapi.yaml`
/// (`tests/openapi.rs`) are all read from it. Order matters only between rows of the same method that could both
/// match; a test refuses a row that an earlier one hides.
const ROUTES: &[Route] = &[
    r("GET", "/api/auth", Open, Id::AuthState),
    r("POST", "/api/setup", Open, Id::Setup),
    r("POST", "/api/login", Open, Id::Login),
    r("POST", "/api/recover/window", Open, Id::RecoverWindow),
    r("POST", "/api/logout", Open, Id::Logout),
    r("GET", "/api/overview", SignedIn, Id::Overview),
    r("GET", "/api/status", SignedIn, Id::Status),
    r("POST", "/api/speak", SignedIn, Id::Speak),
    r("POST", "/api/endpoints", SignedIn, Id::EndpointCreate),
    r(
        "POST",
        "/api/endpoints/{name}/rotate",
        SignedIn,
        Id::EndpointRotate,
    ),
    r("PUT", "/api/endpoints/{name}", SignedIn, Id::EndpointUpdate),
    r(
        "DELETE",
        "/api/endpoints/{name}",
        SignedIn,
        Id::EndpointDelete,
    ),
    r("POST", "/api/admin/password", SignedIn, Id::AdminPassword),
    r("GET", "/api/tools", SignedIn, Id::ToolsList),
    r("GET", "/api/skills", SignedIn, Id::ToolsList),
    r("POST", "/api/tools", SignedIn, Id::ToolAdd),
    r("DELETE", "/api/tools/{name}", SignedIn, Id::ToolDelete),
    r(
        "POST",
        "/api/tools/{name}/enabled",
        SignedIn,
        Id::ToolEnable,
    ),
    r("POST", "/api/tools/{name}/tier", SignedIn, Id::ToolTier),
    r(
        "POST",
        "/api/connections/test",
        SignedIn,
        Id::ConnectionTest,
    ),
    r("POST", "/api/connections", SignedIn, Id::ConnectionAdd),
    r(
        "DELETE",
        "/api/connections/{name}",
        SignedIn,
        Id::ConnectionDelete,
    ),
    r(
        "POST",
        "/api/connections/{name}/enabled",
        SignedIn,
        Id::ConnectionEnable,
    ),
    r(
        "POST",
        "/api/connections/{name}/accept",
        SignedIn,
        Id::ConnectionAccept,
    ),
    r("POST", "/api/skills/{action}", SignedIn, Id::SkillAction),
    r("POST", "/api/skills", SignedIn, Id::SkillAdd),
    r("GET", "/api/settings", SignedIn, Id::SettingsGet),
    r("PUT", "/api/settings", SignedIn, Id::SettingsPut),
    r("GET", "/api/settings/export", SignedIn, Id::SettingsExport),
    r("GET", "/api/diagnostics", SignedIn, Id::Diagnostics),
    r("GET", "/api/logs", SignedIn, Id::Logs),
    r("GET", "/api/audit", SignedIn, Id::Audit),
    r("GET", "/api/notes", SignedIn, Id::NotesList),
    r("POST", "/api/notes", SignedIn, Id::NoteAdd),
    r("DELETE", "/api/notes", SignedIn, Id::NotesForgetAll),
    r("PUT", "/api/notes/{id}", SignedIn, Id::NoteEdit),
    r("DELETE", "/api/notes/{id}", SignedIn, Id::NoteForget),
    r("GET", "/api/models", SignedIn, Id::ModelsGet),
    r("POST", "/api/models", SignedIn, Id::ModelsAsk),
    r("PUT", "/api/llm/key", SignedIn, Id::LlmKeySet),
    r("DELETE", "/api/llm/key", SignedIn, Id::LlmKeyClear),
    r("PUT", "/api/speech/key", SignedIn, Id::SpeechKeySet),
    r("DELETE", "/api/speech/key", SignedIn, Id::SpeechKeyClear),
    r("PUT", "/api/realtime/key", SignedIn, Id::RealtimeKeySet),
    r(
        "DELETE",
        "/api/realtime/key",
        SignedIn,
        Id::RealtimeKeyClear,
    ),
    r("GET", "/api/voices", SignedIn, Id::Voices),
    r("GET", "/api/hive/peers", SignedIn, Id::HivePeers),
    r("GET", "/api/update", SignedIn, Id::UpdateStatus),
    r("POST", "/api/update/check", SignedIn, Id::UpdateCheck),
    r("POST", "/api/update/apply", SignedIn, Id::UpdateApply),
    r("GET", "/api/about", SignedIn, Id::About),
    r("GET", "/api/schedule", SignedIn, Id::ScheduleList),
    r("POST", "/api/schedule", SignedIn, Id::ScheduleAdd),
    r("PUT", "/api/schedule/{id}", SignedIn, Id::ScheduleEdit),
    r("DELETE", "/api/schedule/{id}", SignedIn, Id::ScheduleCancel),
];

/// The rows of this table as `(METHOD, "/api/path/{param}")`, for the OpenAPI check (through
/// `http::documented_routes`).
pub(crate) fn api_routes() -> impl Iterator<Item = (&'static str, &'static str)> {
    ROUTES.iter().map(|r| (r.method, r.path))
}

/// A 405 in the portal's own style (cache headers included), for the page.
pub(crate) fn method_not_allowed() -> Resp {
    error(StatusCode::METHOD_NOT_ALLOWED, "method not allowed")
}

/// The static page, for `GET` and `HEAD` on `/` and `/ui`.
pub(crate) fn serve_page(headers: &HeaderMap) -> Resp {
    page(
        headers
            .get(hyper::header::ACCEPT_ENCODING)
            .and_then(|v| v.to_str().ok()),
    )
}

/// Handle `/api/...`. Returns `None` for any other path so the caller can continue routing.
pub(crate) async fn handle(
    core: &Arc<Core>,
    method: &Method,
    segs: &[&str],
    query: Option<&str>,
    headers: &HeaderMap,
    peer: IpAddr,
    body: Incoming,
) -> Option<Resp> {
    if segs.first() != Some(&"api") {
        return None;
    }
    let Some((route, [p, _])) = find(ROUTES, method.as_str(), segs) else {
        // A path that exists for some other method is a 405 once the caller is signed in; anything else is a 404.
        // The open routes are not in this check: a wrong method on them is a 404.
        let known = ROUTES
            .iter()
            .any(|r| r.auth == SignedIn && fit(r.path, segs).is_some());
        if !known {
            return Some(error(StatusCode::NOT_FOUND, "not found"));
        }
        if let Err(e) = core.check_admin(bearer(headers), peer) {
            return Some(admin_error(e));
        }
        return Some(error(StatusCode::METHOD_NOT_ALLOWED, "method not allowed"));
    };
    // Authenticate before reading any body.
    if route.auth == SignedIn {
        if let Err(e) = core.check_admin(bearer(headers), peer) {
            return Some(admin_error(e));
        }
    }
    Some(match route.id {
        Id::AuthState => {
            let mut state = core.admin_state();
            state["peer_private"] = json!(admin::is_private(peer));
            if !admin::is_private(peer) {
                // Whether the reset window is open is for the local network only.
                state["reset_window_secs"] = json!(0);
            }
            state["version"] = json!(env!("CARGO_PKG_VERSION"));
            json_reply(StatusCode::OK, &state)
        }
        Id::Setup => setup(core, peer, body).await,
        Id::Login => login(core, peer, body).await,
        Id::RecoverWindow => recover_window(core, peer, body).await,
        Id::Logout => {
            if let Some(t) = bearer(headers) {
                core.admin_logout(t);
            }
            json_reply(StatusCode::OK, &json!({ "signed_out": true }))
        }
        Id::Overview => overview(core),
        Id::Status => json_reply(
            StatusCode::OK,
            &json!({ "now": core.now(), "status": core.ui_status() }),
        ),
        Id::Speak => speak(core, body).await,
        Id::EndpointCreate => create(core, body).await,
        Id::EndpointUpdate => match read_json::<EndpointUpdate>(body).await {
            Ok(u) => {
                let mark = core.save_mark();
                let resp = match core
                    .set_endpoint_spoken_name(p, u.spoken_name.as_deref().unwrap_or(""))
                {
                    Ok(e) => json_reply(StatusCode::OK, &endpoint_json(&e)),
                    Err(e) => endpoint_error(&e),
                };
                saved(core, mark, resp).await
            }
            Err(_) => error(
                StatusCode::BAD_REQUEST,
                "expected {\"spoken_name\": text or null}",
            ),
        },
        Id::EndpointRotate => {
            let mark = core.save_mark();
            let resp = match core.rotate_endpoint(p) {
                Ok(t) => json_reply(StatusCode::OK, &json!({ "name": p, "token": t })),
                Err(e) => endpoint_error(&e),
            };
            saved(core, mark, resp).await
        }
        Id::EndpointDelete => {
            let mark = core.save_mark();
            let resp = match core.revoke_endpoint(p) {
                Ok(()) => json_reply(StatusCode::OK, &json!({ "deleted": p })),
                Err(e) => endpoint_error(&e),
            };
            saved(core, mark, resp).await
        }
        Id::AdminPassword => change_password(core, peer, body).await,
        Id::ToolsList => json_reply(StatusCode::OK, &core.ui_tools()),
        Id::ToolAdd => match read_json_limit::<Value>(body, MAX_TOOL_BODY).await {
            Ok(v) => ext(core, ExtOp::ToolAdd(v)).await,
            Err((s, m)) => error(s, m),
        },
        Id::ToolDelete => ext(core, ExtOp::ToolDelete(p.to_owned())).await,
        Id::ToolEnable => match enabled_flag(body).await {
            Ok(on) => ext(core, ExtOp::ToolEnable(p.to_owned(), on)).await,
            Err((s, m)) => error(s, m),
        },
        Id::ToolTier => match tier_choice(body).await {
            Ok(t) => ext(core, ExtOp::ToolTier(p.to_owned(), t)).await,
            Err((s, m)) => error(s, m),
        },
        Id::ConnectionTest => match read_json::<Value>(body).await {
            Ok(v) => match extensions::test_connection(&v, |n| core.env().get(n)).await {
                Ok(r) => json_reply(StatusCode::OK, &r),
                Err(f) => fail_reply(&f),
            },
            Err((s, m)) => error(s, m),
        },
        Id::ConnectionAdd => match read_json::<Value>(body).await {
            Ok(v) => ext(core, ExtOp::McpAdd(v)).await,
            Err((s, m)) => error(s, m),
        },
        Id::ConnectionDelete => ext(core, ExtOp::McpDelete(p.to_owned())).await,
        Id::ConnectionEnable => match enabled_flag(body).await {
            Ok(on) => ext(core, ExtOp::McpEnable(p.to_owned(), on)).await,
            Err((s, m)) => error(s, m),
        },
        Id::ConnectionAccept => match accept_choice(body).await {
            Ok(tool) => ext(core, ExtOp::McpAccept(p.to_owned(), tool)).await,
            Err((s, m)) => error(s, m),
        },
        Id::SkillAction => skill_action(core, p, body).await,
        Id::SkillAdd => match read_json_limit::<Value>(body, MAX_SKILL_BODY).await {
            Ok(v) => ext(core, ExtOp::SkillAdd(v)).await,
            Err((s, m)) => error(s, m),
        },
        Id::SettingsGet => json_reply(StatusCode::OK, &core.ui_settings()),
        Id::SettingsPut => match read_json_limit::<Value>(body, MAX_TOOL_BODY).await {
            Ok(v) => change_settings(core, &v).await,
            Err((s, m)) => error(s, m),
        },
        Id::SettingsExport => export_settings(core),
        Id::Diagnostics => diagnostics(core),
        Id::Logs => logs(query),
        Id::Audit => audit(core, query),
        Id::NotesList => match query_param(query, "q") {
            Some(q) => notes_search(core, &q),
            None => notes_list(core),
        },
        Id::NoteAdd => notes_add(core, body).await,
        Id::NotesForgetAll => notes_forget(core, None),
        Id::NoteEdit => notes_edit(core, p, body).await,
        Id::NoteForget => match p.parse::<u64>() {
            Ok(id) => notes_forget(core, Some(id)),
            Err(_) => error(StatusCode::NOT_FOUND, "no such memory"),
        },
        Id::ModelsGet => models(core).await,
        Id::ModelsAsk => models_with_key(core, body).await,
        Id::LlmKeySet => llm_key_set(core, body).await,
        Id::LlmKeyClear => llm_key_clear(core).await,
        Id::SpeechKeySet => speech_key_set(core, body).await,
        Id::SpeechKeyClear => speech_key_clear(core),
        Id::RealtimeKeySet => realtime_key_set(core, body).await,
        Id::RealtimeKeyClear => realtime_key_clear(core),
        Id::Voices => voices(core).await,
        Id::HivePeers => hive_peers(core),
        Id::UpdateStatus => update_status(core),
        Id::UpdateCheck => update_check(core).await,
        Id::UpdateApply => update_apply(core, body).await,
        Id::About => json_reply(
            StatusCode::OK,
            &json!({ "version": env!("CARGO_PKG_VERSION"), "build": crate::diag::build_info(), "changelog": CHANGELOG }),
        ),
        Id::ScheduleList => schedule(core, SchedOp::List).await,
        Id::ScheduleAdd => match read_json::<Value>(body).await {
            Ok(v) => schedule(core, SchedOp::Add(v)).await,
            Err((s, m)) => error(s, m),
        },
        Id::ScheduleEdit => match (p.parse::<u64>(), read_json::<Value>(body).await) {
            (Ok(id), Ok(v)) => schedule(core, SchedOp::Edit(id, v)).await,
            (Err(_), _) => error(StatusCode::NOT_FOUND, "no such timer, alarm, or reminder"),
            (_, Err((s, m))) => error(s, m),
        },
        Id::ScheduleCancel => match p.parse::<u64>() {
            Ok(id) => schedule(core, SchedOp::Cancel(id)).await,
            Err(_) => error(StatusCode::NOT_FOUND, "no such timer, alarm, or reminder"),
        },
    })
}
