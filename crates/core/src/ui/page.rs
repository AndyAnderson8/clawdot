//! The static portal page: the HTML, its gzip copy, the content-security policy, and the response that serves them.

use super::common::{Resp, reply};
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::header::{CONTENT_TYPE, HeaderValue};
use hyper::{Response, StatusCode};

#[cfg(test)]
mod csp_tests;

/// The page and a gzip copy of it. `ui.html` is generated from the TypeScript sources in `portal/` and
/// committed (ADR-0016); `build.rs` copies it and makes the gzip, so the program carries no compression code
/// (REQ-CFG-043).
pub(super) const PAGE: &str = include_str!(concat!(env!("OUT_DIR"), "/ui-page.html"));
pub(super) const PAGE_GZ: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/ui-page.html.gz"));

/// Only the page's own inline script and style, allowed by their SHA-256 (computed from the page by `build.rs`, so
/// there is no `'unsafe-inline'` and an injected inline script or style is refused), and only calls back to this
/// origin. Pictures are the one thing it may load from elsewhere, and only when a person gives an image address for
/// the picture of their device (or has one saved in the page as data); nothing else on the page asks another site
/// for anything.
pub(super) const CSP: &str = include_str!(concat!(env!("OUT_DIR"), "/ui-csp.txt"));

/// The static page.
#[must_use]
pub fn page(accept_encoding: Option<&str>) -> Resp {
    let mut r = if accepts_gzip(accept_encoding) {
        let mut r = Response::new(Full::new(Bytes::from_static(PAGE_GZ)).boxed());
        r.headers_mut().insert(
            CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        );
        r.headers_mut()
            .insert("cache-control", HeaderValue::from_static("no-store"));
        r.headers_mut().insert(
            "x-content-type-options",
            HeaderValue::from_static("nosniff"),
        );
        r.headers_mut()
            .insert("content-encoding", HeaderValue::from_static("gzip"));
        r
    } else {
        reply(StatusCode::OK, "text/html; charset=utf-8", PAGE.to_owned())
    };
    r.headers_mut()
        .insert("vary", HeaderValue::from_static("Accept-Encoding"));
    r.headers_mut()
        .insert("content-security-policy", HeaderValue::from_static(CSP));
    r.headers_mut()
        .insert("x-frame-options", HeaderValue::from_static("DENY"));
    r.headers_mut()
        .insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    r
}

/// True when an `Accept-Encoding` header lists gzip (or `*`) with a quality above zero.
pub(super) fn accepts_gzip(header: Option<&str>) -> bool {
    header.is_some_and(|h| {
        h.split(',').any(|item| {
            let mut parts = item.split(';');
            let name = parts.next().unwrap_or("").trim();
            let refused = parts.any(|p| {
                p.trim()
                    .strip_prefix("q=")
                    .is_some_and(|q| q.trim().parse::<f32>().is_ok_and(|q| q <= 0.0))
            });
            (name.eq_ignore_ascii_case("gzip") || name == "*") && !refused
        })
    })
}
