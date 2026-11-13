//! The portal page's content-security policy (threat T1): inline script and style are allowed by hash only.
#![allow(clippy::unwrap_used)]

use super::*;

/// Base64 (standard alphabet, padded) of bytes, for the hashes in the policy.
fn b64(data: &[u8]) -> String {
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

/// The `'sha256-...'` sources of every inline `<tag>...</tag>` block of the page, found independently of build.rs.
fn page_hashes(tag: &str) -> Vec<String> {
    use sha2::{Digest, Sha256};
    let (open, close) = (format!("<{tag}>"), format!("</{tag}>"));
    let mut out = Vec::new();
    let mut rest = PAGE;
    while let Some(i) = rest.find(&open) {
        let body = &rest[i + open.len()..];
        let end = body.find(&close).unwrap();
        out.push(format!(
            "'sha256-{}'",
            b64(&Sha256::digest(&body.as_bytes()[..end]))
        ));
        rest = &body[end + close.len()..];
    }
    out
}

/// The policy allows the page's own inline script and style by hash, so an injected inline script or style is
/// refused; it must never fall back to `'unsafe-inline'`.
#[test]
fn the_policy_allows_the_pages_inline_blocks_by_hash_and_not_unsafe_inline() {
    assert!(!CSP.contains("unsafe-inline"), "{CSP}");
    assert!(!CSP.contains("unsafe-eval"), "{CSP}");
    let directive = |name: &str| -> Vec<&str> {
        CSP.split(';')
            .map(str::trim)
            .find_map(|d| d.strip_prefix(name))
            .unwrap_or_else(|| panic!("no {name} in {CSP}"))
            .split_whitespace()
            .collect()
    };
    for (name, tag) in [("script-src", "script"), ("style-src", "style")] {
        let want = page_hashes(tag);
        assert!(!want.is_empty(), "the page has no inline <{tag}>");
        let mut want: Vec<&str> = want.iter().map(String::as_str).collect();
        want.sort_unstable();
        want.dedup();
        let mut have = directive(name);
        have.sort_unstable();
        assert_eq!(
            have, want,
            "{name} lists exactly the page's own {tag} blocks"
        );
    }
    // Nothing in the page can bring its own script or style: no attributes on the tags, no style attributes.
    assert!(!PAGE.contains("<script "), "a <script> with attributes");
    assert!(!PAGE.contains("<style "), "a <style> with attributes");
    assert!(!PAGE.contains(" style=\""), "an inline style attribute");
}
