//! The OpenAPI description in `docs/api/openapi.yaml` must match the routes in the code.
//!
//! Both sides are reduced to a set of `METHOD /path` lines, with every path parameter written as `{}`, and the
//! sets must be equal. The code side is the route table in `src/ui.rs` (read through `ui::api_routes`, so every
//! `/api` route the table serves must be documented and every documented `/api` route must be in the table) plus
//! the `match segs` arms of `src/http.rs`; the document side is read from the `paths:` section. The document is
//! read as text, not parsed as YAML, so it keeps its path keys at two spaces and methods at four.

use std::collections::BTreeSet;

const SPEC: &str = include_str!("../../../docs/api/openapi.yaml");
const HTTP: &str = include_str!("../src/http.rs");

/// `["tools", name, "tier"]` -> `/tools/{}/tier`. Quoted words are literal; anything else is a parameter.
fn path_of(group: &str, prefix: &str) -> String {
    let mut out = String::from(prefix);
    for seg in group.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        out.push('/');
        match seg.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
            Some(lit) => out.push_str(lit),
            None => out.push_str("{}"),
        }
    }
    out
}

/// The `[...]` groups in `text`, without the brackets.
fn groups(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find('[') {
        let Some(len) = rest[start..].find(']') else {
            break;
        };
        out.push(&rest[start + 1..start + len]);
        rest = &rest[start + len + 1..];
    }
    out
}

/// Routes in the `ui.rs` table, all under `/api`: `("POST", "/tools/{name}/tier")` -> `POST /api/tools/{}/tier`.
fn ui_routes() -> BTreeSet<String> {
    clawdot_core::ui::api_routes()
        .into_iter()
        .map(|(method, path)| format!("{method} {}", braces_empty(&format!("/api{path}"))))
        .collect()
}

/// Routes in `http.rs`: a `[..] => match parts.method {` arm followed by `Method::X =>` lines.
fn http_routes() -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut current: Option<String> = None;
    for line in HTTP.lines().map(str::trim) {
        if line.starts_with("#[cfg(test)]") {
            break;
        }
        if line.starts_with('[') && line.contains("=> match parts.method") {
            current = groups(line).first().map(|g| path_of(g, ""));
        } else if let (Some(path), Some(rest)) = (&current, line.strip_prefix("Method::")) {
            if let Some((method, _)) = rest.split_once(' ') {
                out.insert(format!("{method} {path}"));
            }
        } else if line.starts_with("_ =>") && !line.contains("method_not_allowed") {
            current = None;
        }
    }
    out
}

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

/// The routes the code serves, as the document must describe them.
fn code_routes() -> BTreeSet<String> {
    let mut all = ui_routes();
    all.extend(http_routes());
    // `/api/skills/{action}` is one document path for four literal actions (`skill_action`) and the code
    // matches it with a parameter, so it needs no special case. `/api/announce` is in http.rs.
    all
}

#[test]
fn the_route_readers_find_the_routes() {
    let ui = ui_routes();
    assert!(ui.contains("GET /api/status"), "{ui:?}");
    assert!(ui.contains("POST /api/tools/{}/tier"), "{ui:?}");
    assert!(ui.contains("GET /api/skills"), "{ui:?}");
    assert!(ui.contains("PUT /api/notes/{}"), "{ui:?}");
    assert!(ui.len() >= 40, "only {} ui routes read", ui.len());
    let http = http_routes();
    assert!(http.contains("POST /hook/{}"), "{http:?}");
    assert!(http.contains("POST /api/announce"), "{http:?}");
    assert!(http.contains("POST /bridge/{}/replies/{}/ack"), "{http:?}");
    assert!(http.contains("GET /healthz"), "{http:?}");
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

/// The `/api` half on its own, both ways, so a failure names the table rather than the whole document. The one
/// route served from `http.rs` (`/api/announce`) is not in the table; the combined test above covers it.
#[test]
fn the_api_table_and_the_document_agree_in_both_directions() {
    let table = ui_routes();
    let http = http_routes();
    let documented: BTreeSet<String> = spec_routes()
        .into_iter()
        .filter(|r| {
            r.split_once(' ')
                .is_some_and(|(_, p)| p.starts_with("/api/"))
        })
        .filter(|r| !http.contains(r))
        .collect();
    let missing: Vec<_> = table.difference(&documented).collect();
    let extra: Vec<_> = documented.difference(&table).collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "in the route table but not documented: {missing:?}\n  documented under /api but not in the table: {extra:?}"
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
