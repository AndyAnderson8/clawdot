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
fn stamp_target() {
    // The compiler's own name for what is being built (for example `armv7-unknown-linux-musleabihf`). It is the
    // only reliable way to tell ARMv7 from ARMv6: a program cannot see `target_feature = "v7"` on stable Rust.
    let target = std::env::var("TARGET").unwrap_or_default();
    let target: String = target
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .collect();
    println!("cargo:rustc-env=CLAWDOT_BUILD_TARGET={target}");
}

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

/// Base64 (standard alphabet, padded).
fn base64(data: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in data.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(c.get(1).copied().unwrap_or(0)) << 8)
            | u32::from(c.get(2).copied().unwrap_or(0));
        for i in 0..4 {
            if i <= c.len() {
                out.push(A[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The CSP hash sources (`'sha256-...'`, space separated) of every inline `<tag>...</tag>` block of the page, with the
/// exact bytes between the tags. A tag with attributes is refused: the policy would not cover it.
fn inline_hashes(page: &str, tag: &str) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let (open, close) = (format!("<{tag}>"), format!("</{tag}>"));
    if page.matches(&format!("<{tag}")).count() != page.matches(&open).count() {
        return Err(format!(
            "src/ui.html has a <{tag}> with attributes; the policy only covers plain inline blocks"
        ));
    }
    let mut hashes: Vec<String> = Vec::new();
    let mut rest = page;
    while let Some(i) = rest.find(&open) {
        let body = &rest[i + open.len()..];
        let end = body
            .find(&close)
            .ok_or_else(|| format!("an unclosed <{tag}> in src/ui.html"))?;
        let h = format!(
            "'sha256-{}'",
            base64(&Sha256::digest(&body.as_bytes()[..end]))
        );
        if !hashes.contains(&h) {
            hashes.push(h);
        }
        rest = &body[end + close.len()..];
    }
    if hashes.is_empty() {
        return Err(format!("src/ui.html has no inline <{tag}>"));
    }
    Ok(hashes.join(" "))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    stamp_commit();
    stamp_target();
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
    // The page's content-security policy: its own inline script and style by hash, no 'unsafe-inline'. Pictures are
    // the one thing it may load from elsewhere (a person's image address for their device).
    let csp = format!(
        "default-src 'none'; script-src {}; style-src {}; connect-src 'self'; img-src data: https: http:; \
         base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
        inline_hashes(&page, "script")?,
        inline_hashes(&page, "style")?
    );
    std::fs::write(out.join("ui-csp.txt"), csp)?;
    let mut gz = GzEncoder::new(Vec::new(), Compression::best());
    gz.write_all(page.as_bytes())?;
    std::fs::write(out.join("ui-page.html.gz"), gz.finish()?)?;
    Ok(())
}
