//! Prepares the embedded portal page (REQ-CFG-043): copies the committed page to `OUT_DIR` and writes a gzip
//! copy beside it. Compression happens here, at build time, so the running program needs no compression code
//! at all; it only serves one of the two byte strings.
//!
//! `src/ui.html` is generated from the TypeScript sources in `portal/` (see ADR-0016) and committed, so a Rust
//! build never needs Node. CI rebuilds the page and fails if the committed copy is out of date.

use flate2::{Compression, write::GzEncoder};
use std::io::Write;
use std::path::PathBuf;

/// Run `git` with the given arguments and return its trimmed output, or `None` when git is missing, the source
/// is not a checkout (a tarball), or the command fails.
fn git(args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// Tell the program which commit it was built from: `CLAWDOT_BUILD_COMMIT` (short hash, or `unknown`) and
/// `CLAWDOT_BUILD_DIRTY` (`1` when tracked files differ from the commit). A build outside a checkout, or with no
/// git, gets `unknown`; `CLAWDOT_BUILD_COMMIT` in the environment overrides git (for tarball and packaging builds).
fn stamp_commit() {
    println!("cargo:rerun-if-env-changed=CLAWDOT_BUILD_COMMIT");
    println!("cargo:rerun-if-env-changed=CLAWDOT_BUILD_DIRTY");
    let from_env = std::env::var("CLAWDOT_BUILD_COMMIT")
        .ok()
        .filter(|s| !s.is_empty());
    let dirty_env = std::env::var("CLAWDOT_BUILD_DIRTY").ok();
    let commit = from_env.or_else(|| git(&["rev-parse", "--short=9", "HEAD"]));
    let dirty = match (&dirty_env, &commit) {
        (Some(d), _) => d == "1",
        (None, Some(_)) => git(&["status", "--porcelain", "--untracked-files=no"]).is_some(),
        (None, None) => false,
    };
    if let Some(dir) = git(&["rev-parse", "--git-dir"]) {
        println!("cargo:rerun-if-changed={dir}/HEAD");
        println!("cargo:rerun-if-changed={dir}/index");
    }
    // Only characters a hash can hold, so nothing odd reaches the compiler or the output.
    let commit = commit
        .filter(|c| c.len() <= 40 && c.chars().all(|ch| ch.is_ascii_alphanumeric()))
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=CLAWDOT_BUILD_COMMIT={commit}");
    println!("cargo:rustc-env=CLAWDOT_BUILD_DIRTY={}", u8::from(dirty));
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    stamp_commit();
    println!("cargo:rerun-if-changed=src/ui.html");
    println!("cargo:rerun-if-changed=build.rs");
    let page = std::fs::read_to_string("src/ui.html")?;
    if !page.contains("<script>") {
        return Err(
            "src/ui.html looks wrong: it has no inline script (run `npm run build` in portal/)"
                .into(),
        );
    }
    let out = PathBuf::from(std::env::var("OUT_DIR")?);
    std::fs::write(out.join("ui-page.html"), &page)?;
    let mut gz = GzEncoder::new(Vec::new(), Compression::best());
    gz.write_all(page.as_bytes())?;
    std::fs::write(out.join("ui-page.html.gz"), gz.finish()?)?;
    Ok(())
}
