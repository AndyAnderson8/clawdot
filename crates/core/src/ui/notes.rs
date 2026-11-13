//! The notes (memories) the user asked the assistant to keep: list, search, add, edit and forget (REQ-CFG-044, 061).

use super::common::{Resp, error, json_reply, read_json};
use crate::app::Core;
use hyper::StatusCode;
use hyper::body::Incoming;
use serde::Deserialize;
use serde_json::{Value, json};

/// `GET /api/notes`: the notes the user asked the assistant to keep (REQ-AST-023, REQ-CFG-044).
pub(super) fn notes_list(core: &Core) -> Resp {
    let Some(store) = core.notes() else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "Memory is not available.");
    };
    json_reply(
        StatusCode::OK,
        &json!({
            "enabled": store.enabled(),
            "notes": store.list(),
            "limits": {
                "max_notes": crate::notes::MAX_NOTES,
                "max_note_chars": crate::notes::MAX_NOTE_CHARS,
                "max_total_bytes": crate::notes::MAX_TOTAL_BYTES,
            },
            "note": "Memories are written only when the user asks, by voice or in the portal. Show their text as text.",
        }),
    )
}

/// `GET /api/notes?q=words`: the notes that match, best first (REQ-MEM-004). The same ranking the assistant's
/// `recall` tool uses. Each note carries its `position`, the number voice commands use ("forget note 7").
pub(super) fn notes_search(core: &Core, q: &str) -> Resp {
    let Some(store) = core.notes() else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "Memory is not available.");
    };
    let found: Vec<Value> = store
        .search(q, crate::notes::PORTAL_SEARCH_MAX)
        .into_iter()
        .map(
            |(position, n)| json!({ "id": n.id, "text": n.text, "ts": n.ts, "position": position }),
        )
        .collect();
    json_reply(
        StatusCode::OK,
        &json!({
            "enabled": store.enabled(),
            "query": q.chars().take(crate::notes::MAX_QUERY_CHARS).collect::<String>(),
            "notes": found,
            "limits": {
                "max_notes": crate::notes::MAX_NOTES,
                "max_note_chars": crate::notes::MAX_NOTE_CHARS,
                "max_total_bytes": crate::notes::MAX_TOTAL_BYTES,
            },
            "note": "Best matches first. Memories are the user's own words; show their text as text.",
        }),
    )
}

/// `POST /api/notes` with `{"text":..}`: the administrator writes a note. It goes through the same store checks
/// as a spoken "remember that" (REQ-CFG-061), and the answer is the new list.
pub(super) async fn notes_add(core: &Core, body: Incoming) -> Resp {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct NewNote {
        text: String,
    }
    let Some(store) = core.notes() else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "Memory is not available.");
    };
    let req = match read_json::<NewNote>(body).await {
        Ok(r) => r,
        Err((s, _)) => return error(s, "expected {\"text\":..} with the note to keep"),
    };
    match store.add(&req.text, core.now()) {
        Ok(_) => notes_list(core),
        Err(e) => {
            use crate::notes::NoteError::{Duplicate, Full, Off, Storage};
            let status = match e {
                Off | Duplicate | Full => StatusCode::CONFLICT,
                Storage => StatusCode::INTERNAL_SERVER_ERROR,
                _ => StatusCode::BAD_REQUEST,
            };
            error(status, &e.plain())
        }
    }
}

/// `PUT /api/notes/{id}` with `{"text":..}`: change what one memory says. The answer is the new list.
pub(super) async fn notes_edit(core: &Core, id: &str, body: Incoming) -> Resp {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Change {
        text: String,
    }
    let Ok(id) = id.parse::<u64>() else {
        return error(StatusCode::NOT_FOUND, "no such memory");
    };
    let Some(store) = core.notes() else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "Memory is not available.");
    };
    let req = match read_json::<Change>(body).await {
        Ok(r) => r,
        Err((s, _)) => return error(s, "expected {\"text\":..} with what to remember"),
    };
    match store.edit(id, &req.text) {
        Ok(_) => notes_list(core),
        Err(e) => {
            use crate::notes::NoteError::{Duplicate, Full, Missing, Off, Storage};
            let status = match e {
                Missing => StatusCode::NOT_FOUND,
                Off | Duplicate | Full => StatusCode::CONFLICT,
                Storage => StatusCode::INTERNAL_SERVER_ERROR,
                _ => StatusCode::BAD_REQUEST,
            };
            error(status, &e.plain())
        }
    }
}

/// `DELETE /api/notes/{id}` forgets one note, `DELETE /api/notes` forgets all of them.
pub(super) fn notes_forget(core: &Core, id: Option<u64>) -> Resp {
    let Some(store) = core.notes() else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "Memory is not available.");
    };
    let done = match id {
        Some(id) => store.forget(id).map(|()| 1),
        None => store.clear(),
    };
    match done {
        Ok(n) => json_reply(StatusCode::OK, &json!({ "forgotten": n })),
        Err(crate::notes::NoteError::Missing) => error(StatusCode::NOT_FOUND, "no such memory"),
        Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, &e.plain()),
    }
}
