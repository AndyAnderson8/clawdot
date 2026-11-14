//! Speech in the portal: the speech and realtime keys, the voice list, and the test phrase (REQ-VOI-022, 023, REQ-CFG-070, 081).

use super::common::{KeyBody, MAX_BODY, Resp, error, json_or_reply, json_reply};
use crate::app::Core;
use crate::http::{BODY_TIMEOUT, read_body};
use hyper::StatusCode;
use hyper::body::Incoming;
use serde::Deserialize;
use serde_json::{Value, json};

/// `PUT /api/speech/key`: keep the speech-service key pasted on the page. It is read on every request, so it applies at
/// once (REQ-VOI-023). Only the masked hint comes back.
pub(super) async fn speech_key_set(core: &Core, body: Incoming) -> Resp {
    let k = match json_or_reply::<KeyBody>(body).await {
        Ok(k) => k,
        Err(r) => return r,
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
pub(super) fn speech_key_clear(core: &Core) -> Resp {
    if let Err(m) = core.secrets().clear(crate::secrets::SPEECH_KEY) {
        return error(StatusCode::INTERNAL_SERVER_ERROR, &m);
    }
    json_reply(StatusCode::OK, &json!({ "hint": null }))
}

/// `PUT /api/realtime/key`: keep the key for the realtime voice service. It is read at the start of each session, so
/// it applies to the next one without a restart, and it wins over the environment variable.
pub(super) async fn realtime_key_set(core: &Core, body: Incoming) -> Resp {
    let k = match json_or_reply::<KeyBody>(body).await {
        Ok(k) => k,
        Err(r) => return r,
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
pub(super) fn realtime_key_clear(core: &Core) -> Resp {
    if let Err(m) = core.secrets().clear(crate::secrets::REALTIME_KEY) {
        return error(StatusCode::INTERNAL_SERVER_ERROR, &m);
    }
    json_reply(StatusCode::OK, &json!({ "hint": null }))
}

/// `GET /api/voices`: the voices the speech server offers (REQ-CFG-081): its own list when it has one, OpenAI's fixed
/// set for OpenAI, otherwise a plain statement that the voice has to be typed.
pub(super) async fn voices(core: &Core) -> Resp {
    let Some(src) = core.speech_source() else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "No speech provider is set up, so there are no voices to choose from.",
        );
    };
    let (url, dialect) = (src.url.as_str(), src.dialect);
    // `voices` stays the plain list of values to send; `details` adds the name to show and the category, so a
    // cloned voice can be shown by its name while its id is sent (REQ-VOI-022).
    let listed = |v: &[crate::speech::VoiceInfo], source: &str| {
        let ids: Vec<&str> = v.iter().map(|x| x.id.as_str()).collect();
        let details: Vec<Value> = v
            .iter()
            .map(|x| json!({ "id": x.id, "label": x.label, "category": x.category }))
            .collect();
        json_reply(
            StatusCode::OK,
            &json!({ "voices": ids, "details": details, "source": source }),
        )
    };
    match crate::speech::list_voice_details(dialect, url, &src.key).await {
        Ok((v, source)) => listed(&v, source),
        Err(e) => match dialect.provider().builtin_voices(url) {
            Some(v) => listed(&v, "builtin"),
            None => voices_error(dialect, &e),
        },
    }
}

/// Why the voice list could not be had, in words for the person who has to type a voice instead.
fn voices_error(dialect: crate::speech::Dialect, e: &crate::speech::ProviderError) -> Resp {
    if dialect != crate::speech::Dialect::OpenAi {
        let status = if e.is_key_problem() {
            StatusCode::UNAUTHORIZED
        } else {
            StatusCode::BAD_GATEWAY
        };
        return error(status, &format!("Could not get the list of voices: {e}."));
    }
    error(
        StatusCode::BAD_GATEWAY,
        &format!(
            "This speech server does not share a list of its voices ({e}). Type the name of one it accepts; its documentation lists them."
        ),
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Speak {
    text: String,
    /// A voice to try for this phrase, so it can be heard before it is saved.
    #[serde(default)]
    voice: Option<String>,
}

/// "Test speak": queue a phrase for the speaker to say.
pub(super) async fn speak(core: &Core, body: Incoming) -> Resp {
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
