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
use crate::app::{Core, Op, SchedOp};
use crate::extensions::{self, ExtOp};
use crate::http::{BODY_TIMEOUT, read_body};
use hyper::body::Incoming;
use hyper::header::HeaderMap;
use hyper::{Method, StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};
use std::net::IpAddr;
use std::sync::Arc;

use self::auth::{admin_error, change_password, login, recover_window, setup};
use self::common::{
    KeyBody, MAX_BODY, MAX_SKILL_BODY, MAX_TOOL_BODY, Resp, attachment, bearer, error, fail_reply,
    json_reply, query_param, read_json, read_json_limit, saved,
};
use self::endpoints::{EndpointUpdate, create, endpoint_error, endpoint_json};
use self::notes::{notes_add, notes_edit, notes_forget, notes_list, notes_search};
use self::page::page;
use self::status::{audit, diagnostics, hive_peers, logs, overview};
use self::tools::{accept_choice, enabled_flag, ext, skill_action, tier_choice};

mod auth;
mod common;
mod endpoints;
mod notes;
mod page;
mod status;
mod tools;

#[cfg(test)]
mod openai_host_tests;
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

/// `GET /api/models`: the models the language-model service offers, asked of the service with the SAVED address and
/// key (REQ-CFG-081). A service with no list, or a wrong key, is said in plain words and the page falls back to typing.
///
/// It takes nothing from the web address. It once took `?url=` and `?key_env=`, which let any web page shown in a
/// browser on the home network (in open mode, with a plain cross-site GET such as an image) make the device send the
/// value of an environment variable to an address of the page's choosing. A choice that is not saved yet is asked
/// about with `POST /api/models`, which a page on another site cannot send.
async fn models(core: &Core) -> Resp {
    let s = core.ui_settings();
    let value = |k: &str| s["fields"][k]["value"].as_str().unwrap_or("").to_owned();
    let url = value("llm_url");
    if url.is_empty() {
        return error(
            StatusCode::CONFLICT,
            "Choose who answers and save first, then the list of models can be asked for.",
        );
    }
    fetch_models(&url, &saved_llm_key(core)).await
}

/// The key in use for the language-model service: the one pasted into the portal, or else the value of the
/// environment variable the saved settings name.
fn saved_llm_key(core: &Core) -> String {
    core.secrets()
        .get(crate::secrets::LLM_KEY)
        .unwrap_or_else(|| {
            let s = core.ui_settings();
            let var = s["fields"]["llm_key_env"]["value"].as_str().unwrap_or("");
            core.env().get(var).unwrap_or_default()
        })
}

