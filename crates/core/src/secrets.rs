//! Keys a person pastes into the portal. They are kept in one private file in the data directory (mode 0600 on Unix, in a
//! directory only the service user can open), never sent back whole, never logged, and never put in a settings export.
//! The page only ever sees a masked hint such as `sk-or-…c9f2`, so it can say which key is in use.
//!
//! This is the same protection the rest of ClawDot's data files have; it is not encryption at rest. Anyone who can read
//! the data directory as the service user can read the key, as they could read an environment file.

use crate::store::{JsonStore, StoreError};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

/// The schema version of `secrets.json`.
pub const SCHEMA: u32 = 1;

/// The name the language-model key is kept under.
pub const LLM_KEY: &str = "llm_key";

/// The name the realtime voice key is kept under.
pub const REALTIME_KEY: &str = "realtime_key";
/// The name the speech-service key is kept under (REQ-VOI-023).
pub const SPEECH_KEY: &str = "speech_key";

/// The longest key accepted.
const MAX_KEY: usize = 512;

/// The stored keys. A cheap handle: clones read and write the same file, and each [`crate::app::Core`] owns its own, so
/// two cores in one process (two data directories) never see each other's keys.
#[derive(Debug, Clone, Default)]
pub struct Secrets {
    /// Where the file lives; `None` keeps nothing (an in-memory core).
    store: Option<JsonStore>,
    /// Held from reading the file to writing it back, so two changes at once cannot lose one of them (the portal
    /// handlers run side by side, and the save goes through one temporary file).
    writing: Arc<Mutex<()>>,
}

impl Secrets {
    /// Keys kept in `secrets.json` under `dir`.
    #[must_use]
    pub fn in_dir(dir: &Path) -> Self {
        Self::at(JsonStore::new(dir.join("secrets.json"), SCHEMA))
    }

    /// Keys kept in this store.
    #[must_use]
    pub fn at(store: JsonStore) -> Self {
        Self {
            store: Some(store),
            writing: Arc::default(),
        }
    }

    /// Nowhere to keep a key: nothing is ever found and nothing can be stored.
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    fn load(&self) -> BTreeMap<String, String> {
        self.store
            .as_ref()
            .and_then(|s| s.load::<BTreeMap<String, String>>().ok())
            .unwrap_or_default()
    }

