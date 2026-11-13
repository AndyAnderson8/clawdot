//! The OpenAPI description in `docs/api/openapi.yaml` must match the routes in the code.
//!
//! Both sides are reduced to a set of `METHOD /path` lines, with every path parameter written as `{}`, and the
//! sets must be equal, so every route the server answers must be documented and every documented route must be in
//! a table. The code side is the two route tables (`http::ROUTES` and `ui::ROUTES`), read through
//! `http::documented_routes`; no source text is scraped. The document side is read from the `paths:` section as
//! text, not parsed as YAML, so it keeps its path keys at two spaces and methods at four.

use std::collections::BTreeSet;

const SPEC: &str = include_str!("../../../docs/api/openapi.yaml");
/// Routes in the document: path keys at two spaces, method keys at four, under `paths:`.
fn spec_routes() -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut in_paths = false;
    let mut path = String::new();
    for line in SPEC.lines() {
        if !line.starts_with(' ') && !line.trim().is_empty() {
            in_paths = line.trim_end() == "paths:";
            continue;
        }
        if !in_paths {
            continue;
        }
        if let Some(p) = line.strip_prefix("  /").and_then(|r| r.strip_suffix(':')) {
            path = braces_empty(&format!("/{p}"));
        } else if let Some(m) = line.strip_prefix("    ").and_then(|r| r.strip_suffix(':')) {
            if matches!(m, "get" | "post" | "put" | "delete" | "patch" | "head") {
                out.insert(format!("{} {path}", m.to_ascii_uppercase()));
            }
        }
    }
    out
}

/// `/tools/{name}/tier` -> `/tools/{}/tier`.
fn braces_empty(path: &str) -> String {
    let mut out = String::new();
    let mut skipping = false;
    for c in path.chars() {
        match c {
            '{' => {
                out.push('{');
                skipping = true;
            }
            '}' => {
                out.push('}');
                skipping = false;
            }
            _ if skipping => {}
            c => out.push(c),
        }
    }
    out
}

/// The routes the code serves, as the document must describe them: every row of both route tables
/// (`http::ROUTES` and `ui::ROUTES`, read through `http::documented_routes`) except the HTML page.
/// `/api/skills/{action}` is one document path for four literal actions (`skill_action`) and the code matches it
/// with a parameter, so it needs no special case.
fn code_routes() -> BTreeSet<String> {
    let rows = clawdot_core::http::documented_routes();
    let set: BTreeSet<String> = rows
        .iter()
        .map(|(method, path)| format!("{method} {}", braces_empty(path)))
        .collect();
    assert_eq!(set.len(), rows.len(), "a route is in the tables twice");
    set
}

#[test]
fn the_route_readers_find_the_routes() {
    let code = code_routes();
    for want in [
        "GET /api/status",
        "POST /api/tools/{}/tier",
        "GET /api/skills",
        "PUT /api/notes/{}",
        "POST /hook/{}",
        "POST /api/announce",
        "POST /mcp",
        "GET /bridge/{}/replies",
        "POST /bridge/{}/replies/{}/ack",
        "GET /healthz",
    ] {
        assert!(code.contains(want), "{want} not in {code:?}");
    }
    assert!(!code.contains("GET /"), "the page is not API");
    assert!(code.len() >= 40, "only {} routes read", code.len());
    assert!(spec_routes().len() >= 40);
}

#[test]
fn the_openapi_document_describes_exactly_the_routes_in_the_code() {
    let code = code_routes();
    let spec = spec_routes();
    let undocumented: Vec<_> = code.difference(&spec).collect();
    let stale: Vec<_> = spec.difference(&code).collect();
    assert!(
        undocumented.is_empty() && stale.is_empty(),
        "docs/api/openapi.yaml is out of step with the code.\n  in the code but not described: {undocumented:?}\n  described but not in the code: {stale:?}"
    );
}

#[test]
fn every_operation_has_an_id_and_responses() {
    let ids = SPEC.matches("      operationId: ").count();
    let ops = spec_routes().len();
    assert_eq!(ids, ops, "each operation needs one operationId");
    assert!(SPEC.matches("      responses:\n").count() >= ops);
    assert!(SPEC.starts_with("openapi: 3.1."));
}