/// Ask a language-model service for its models with `key`, and say in plain words why it could not.
async fn fetch_models(url: &str, key: &str) -> Resp {
    match crate::transport::list_models(url, key).await {
        Ok(models) => json_reply(StatusCode::OK, &json!({ "models": models })),
        Err(e) if e.is_unauthorized() => error(
            StatusCode::UNAUTHORIZED,
            "The service did not accept that key. Check that it is the whole key and that it is for this service.",
        ),
        Err(e) => error(
            StatusCode::BAD_GATEWAY,
            &format!("Could not get the list of models: {e}."),
        ),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelsAsk {
    url: String,
    /// A key to try. Left out, the stored key (or the one in the named environment variable) is used.
    #[serde(default)]
    key: Option<String>,
}

/// `POST /api/models`: the same list, asking with a key typed on the page, so the key is checked before it is kept and
/// never travels in a web address.
async fn models_with_key(core: &Core, body: Incoming) -> Resp {
    let ask = match read_json::<ModelsAsk>(body).await {
        Ok(a) => a,
        Err((s, m)) => return error(s, m),
    };
    let url = ask.url.trim();
    if url.is_empty() {
        return error(StatusCode::CONFLICT, "Choose a provider first.");
    }
    let key = match ask.key.as_deref().map(str::trim).filter(|k| !k.is_empty()) {
        Some(k) => k.to_owned(),
        None => saved_llm_key(core),
    };
    fetch_models(url, &key).await
}

/// `PUT /api/llm/key`: keep the key pasted on the page and use it from now on.
async fn llm_key_set(core: &Core, body: Incoming) -> Resp {
    let k = match read_json::<KeyBody>(body).await {
        Ok(k) => k,
        Err((s, m)) => return error(s, m),
    };
    if let Err(m) = core.secrets().set(crate::secrets::LLM_KEY, &k.key) {
        return error(StatusCode::BAD_REQUEST, &m);
    }
    rekeyed(core, "saved").await
}

/// `DELETE /api/llm/key`: forget the stored key.
async fn llm_key_clear(core: &Core) -> Resp {
    if let Err(m) = core.secrets().clear(crate::secrets::LLM_KEY) {
        return error(StatusCode::INTERNAL_SERVER_ERROR, &m);
    }
    rekeyed(core, "removed").await
}

/// Tell the running assistant about the changed key and answer with what the page may know of it: the masked hint.
/// `done` is "saved" or "removed". When the main loop cannot take the request, the change is on disk but the running
/// model client still has the old key, so the answer is a 503 that says so (the page shows an error's message as
/// written); the hint is included so it can still be read.
async fn rekeyed(core: &Core, done: &str) -> Resp {
    let hint = core.secrets().hint(crate::secrets::LLM_KEY);
    match core.portal_call(Op::Rekey).await {
        Ok(_) => json_reply(StatusCode::OK, &json!({ "hint": hint })),
        Err(_) => json_reply(
            StatusCode::SERVICE_UNAVAILABLE,
            &json!({
                "error": format!(
                    "The key is {done}, but the assistant could not switch to it just now. It will be used after a restart, or when the assistant is free."
                ),
                "hint": hint,
            }),
        ),
    }
}

/// `PUT /api/speech/key`: keep the speech-service key pasted on the page. It is read on every request, so it applies at
/// once (REQ-VOI-023). Only the masked hint comes back.
async fn speech_key_set(core: &Core, body: Incoming) -> Resp {
    let k = match read_json::<KeyBody>(body).await {
        Ok(k) => k,
        Err((s, m)) => return error(s, m),
    };
    if let Err(m) = core.secrets().set(crate::secrets::SPEECH_KEY, &k.key) {
        return error(StatusCode::BAD_REQUEST, &m);
    }
    json_reply(
        StatusCode::OK,
        &json!({ "hint": core.secrets().hint(crate::secrets::SPEECH_KEY) }),
    )
}

/// `DELETE /api/speech/key`: forget the stored speech key (an environment variable, if named, applies again).
fn speech_key_clear(core: &Core) -> Resp {
    if let Err(m) = core.secrets().clear(crate::secrets::SPEECH_KEY) {
        return error(StatusCode::INTERNAL_SERVER_ERROR, &m);
    }
    json_reply(StatusCode::OK, &json!({ "hint": null }))
}

/// `PUT /api/realtime/key`: keep the key for the realtime voice service. It is read at the start of each session, so
/// it applies to the next one without a restart, and it wins over the environment variable.
async fn realtime_key_set(core: &Core, body: Incoming) -> Resp {
    let k = match read_json::<KeyBody>(body).await {
        Ok(k) => k,
        Err((s, m)) => return error(s, m),
    };
    if let Err(m) = core.secrets().set(crate::secrets::REALTIME_KEY, &k.key) {
        return error(StatusCode::BAD_REQUEST, &m);
    }
    json_reply(
        StatusCode::OK,
        &json!({ "hint": core.secrets().hint(crate::secrets::REALTIME_KEY) }),
    )
}

/// `DELETE /api/realtime/key`: forget the stored realtime key.
fn realtime_key_clear(core: &Core) -> Resp {
    if let Err(m) = core.secrets().clear(crate::secrets::REALTIME_KEY) {
        return error(StatusCode::INTERNAL_SERVER_ERROR, &m);
    }
    json_reply(StatusCode::OK, &json!({ "hint": null }))
}

/// The voices of OpenAI's speech service, which has a fixed set and no list to ask for.
const OPENAI_VOICES: [&str; 11] = [
    "alloy", "ash", "ballad", "coral", "echo", "fable", "nova", "onyx", "sage", "shimmer", "verse",
];

/// Whether `url` is OpenAI's own service. The host is compared, not the text: a server whose path or query merely
/// mentions `api.openai.com`, or whose name only starts with it, is not OpenAI.
fn is_openai(url: &str) -> bool {
    crate::transport::Endpoint::parse(url)
        .is_ok_and(|e| e.host.eq_ignore_ascii_case("api.openai.com"))
}

/// `GET /api/voices`: the voices the speech server offers (REQ-CFG-081): its own list when it has one, OpenAI's fixed
/// set for OpenAI, otherwise a plain statement that the voice has to be typed.
async fn voices(core: &Core) -> Resp {
    let Some(src) = core.speech_source() else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "No speech provider is set up, so there are no voices to choose from.",
        );
    };
    let (url, dialect) = (src.url.as_str(), src.dialect);
    match crate::speech::list_voice_details(dialect, url, &src.key).await {
        Ok((v, source)) => {
            // `voices` stays the plain list of values to send; `details` adds the name to show and the category, so a
            // cloned voice can be shown by its name while its id is sent (REQ-VOI-022).
            let ids: Vec<&str> = v.iter().map(|x| x.id.as_str()).collect();
            let details: Vec<Value> = v
                .iter()
                .map(|x| json!({ "id": x.id, "label": x.label, "category": x.category }))
                .collect();
            json_reply(
                StatusCode::OK,
                &json!({ "voices": ids, "details": details, "source": source }),
            )
        }
        Err(_) if is_openai(url) => json_reply(
            StatusCode::OK,
            &json!({
                "voices": OPENAI_VOICES,
                "details": OPENAI_VOICES.iter().map(|n| json!({ "id": n, "label": n, "category": null })).collect::<Vec<_>>(),
                "source": "builtin",
            }),
        ),
        Err(e) if dialect != crate::speech::Dialect::OpenAi => {
            let status = if e.is_key_problem() {
                StatusCode::UNAUTHORIZED
            } else {
                StatusCode::BAD_GATEWAY
            };
            error(status, &format!("Could not get the list of voices: {e}."))
        }
        Err(e) => error(
            StatusCode::BAD_GATEWAY,
            &format!(
                "This speech server does not share a list of its voices ({e}). Type the name of one it accepts; its documentation lists them."
            ),
        ),
    }
}

