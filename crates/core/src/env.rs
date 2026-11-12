//! The process environment, read once.
//!
//! [`Env::from_process`] is the only place in the core that reads the process environment, and `main` calls it
//! once, first. Everything that needs a variable (a key named by a setting, a token named by the configuration, a
//! supervisor flag) is handed an [`Env`] or a lookup made from one.
//!
//! Some variable NAMES are chosen by a setting and looked up later, when a client is built again after a live
//! settings change. That still resolves the name chosen at that time, in the snapshot taken at start-up. The
//! environment of a running process does not change (nothing here sets variables), so the snapshot answers exactly
//! what reading the process environment would have.
//!
//! Values are secrets. `Debug` shows how many variables there are, never their names or values.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

/// A lookup of environment variables by name, as a closure that can be kept.
pub type EnvFn = Box<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// A snapshot of the environment. Cheap to clone: clones share one map.
#[derive(Clone, Default)]
pub struct Env(Arc<BTreeMap<String, String>>);

impl Env {
    /// The environment of this process, as it is now. Variables that are not valid Unicode are left out.
    /// Call it once at start-up; this is the only read of the process environment in the core.
    #[must_use]
    pub fn from_process() -> Self {
        Self(Arc::new(
            std::env::vars_os()
                .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
                .collect(),
        ))
    }

    /// An environment with exactly these variables (tests, and embedding the core).
    #[must_use]
    pub fn from_pairs<K: Into<String>, V: Into<String>>(
        pairs: impl IntoIterator<Item = (K, V)>,
    ) -> Self {
        Self(Arc::new(
            pairs
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        ))
    }

    /// No variables at all.
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// The value of `name`, if it is set.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<String> {
        self.0.get(name).cloned()
    }

    /// Whether `name` is set (even to an empty value).
    #[must_use]
    pub fn is_set(&self, name: &str) -> bool {
        self.0.contains_key(name)
    }

    /// A lookup that keeps its own handle on the snapshot, for code that stores one.
    pub fn lookup(&self) -> EnvFn {
        let env = self.clone();
        Box::new(move |n| env.get(n))
    }
}

impl fmt::Debug for Env {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Env")
            .field("variables", &self.0.len())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lookup_answers_from_the_snapshot() {
        let env = Env::from_pairs([("A_KEY", "alpha"), ("EMPTY", "")]);
        assert_eq!(env.get("A_KEY").as_deref(), Some("alpha"));
        assert_eq!(env.get("EMPTY").as_deref(), Some(""));
        assert!(env.is_set("EMPTY") && !env.is_set("NOPE"));
        assert_eq!(env.get("NOPE"), None);
        let look = env.lookup();
        drop(env);
        assert_eq!(
            look("A_KEY").as_deref(),
            Some("alpha"),
            "it keeps its own handle"
        );
        assert_eq!(Env::empty().get("A_KEY"), None);
    }

    #[test]
    fn debug_shows_neither_names_nor_values() {
        let env = Env::from_pairs([("SECRET_NAME_X", "hunter2-value")]);
        let shown = format!("{env:?} {env:#?}");
        assert!(
            !shown.contains("hunter2") && !shown.contains("SECRET_NAME_X"),
            "{shown}"
        );
        assert!(shown.contains("variables"), "{shown}");
    }

    #[test]
    fn the_process_snapshot_matches_the_process() {
        // PATH is set in nearly every environment; reading a variable is not a mutation.
        let env = Env::from_process();
        assert_eq!(env.get("PATH"), std::env::var("PATH").ok());
    }
}
