//! The language-model service: its list of models and its key (REQ-CFG-081).

use super::common::{KeyBody, Resp, error, json_or_reply, json_reply};
use crate::app::{Core, Op};
use hyper::StatusCode;
use hyper::body::Incoming;
use serde::Deserialize;
use serde_json::json;

/// `GET /api/models`: the models the language-model service offers, asked of the service with the SAVED address and
/// key (REQ-CFG-081). A service with no list, or a wrong key, is said in plain words and the page falls back to typing.
///
/// It takes nothing from the web address. It once took `?url=` and `?key_env=`, which let any web page shown in a
/// browser on the home network (in open mode, with a plain cross-site GET such as an image) make the device send the
/// value of an environment variable to an address of the page's choosing. A choice that is not saved yet is asked
/// about with `POST /api/models`, which a page on another site cannot send.
pub(super) async fn models(core: &Core) -> Resp {
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
pub(super) async fn fetch_models(url: &str, key: &str) -> Resp {
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
pub(super) struct ModelsAsk {
    url: String,
    /// A key to try. Left out, the stored key (or the one in the named environment variable) is used.
    #[serde(default)]
    key: Option<String>,
}

/// `POST /api/models`: the same list, asking with a key typed on the page, so the key is checked before it is kept and
/// never travels in a web address.
pub(super) async fn models_with_key(core: &Core, body: Incoming) -> Resp {
    let ask = match json_or_reply::<ModelsAsk>(body).await {
        Ok(a) => a,
        Err(r) => return r,
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
pub(super) async fn llm_key_set(core: &Core, body: Incoming) -> Resp {
    let k = match json_or_reply::<KeyBody>(body).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    if let Err(m) = core.secrets().set(crate::secrets::LLM_KEY, &k.key) {
        return error(StatusCode::BAD_REQUEST, &m);
    }
    rekeyed(core, "saved").await
}

/// `DELETE /api/llm/key`: forget the stored key.
pub(super) async fn llm_key_clear(core: &Core) -> Resp {
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
