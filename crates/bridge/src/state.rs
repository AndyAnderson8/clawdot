//! Remembers which replies were already handled, so redelivery (at-least-once) never runs an
//! instruction twice.
//!
//! A reply is known by its number and the time the speaker queued it. The number alone is not enough: a
//! speaker whose data was reset counts from one again, and its new replies must not be mistaken for old
//! ones and dropped.

use clawdot_protocol::bridge::Reply;
use std::collections::VecDeque;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// How many handled ids are remembered.
const KEEP: usize = 2_000;

/// What identifies one reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Key {
    /// The reply's number.
    pub id: u64,
    /// When the speaker queued it (unix seconds). `None` for an entry saved before the time was kept; such
    /// an entry stands for every reply with its number.
    pub queued_at: Option<i64>,
}

impl Key {
    /// The key of `reply`.
    #[must_use]
    pub fn of(reply: &Reply) -> Self {
        Self {
            id: reply.id,
            queued_at: Some(reply.created_at),
        }
    }

    /// One line of the state file: `ID QUEUED_AT`, or `ID` alone in a file from an older bridge.
    fn parse(line: &str) -> Option<Self> {
        let mut parts = line.split_whitespace();
        let id = parts.next()?.parse().ok()?;
        let queued_at = match parts.next() {
            Some(t) => Some(t.parse().ok()?),
            None => None,
        };
        parts.next().is_none().then_some(Self { id, queued_at })
    }

    fn line(self) -> String {
        match self.queued_at {
            Some(t) => format!("{} {t}\n", self.id),
            None => format!("{}\n", self.id),
        }
    }

    /// Whether this saved entry is the reply `other`.
    fn covers(self, other: Self) -> bool {
        self.id == other.id && (self.queued_at.is_none() || self.queued_at == other.queued_at)
    }
}

/// A small persisted set of handled replies.
#[derive(Debug)]
pub struct Handled {
    path: Option<PathBuf>,
    ids: VecDeque<Key>,
}

impl Handled {
    /// In-memory only.
    #[must_use]
    pub fn in_memory() -> Self {
        Self {
            path: None,
            ids: VecDeque::new(),
        }
    }

    /// Load from `path`, starting empty if it does not exist.
    ///
    /// # Errors
    /// I/O errors other than "not found"; a corrupt file is treated as empty (worst case: one
    /// instruction could repeat after a crash).
    pub fn load(path: &Path) -> io::Result<Self> {
        let ids = match fs::read_to_string(path) {
            Ok(s) => s.lines().filter_map(Key::parse).collect(),
            Err(e) if e.kind() == io::ErrorKind::NotFound => VecDeque::new(),
            Err(e) => return Err(e),
        };
        Ok(Self {
            path: Some(path.to_owned()),
            ids,
        })
    }

    /// Whether the reply `key` was handled before.
    #[must_use]
    pub fn contains(&self, key: Key) -> bool {
        self.ids.iter().any(|k| k.covers(key))
    }

    /// Record the reply `key` as handled and persist.
    ///
    /// # Errors
    /// I/O errors writing the state file.
    pub fn insert(&mut self, key: Key) -> io::Result<()> {
        if self.contains(key) {
            return Ok(());
        }
        self.ids.push_back(key);
        while self.ids.len() > KEEP {
            self.ids.pop_front();
        }
        self.persist()
    }

    /// Forget the reply `key` (the command never started, so the reply must be retried).
    ///
    /// # Errors
    /// I/O errors writing the state file.
    pub fn remove(&mut self, key: Key) -> io::Result<()> {
        self.ids.retain(|k| !k.covers(key));
        self.persist()
    }

    fn persist(&self) -> io::Result<()> {
        if let Some(path) = &self.path {
            let body: String = self.ids.iter().map(|k| k.line()).collect();
            let tmp = path.with_extension("tmp");
            fs::write(&tmp, body)?;
            fs::rename(&tmp, path)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(id: u64) -> Key {
        Key {
            id,
            queued_at: Some(1_800_000_000),
        }
    }

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("clawdot-bridge-{name}-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir.join("handled")
    }

    #[test]
    fn remembers_across_restarts() {
        let path = temp("persist");
        let mut h = Handled::load(&path).unwrap();
        assert!(!h.contains(key(5)));
        h.insert(key(5)).unwrap();
        h.insert(key(5)).unwrap();
        let again = Handled::load(&path).unwrap();
        assert!(again.contains(key(5)) && !again.contains(key(6)));
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn the_set_is_bounded() {
        let mut h = Handled::in_memory();
        for i in 0..(KEEP as u64 + 50) {
            h.insert(key(i)).unwrap();
        }
        assert!(!h.contains(key(0)) && h.contains(key(KEEP as u64 + 49)));
        assert!(h.ids.len() <= KEEP);
    }

    #[test]
    fn a_corrupt_file_is_treated_as_empty() {
        let path = temp("corrupt");
        fs::write(&path, "not numbers\n7\n???\n").unwrap();
        let h = Handled::load(&path).unwrap();
        assert!(h.contains(key(7)));
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_speaker_that_counts_from_one_again_is_not_mistaken_for_its_old_replies() {
        let path = temp("reset");
        let mut h = Handled::load(&path).unwrap();
        h.insert(key(3)).unwrap();
        // The speaker's data was reset: reply 3 again, queued at another time.
        let fresh = Key {
            id: 3,
            queued_at: Some(1_800_009_999),
        };
        assert!(!Handled::load(&path).unwrap().contains(fresh));
        h.insert(fresh).unwrap();
        let again = Handled::load(&path).unwrap();
        assert!(again.contains(fresh) && again.contains(key(3)));
        // Forgetting one leaves the other.
        h.remove(fresh).unwrap();
        assert!(h.contains(key(3)) && !h.contains(fresh));
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn an_entry_from_an_older_bridge_still_stands_for_its_number() {
        let path = temp("legacy");
        fs::write(&path, "7\n8 1800000000\n9 x\n").unwrap();
        let h = Handled::load(&path).unwrap();
        assert!(h.contains(key(7)), "no time saved: any reply 7");
        assert!(h.contains(key(8)));
        assert!(!h.contains(key(9)), "a line that does not parse is skipped");
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }
}