/// `GET /api/update`: whether a newer version exists, when it was last looked for, and whether it can be installed
/// here (REQ-UPD-005).
#[cfg(feature = "tls")]
fn update_status(core: &Core) -> Resp {
    match core.updater() {
        Some(u) => json_reply(StatusCode::OK, &u.status()),
        None => error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Updates are not running on this device.",
        ),
    }
}

#[cfg(not(feature = "tls"))]
fn update_status(_core: &Core) -> Resp {
    error(
        StatusCode::SERVICE_UNAVAILABLE,
        "This build does not check for updates.",
    )
}

/// `POST /api/update/check`: look for a newer version now. The answer is the same document as `GET /api/update`;
/// a failed look is in its `error` field, not an HTTP error.
#[cfg(feature = "tls")]
async fn update_check(core: &Core) -> Resp {
    let Some(u) = core.updater() else {
        return update_status(core);
    };
    let _ = u.check().await;
    json_reply(StatusCode::OK, &u.status())
}

#[cfg(not(feature = "tls"))]
async fn update_check(core: &Core) -> Resp {
    update_status(core)
}

/// `POST /api/update/apply` with `{"version": "1.2.3"}`: install the version the person confirmed (REQ-UPD-006).
/// It starts at once and the page watches `GET /api/update`.
#[cfg(feature = "tls")]
async fn update_apply(core: &Core, body: Incoming) -> Resp {
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
async fn update_apply(core: &Core, _body: Incoming) -> Resp {
    update_status(core)
}

/// `GET /api/settings/export`: the saved settings as a file the page offers for download (REQ-CFG-065).
fn export_settings(core: &Core) -> Resp {
    let v = core.export_settings();
    if v.is_null() {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Settings are not available yet.",
        );
    }
    attachment(json_reply(StatusCode::OK, &v), "clawdot-settings.json")
}

