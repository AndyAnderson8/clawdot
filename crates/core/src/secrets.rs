//! Keys a person pastes into the portal. They are kept in one private file in the data directory (mode 0600 on Unix, in a
//! directory only the service user can open), never sent back whole, never logged, and never put in a settings export.
//! The page only ever sees a masked hint such as `sk-or-…c9f2`, so it can say which key is in use.
//!
//! This is the same protection the rest of ClawDot's data files have; it is not encryption at rest. Anyone who can read
//! the data directory as the service user can read the key, as they could read an environment file.

use crate::store::JsonStore;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Mutex, PoisonError};

/// The schema version of `secrets.json`.
pub const SCHEMA: u32 = 1;

/// The name the language-model key is kept under.
pub const LLM_KEY: &str = "llm_key";

/// The name the speech-service key is kept under (REQ-VOI-023).
pub const SPEECH_KEY: &str = "speech_key";

/// The longest key accepted.
const MAX_KEY: usize = 512;

static STORE: Mutex<Option<JsonStore>> = Mutex::new(None);

/// Say where the file lives. Called once at start-up; until then there is nothing stored.
pub fn init(dir: &Path) {
    *STORE.lock().unwrap_or_else(PoisonError::into_inner) =
        Some(JsonStore::new(dir.join("secrets.json"), SCHEMA));
}

fn load() -> BTreeMap<String, String> {
    let g = STORE.lock().unwrap_or_else(PoisonError::into_inner);
    g.as_ref()
        .and_then(|s| s.load::<BTreeMap<String, String>>().ok())
        .unwrap_or_default()
}

/// The stored secret called `name`, if any.
#[must_use]
pub fn get(name: &str) -> Option<String> {
    load().remove(name).filter(|v| !v.is_empty())
}

/// Store a secret, replacing any earlier one of that name.
///
/// # Errors
/// A plain reason when the value cannot be a key or the file cannot be written.
pub fn set(name: &str, value: &str) -> Result<(), String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("Paste the key first.".into());
    }
    if value.chars().count() > MAX_KEY || value.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err("A key is one line of letters, digits and dashes, with no spaces.".into());
    }
    let g = STORE.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(store) = g.as_ref() else {
        return Err("This ClawDot has nowhere to keep a key.".into());
    };
    let mut all = store.load::<BTreeMap<String, String>>().unwrap_or_default();
    all.insert(name.to_owned(), value.to_owned());
    store
        .save(&all)
        .map_err(|e| format!("The key could not be saved: {e}"))
}

/// Forget a stored secret.
///
/// # Errors
/// When the file cannot be written.
pub fn clear(name: &str) -> Result<(), String> {
    let g = STORE.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(store) = g.as_ref() else {
        return Ok(());
    };
    let mut all = store.load::<BTreeMap<String, String>>().unwrap_or_default();
    if all.remove(name).is_some() {
        store
            .save(&all)
            .map_err(|e| format!("The key could not be removed: {e}"))?;
    }
    Ok(())
}

/// The first and last few characters of a key, with the middle left out. A short key shows less, so the hint never gives
/// away most of it.
#[must_use]
pub fn mask(key: &str) -> String {
    let c: Vec<char> = key.chars().collect();
    let (head, tail) = match c.len() {
        0..=9 => (0, 2),
        10..=19 => (3, 3),
        _ => (6, 4),
    };
    let start: String = c.iter().take(head).collect();
    let end: String = c.iter().skip(c.len().saturating_sub(tail)).collect();
    format!("{start}…{end}")
}

/// The masked hint of the stored secret called `name`, if there is one.
#[must_use]
pub fn hint(name: &str) -> Option<String> {
    get(name).map(|k| mask(&k))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hint_shows_the_ends_and_never_most_of_the_key() {
        let k = "sk-or-v1-0123456789abcdef0123456789abcdef";
        let m = mask(k);
        assert_eq!(m, "sk-or-…cdef");
        assert!(!m.contains("0123456789"));
        assert_eq!(mask("abcdefghijkl"), "abc…jkl");
        assert_eq!(mask("short"), "…rt", "a short key shows almost nothing");
        assert_eq!(mask(""), "…");
    }

    #[test]
    fn keys_are_stored_privately_replaced_and_forgotten() {
        let dir = std::env::temp_dir().join(format!("clawdot-secrets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        init(&dir);
        assert!(get(LLM_KEY).is_none());
        assert!(set(LLM_KEY, "  ").is_err(), "an empty key is refused");
        assert!(set(LLM_KEY, "has space").is_err());
        set(LLM_KEY, "sk-first-0123456789").unwrap();
        assert_eq!(get(LLM_KEY).as_deref(), Some("sk-first-0123456789"));
        set(LLM_KEY, "sk-second-9876543210").unwrap();
        assert_eq!(
            get(LLM_KEY).as_deref(),
            Some("sk-second-9876543210"),
            "replaced"
        );
        assert_eq!(hint(LLM_KEY).as_deref(), Some("sk-sec…3210"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join("secrets.json"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o077, 0, "only the owner can read the file");
        }
        clear(LLM_KEY).unwrap();
        assert!(get(LLM_KEY).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