    /// The stored secret called `name`, if any.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<String> {
        self.load().remove(name).filter(|v| !v.is_empty())
    }

    /// Store a secret, replacing any earlier one of that name.
    ///
    /// # Errors
    /// A plain reason when the value cannot be a key or the file cannot be written.
    pub fn set(&self, name: &str, value: &str) -> Result<(), String> {
        let value = value.trim();
        if value.is_empty() {
            return Err("Paste the key first.".into());
        }
        if value.chars().count() > MAX_KEY
            || value.chars().any(|c| c.is_whitespace() || c.is_control())
        {
            return Err("A key is one line of letters, digits and dashes, with no spaces.".into());
        }
        let Some(store) = self.store.as_ref() else {
            return Err("This ClawDot has nowhere to keep a key.".into());
        };
        let _writing = self.writing.lock().unwrap_or_else(PoisonError::into_inner);
        let mut all = Self::load_for_update(store)?;
        all.insert(name.to_owned(), value.to_owned());
        store
            .save(&all)
            .map_err(|e| format!("The key could not be saved: {e}"))
    }

    /// The stored keys, to be changed and written back. A file that is damaged is replaced (there is nothing in it
    /// to keep), but one that cannot be read right now, or that a newer version wrote, is left alone: writing it
    /// back with one key would lose the others.
    fn load_for_update(store: &JsonStore) -> Result<BTreeMap<String, String>, String> {
        match store.load::<BTreeMap<String, String>>() {
            Ok(all) => Ok(all),
            Err(StoreError::Corrupt(_)) => Ok(BTreeMap::new()),
            Err(e) => Err(format!(
                "The stored keys could not be read, so none was changed: {e}"
            )),
        }
    }

    /// Forget a stored secret.
    ///
    /// # Errors
    /// When the file cannot be written.
    pub fn clear(&self, name: &str) -> Result<(), String> {
        let Some(store) = self.store.as_ref() else {
            return Ok(());
        };
        let _writing = self.writing.lock().unwrap_or_else(PoisonError::into_inner);
        let mut all = Self::load_for_update(store)?;
        if all.remove(name).is_some() {
            store
                .save(&all)
                .map_err(|e| format!("The key could not be removed: {e}"))?;
        }
        Ok(())
    }

    /// The masked hint of the stored secret called `name`, if there is one.
    #[must_use]
    pub fn hint(&self, name: &str) -> Option<String> {
        self.get(name).map(|k| mask(&k))
    }
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
        let s = Secrets::in_dir(&dir);
        assert!(s.get(LLM_KEY).is_none());
        assert!(s.set(LLM_KEY, "  ").is_err(), "an empty key is refused");
        assert!(s.set(LLM_KEY, "has space").is_err());
        s.set(LLM_KEY, "sk-first-0123456789").unwrap();
        assert_eq!(s.get(LLM_KEY).as_deref(), Some("sk-first-0123456789"));
        s.set(LLM_KEY, "sk-second-9876543210").unwrap();
        assert_eq!(
            s.get(LLM_KEY).as_deref(),
            Some("sk-second-9876543210"),
            "replaced"
        );
        assert_eq!(s.hint(LLM_KEY).as_deref(), Some("sk-sec…3210"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join("secrets.json"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o077, 0, "only the owner can read the file");
        }
        s.clear(LLM_KEY).unwrap();
        assert!(s.get(LLM_KEY).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_that_cannot_be_read_is_never_overwritten_with_one_key() {
        let dir = std::env::temp_dir().join(format!("clawdot-secrets-keep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // Written by a newer build: this one cannot read it, so it must not rewrite it and lose the other keys.
        let newer = JsonStore::new(dir.join("secrets.json"), SCHEMA + 1);
        let mut kept = BTreeMap::new();
        kept.insert(LLM_KEY.to_owned(), "sk-kept-0123456789".to_owned());
        newer.save(&kept).unwrap();
        let before = std::fs::read(dir.join("secrets.json")).unwrap();
        let s = Secrets::in_dir(&dir);
        assert!(s.set(SPEECH_KEY, "sk-other-0123456789").is_err());
        assert!(s.clear(LLM_KEY).is_err());
        assert_eq!(std::fs::read(dir.join("secrets.json")).unwrap(), before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn keys_set_at_the_same_moment_are_all_kept() {
        let dir = std::env::temp_dir().join(format!("clawdot-secrets-race-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let s = Secrets::in_dir(&dir);
        let names: Vec<String> = (0..12).map(|i| format!("key_{i}")).collect();
        std::thread::scope(|scope| {
            for name in &names {
                let s = s.clone();
                scope.spawn(move || s.set(name, "sk-value-0123456789").unwrap());
            }
        });
        for name in &names {
            assert!(
                s.get(name).is_some(),
                "{name} was lost to a concurrent save"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn two_stores_in_one_process_do_not_see_each_other() {
        let base = std::env::temp_dir().join(format!("clawdot-secrets-two-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (a, b) = (
            Secrets::in_dir(&base.join("a")),
            Secrets::in_dir(&base.join("b")),
        );
        a.set(LLM_KEY, "sk-only-in-a-0123").unwrap();
        assert_eq!(a.get(LLM_KEY).as_deref(), Some("sk-only-in-a-0123"));
        assert!(b.get(LLM_KEY).is_none(), "b must not see a's key");
        assert!(Secrets::none().get(LLM_KEY).is_none());
        let _ = std::fs::remove_dir_all(&base);
    }
}
