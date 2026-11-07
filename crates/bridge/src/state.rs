//! Remembers which replies were already handled, so redelivery (at-least-once) never runs an
//! instruction twice.

use std::collections::VecDeque;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// How many handled ids are remembered.
const KEEP: usize = 2_000;

/// A small persisted set of handled reply ids.
#[derive(Debug)]
pub struct Handled {
    path: Option<PathBuf>,
    ids: VecDeque<u64>,
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
            Ok(s) => s.lines().filter_map(|l| l.trim().parse().ok()).collect(),
            Err(e) if e.kind() == io::ErrorKind::NotFound => VecDeque::new(),
            Err(e) => return Err(e),
        };
        Ok(Self {
            path: Some(path.to_owned()),
            ids,
        })
    }

    /// Whether `id` was handled before.
    #[must_use]
    pub fn contains(&self, id: u64) -> bool {
        self.ids.contains(&id)
    }

    /// Record `id` as handled and persist.
    ///
    /// # Errors
    /// I/O errors writing the state file.
    pub fn insert(&mut self, id: u64) -> io::Result<()> {
        if self.contains(id) {
            return Ok(());
        }
        self.ids.push_back(id);
        while self.ids.len() > KEEP {
            self.ids.pop_front();
        }
        self.persist()
    }

    /// Forget `id` (the command never started, so the reply must be retried).
    ///
    /// # Errors
    /// I/O errors writing the state file.
    pub fn remove(&mut self, id: u64) -> io::Result<()> {
        self.ids.retain(|i| *i != id);
        self.persist()
    }

    fn persist(&self) -> io::Result<()> {
        if let Some(path) = &self.path {
            let body: String = self.ids.iter().map(|i| format!("{i}\n")).collect();
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
        assert!(!h.contains(5));
        h.insert(5).unwrap();
        h.insert(5).unwrap();
        let again = Handled::load(&path).unwrap();
        assert!(again.contains(5) && !again.contains(6));
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn the_set_is_bounded() {
        let mut h = Handled::in_memory();
        for i in 0..(KEEP as u64 + 50) {
            h.insert(i).unwrap();
        }
        assert!(!h.contains(0) && h.contains(KEEP as u64 + 49));
        assert!(h.ids.len() <= KEEP);
    }

    #[test]
    fn a_corrupt_file_is_treated_as_empty() {
        let path = temp("corrupt");
        fs::write(&path, "not numbers\n7\n???\n").unwrap();
        let h = Handled::load(&path).unwrap();
        assert!(h.contains(7));
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }
}
