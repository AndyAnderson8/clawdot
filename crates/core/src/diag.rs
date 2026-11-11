//! Recent warnings and errors, kept in memory only, and the build information (REQ-CFG-063, 064, 066).
//!
//! The core prints its problems to the console, which on a speaker with no screen nobody sees. Each one is
//! also kept here, redacted by the audit log's rules and clipped, in a ring of [`MAX_LINES`] lines that is lost at
//! restart and never written to disk. Only the core's own messages go in: not audio, not what the user said, not
//! note text and not event text.

use serde::Serialize;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::sync::{Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

/// Most lines kept; the oldest is dropped first.
pub const MAX_LINES: usize = 200;
/// Longest line, in characters.
pub const MAX_CHARS: usize = 300;

/// One remembered message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Line {
    /// Unix seconds.
    pub ts: i64,
    /// `warn` or `error`.
    pub level: &'static str,
    /// The message with secrets removed.
    pub text: String,
}

static RING: Mutex<VecDeque<Line>> = Mutex::new(VecDeque::new());

fn push(level: &'static str, msg: &str) {
    eprintln!("clawdot: {msg}");
    let mut text = crate::audit::redact_text(msg);
    if text.chars().count() > MAX_CHARS {
        text = text.chars().take(MAX_CHARS - 1).collect::<String>() + "…";
    }
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0));
    let mut ring = RING.lock().unwrap_or_else(PoisonError::into_inner);
    if ring.len() >= MAX_LINES {
        ring.pop_front();
    }
    ring.push_back(Line { ts, level, text });
}

/// Print a warning and remember it.
pub fn warn(msg: &str) {
    push("warn", msg);
}

/// Print an error and remember it.
pub fn error(msg: &str) {
    push("error", msg);
}

/// The most recent lines, newest first.
#[must_use]
pub fn recent(limit: usize) -> Vec<Line> {
    RING.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .rev()
        .take(limit)
        .cloned()
        .collect()
}

/// The commit this program was built from: the short hash, with `-dirty` when tracked files differed, or `unknown`
/// when the build had no git (a source tarball). Set by `build.rs`.
pub const COMMIT: &str = env!("CLAWDOT_BUILD_COMMIT");

/// Whether tracked files differed from [`COMMIT`] when this was built.
pub const DIRTY: bool = matches!(env!("CLAWDOT_BUILD_DIRTY").as_bytes(), b"1");

/// The version and commit as one string, such as `0.0.0 (a1b2c3d4e)` or `0.0.0 (a1b2c3d4e-dirty)`. The version
/// comes first and is unchanged, so anything that looks for it in the text still finds it.
#[must_use]
pub fn build_id() -> String {
    let version = env!("CARGO_PKG_VERSION");
    match (COMMIT, DIRTY) {
        ("unknown", _) => format!("{version} (unknown commit)"),
        (c, true) => format!("{version} ({c}-dirty)"),
        (c, false) => format!("{version} ({c})"),
    }
}

/// What this program is: version, build profile, platform and the optional features compiled in (REQ-CFG-066).
#[must_use]
pub fn build_info() -> Value {
    let mut features = Vec::new();
    if cfg!(feature = "tz") {
        features.push("tz");
    }
    if cfg!(feature = "tls") {
        features.push("tls");
    }
    if cfg!(feature = "esphome-satellite") {
        features.push("esphome-satellite");
    }
    if cfg!(feature = "esphome-noise") {
        features.push("esphome-noise");
    }
    json!({
        "version": env!("CARGO_PKG_VERSION"),
        "commit": COMMIT,
        "dirty": DIRTY,
        "id": build_id(),
        "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "features": features,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // REQ-CFG-064
    #[test]
    fn lines_are_bounded_redacted_clipped_and_newest_first() {
        warn("diag-test first with Authorization: Bearer abcdefghijklmnopqrstuvwxyz123456");
        error(&format!("diag-test long {}", "x ".repeat(400)));
        for i in 0..(MAX_LINES + 20) {
            warn(&format!("diag-test filler {i}"));
        }
        let all = recent(usize::MAX);
        assert!(all.len() <= MAX_LINES, "{}", all.len());
        assert_eq!(recent(3).len(), 3);
        assert!(
            all[0].text.ends_with(&format!("{}", MAX_LINES + 19)),
            "newest first"
        );
        assert!(all.iter().all(|l| l.text.chars().count() <= MAX_CHARS));
        // the oldest were dropped, so the first two are gone; check the rules on fresh lines
        warn("diag-test secret Bearer abcdefghijklmnopqrstuvwxyz123456 end");
        let l = &recent(1)[0];
        assert!(
            !l.text.contains("abcdefghijklmnopqrstuvwxyz123456"),
            "{}",
            l.text
        );
        error(&format!("diag-test long {}", "y ".repeat(400)));
        let l = &recent(1)[0];
        assert_eq!((l.level, l.text.chars().count()), ("error", MAX_CHARS));
        assert!(l.ts > 1_600_000_000);
    }

    // REQ-CFG-066
    #[test]
    fn the_build_information_names_the_version_and_the_platform() {
        let b = build_info();
        assert_eq!(b["version"], env!("CARGO_PKG_VERSION"));
        assert!(["debug", "release"].contains(&b["profile"].as_str().unwrap()));
        assert!(b["os"].as_str().is_some_and(|s| !s.is_empty()));
        assert!(b["features"].is_array());
    }

    #[test]
    fn the_build_id_keeps_the_version_first_and_names_the_commit() {
        let b = build_info();
        let id = build_id();
        assert!(id.starts_with(env!("CARGO_PKG_VERSION")), "{id}");
        assert_eq!(b["id"], id);
        assert_eq!(b["commit"], COMMIT);
        assert_eq!(b["dirty"], DIRTY);
        assert!(id.contains(COMMIT) || COMMIT == "unknown", "{id}");
        assert!(COMMIT.chars().all(|c| c.is_ascii_alphanumeric()));
    }
}
