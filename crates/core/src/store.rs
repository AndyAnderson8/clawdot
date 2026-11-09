//! Small atomic JSON persistence (spec 0001 REQ-NFR-008, REQ-NFR-010; ADR "no embedded SQL").
//!
//! Files are written to a temporary sibling, flushed, then renamed over the target, so a crash never
//! leaves a half-written file. Every file carries a schema version so upgrades can migrate or refuse
//! clearly instead of misreading old data.

use serde::{Serialize, de::DeserializeOwned};
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Why a store operation failed.
#[derive(Debug)]
pub enum StoreError {
    /// Reading or writing the file failed.
    Io(io::Error),
    /// The file is not valid JSON of the expected shape.
    Corrupt(String),
    /// The file was written by a newer version than this build understands.
    NewerSchema {
        /// Version found in the file.
        found: u32,
        /// Highest version this build understands.
        supported: u32,
    },
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "storage I/O error: {e}"),
            Self::Corrupt(e) => write!(f, "stored data is corrupt: {e}"),
            Self::NewerSchema { found, supported } => write!(
                f,
                "stored data uses schema {found} but this build supports up to {supported}; upgrade ClawDot"
            ),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<io::Error> for StoreError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

#[derive(Serialize, serde::Deserialize)]
struct Envelope<T> {
    schema: u32,
    data: T,
}

/// A single JSON document on disk.
#[derive(Debug, Clone)]
pub struct JsonStore {
    path: PathBuf,
    schema: u32,
}

impl JsonStore {
    /// A store at `path` whose current schema version is `schema`.
    pub fn new(path: impl Into<PathBuf>, schema: u32) -> Self {
        Self {
            path: path.into(),
            schema,
        }
    }

    /// Where the document lives.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Load the document, or `T::default()` if it does not exist yet.
    ///
    /// # Errors
    /// [`StoreError::Corrupt`] for unreadable JSON, [`StoreError::NewerSchema`] if written by a newer build.
    pub fn load<T: DeserializeOwned + Default>(&self) -> Result<T, StoreError> {
        let bytes = match fs::read(&self.path) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(T::default()),
            Err(e) => return Err(e.into()),
        };
        // Read the schema first so a newer file is reported as such, not as "corrupt".
        let probe: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|e| StoreError::Corrupt(e.to_string()))?;
        let found = probe
            .get("schema")
            .and_then(serde_json::Value::as_u64)
            .and_then(|s| u32::try_from(s).ok())
            .ok_or_else(|| StoreError::Corrupt("missing schema version".into()))?;
        if found > self.schema {
            return Err(StoreError::NewerSchema {
                found,
                supported: self.schema,
            });
        }
        let env: Envelope<T> =
            serde_json::from_value(probe).map_err(|e| StoreError::Corrupt(e.to_string()))?;
        Ok(env.data)
    }

    /// Atomically replace the document.
    ///
    /// # Errors
    /// I/O failures, or a value that cannot be serialised.
    pub fn save<T: Serialize>(&self, value: &T) -> Result<(), StoreError> {
        let env = Envelope {
            schema: self.schema,
            data: value,
        };
        let bytes =
            serde_json::to_vec_pretty(&env).map_err(|e| StoreError::Corrupt(e.to_string()))?;
        if let Some(dir) = self.path.parent() {
            create_private_dir(dir)?;
        }
        let tmp = self.path.with_extension("tmp");
        {
            let mut f = create_private_file(&tmp)?;
            f.write_all(&bytes)?;
            f.sync_all()?;
        }
        fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

/// Create `dir` and any missing parents readable only by the service user (0700 on Unix). A directory
/// that already exists keeps its mode: it may be shared on purpose, and the files inside are private
/// either way (REQ-SEC-009).
fn create_private_dir(dir: &Path) -> io::Result<()> {
    let mut b = fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        b.mode(0o700);
    }
    b.create(dir)
}

/// Create (or truncate) a file that only the service user can read or write (0600 on Unix), with the
/// mode set at creation so there is no window in which it is world-readable.
fn create_private_file(path: &Path) -> io::Result<fs::File> {
    let mut o = fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(path)
}

#[cfg(test)]
pub(crate) mod testutil {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// A unique scratch directory under the system temp dir, removed on drop.
    pub struct TempDir(pub PathBuf);

    impl TempDir {
        pub fn new(tag: &str) -> Self {
            static N: AtomicU64 = AtomicU64::new(0);
            let n = N.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "clawdot-test-{}-{}-{}",
                tag,
                std::process::id(),
                n
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::TempDir;
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
    struct Doc {
        items: Vec<String>,
    }

    #[test]
    fn missing_file_loads_as_default() {
        let dir = TempDir::new("missing");
        let store = JsonStore::new(dir.0.join("d.json"), 1);
        assert_eq!(store.load::<Doc>().unwrap(), Doc::default());
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = TempDir::new("rt");
        let store = JsonStore::new(dir.0.join("sub/d.json"), 1);
        let doc = Doc {
            items: vec!["a".into(), "b".into()],
        };
        store.save(&doc).unwrap();
        assert_eq!(store.load::<Doc>().unwrap(), doc);
        assert!(
            !dir.0.join("sub/d.tmp").exists(),
            "temp file must not linger"
        );
    }

    #[cfg(unix)]
    #[test]
    fn state_is_private_to_the_service_user() {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        let dir = TempDir::new("private");
        let path = dir.0.join("data/inner/d.json");
        let store = JsonStore::new(path.clone(), 1);
        store.save(&Doc::default()).unwrap();
        assert_eq!(mode(&dir.0.join("data")), 0o700, "new directory");
        assert_eq!(mode(&dir.0.join("data/inner")), 0o700, "new subdirectory");
        assert_eq!(mode(&path), 0o600, "new file");
        // A rewrite (temp file renamed over the old one) stays private, even if the old file was open.
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        store.save(&Doc::default()).unwrap();
        assert_eq!(mode(&path), 0o600, "rewritten file");
    }

    #[test]
    fn a_crash_between_write_and_rename_leaves_the_old_data_intact() {
        let dir = TempDir::new("crash");
        let store = JsonStore::new(dir.0.join("d.json"), 1);
        store
            .save(&Doc {
                items: vec!["old".into()],
            })
            .unwrap();
        // Simulate a crash mid-save: a half-written temp file next to the real one.
        std::fs::write(dir.0.join("d.tmp"), b"{\"schema\":1,\"da").unwrap();
        assert_eq!(store.load::<Doc>().unwrap().items, vec!["old".to_string()]);
    }

    #[test]
    fn corrupt_files_are_reported_not_ignored() {
        let dir = TempDir::new("corrupt");
        let path = dir.0.join("d.json");
        std::fs::write(&path, b"not json").unwrap();
        assert!(matches!(
            JsonStore::new(path, 1).load::<Doc>(),
            Err(StoreError::Corrupt(_))
        ));
    }

    #[test]
    fn newer_schemas_are_refused_with_a_clear_error() {
        let dir = TempDir::new("newer");
        let path = dir.0.join("d.json");
        JsonStore::new(path.clone(), 5)
            .save(&Doc::default())
            .unwrap();
        assert!(matches!(
            JsonStore::new(path, 2).load::<Doc>(),
            Err(StoreError::NewerSchema {
                found: 5,
                supported: 2
            })
        ));
    }
}