/// Timers, alarms, and reminders: the main loop does the work, so both the voice path and the portal use
/// one scheduler (REQ-CFG-027).
async fn schedule(core: &Core, op: SchedOp) -> Resp {
    match core.portal_call(Op::Sched(op)).await {
        Ok(v) => json_reply(StatusCode::OK, &v),
        Err(f) => fail_reply(&f),
    }
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Speak {
    text: String,
    /// A voice to try for this phrase, so it can be heard before it is saved.
    #[serde(default)]
    voice: Option<String>,
}

/// "Test speak": queue a phrase for the speaker to say.
async fn speak(core: &Core, body: Incoming) -> Resp {
    let Some(bytes) = read_body(body, MAX_BODY, BODY_TIMEOUT).await else {
        return error(StatusCode::PAYLOAD_TOO_LARGE, "body too large or too slow");
    };
    let Ok(req) = serde_json::from_slice::<Speak>(&bytes) else {
        return error(StatusCode::BAD_REQUEST, "expected {\"text\":..}");
    };
    let text = req.text.trim();
    if text.is_empty() || text.chars().count() > crate::app::MAX_SPEECH_CHARS {
        return error(
            StatusCode::BAD_REQUEST,
            "give some text of up to 200 characters",
        );
    }
    if let Some((status, reason, message)) = speak_refusal(&core.ui_status()) {
        return speak_refused(status, reason, message);
    }
    let voice = req
        .voice
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty());
    if voice.is_some_and(|v| v.len() > 128 || v.chars().any(char::is_control)) {
        return error(StatusCode::BAD_REQUEST, "that voice name is not valid");
    }
    if core.queue_speech_as(text, voice) {
        json_reply(StatusCode::ACCEPTED, &json!({ "ok": true, "queued": true }))
    } else {
        speak_refused(
            StatusCode::CONFLICT,
            "busy",
            "The speaker is still saying the last phrase. Wait for it to finish, then try again.",
        )
    }
}

fn speak_refused(status: StatusCode, reason: &str, message: &str) -> Resp {
    json_reply(
        status,
        &json!({ "ok": false, "reason": reason, "message": message, "error": message }),
    )
}

/// Why a test phrase cannot be spoken right now, from the latest status snapshot (REQ-CFG-070). A snapshot that
/// does not say (not published yet, or an older shape) is treated as unknown and nothing is refused.
fn speak_refusal(status: &Value) -> Option<(StatusCode, &'static str, &'static str)> {
    let flag = |a: &str, b: &str| status.get(a)?.get(b)?.as_bool();
    // Speech printed to the terminal needs neither a satellite nor a voice.
    if status
        .get("speaker")
        .and_then(|s| s.get("name"))
        .and_then(Value::as_str)
        == Some("console")
    {
        return None;
    }
    if flag("voice", "configured") == Some(false) {
        return Some((
            StatusCode::SERVICE_UNAVAILABLE,
            "no_voice",
            "No voice is set up, so the speaker has nothing to speak with. Add a speech provider in the configuration file (see the voice pipeline guide), then restart ClawDot.",
        ));
    }
    if flag("speaker", "connected") == Some(false) {
        return Some((
            StatusCode::CONFLICT,
            "no_speaker",
            "No speaker is connected. Start the speaker program on the device or a computer, check that it points at this address, then try again.",
        ));
    }
    if status.get("muted").and_then(Value::as_bool) == Some(true) {
        return Some((
            StatusCode::CONFLICT,
            "muted",
            "The speaker is muted. Press its mute button to unmute it, then try again.",
        ));
    }
    if flag("speaker", "available") == Some(false) {
        return Some((
            StatusCode::CONFLICT,
            "busy",
            "The speaker is in a conversation or muted. Wait a moment, or unmute it, then try again.",
        ));
    }
    None
}
