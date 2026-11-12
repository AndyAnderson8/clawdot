//! The shape of a route table, shared by the two tables the server answers from: `http::ROUTES` (the page, health,
//! webhooks, announce, the MCP server and the agent bridge) and `ui::ROUTES` (the portal's `/api`).
//!
//! There are two tables, not one, because each row's `id` names a handler and the handlers live in different
//! modules: every table dispatches on its own `Id` enum with no catch-all arm, so a row without a handler or a
//! handler without a row does not compile. Both tables use the same [`Row`], the same matcher ([`find`], [`fit`])
//! and the same [`Auth`] classes, and `http::documented_routes` joins them for the OpenAPI check, so nothing about
//! a route is written outside a table.

/// Who may call a route, and so which check runs before anything else is read from the request.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Auth {
    /// No credential at all (`GET /healthz`). A wrong method is a plain 405.
    Public,
    /// The static page. No credential. A wrong method is a 405 with the portal's headers.
    Page,
    /// The portal's sign-in routes: what to show, setup and sign-in. A wrong method on these is a plain 404.
    Open,
    /// An administrator session, or a private-network peer in open mode (REQ-CFG-005). Checked before any body is
    /// read. A wrong method is a 405 once the caller is signed in.
    SignedIn,
    /// The token of the endpoint named in the path (`{endpoint}`), for the webhook and the agent bridge
    /// (REQ-INB-002). A wrong or unknown token spends from the shared failed-authentication budget (401, then 429).
    EndpointToken,
    /// Any endpoint's token, and nothing else: the MCP server (REQ-EXT-011).
    AnyEndpointToken,
    /// Any endpoint's token, or an administrator session: announce (REQ-INB-021).
    EndpointOrAdmin,
}

/// One route: the method, the path (`{name}` is one segment taken as a parameter; `{endpoint}` is one segment that
/// must also be a valid endpoint name), who may call it and what it does.
pub(crate) struct Row<I: 'static> {
    pub method: &'static str,
    pub path: &'static str,
    pub auth: Auth,
    pub id: I,
}

impl<I> Row<I> {
    pub(crate) const fn new(method: &'static str, path: &'static str, auth: Auth, id: I) -> Self {
        Self {
            method,
            path,
            auth,
            id,
        }
    }
}

/// The first two `{parameter}` segments of a match, empty when the pattern has fewer.
///
/// A parameter is the raw path segment and is never percent-decoded, because none needs an escape: every one is an
/// endpoint, tool or connection name (lowercase letters, digits, `-` and `_`; an MCP tool's remote name is
/// sanitised to those), one of the fixed skill actions, or a number. A segment that carries an escape names nothing
/// and gets the handler's "no such ..." answer. A new row whose parameter can hold other characters must decode it.
pub(crate) type Params<'a> = [&'a str; 2];

use crate::inbox::endpoint::valid_name;

/// Whether `segs` (the path split on `/`, without the outer slashes) fits `pattern`, and the parameters if so.
pub(crate) fn fit<'a>(pattern: &str, segs: &[&'a str]) -> Option<Params<'a>> {
    let mut want = pattern.split('/').skip(1);
    let mut have = segs.iter();
    let mut params = [""; 2];
    let mut taken = 0;
    loop {
        match (want.next(), have.next()) {
            (None, None) => return Some(params),
            (Some(w), Some(h)) if w.starts_with('{') => {
                if w == "{endpoint}" && !valid_name(h) {
                    return None;
                }
                if taken < params.len() {
                    params[taken] = h;
                    taken += 1;
                }
            }
            (Some(w), Some(h)) if w == *h => {}
            _ => return None,
        }
    }
}

/// The row for this method and path, and its parameters.
pub(crate) fn find<'a, I>(
    rows: &'static [Row<I>],
    method: &str,
    segs: &[&'a str],
) -> Option<(&'static Row<I>, Params<'a>)> {
    rows.iter()
        .filter(|r| r.method == method)
        .find_map(|r| fit(r.path, segs).map(|p| (r, p)))
}

/// The first row whose path fits, whatever its method: the path exists, so a wrong method is a 405 (or a 404,
/// by the row's class).
pub(crate) fn path_row<I>(rows: &'static [Row<I>], segs: &[&str]) -> Option<&'static Row<I>> {
    rows.iter().find(|r| fit(r.path, segs).is_some())
}

/// A real-looking path for a pattern: every `{parameter}` segment becomes `1`.
#[cfg(test)]
pub(crate) fn sample_path(pattern: &str) -> String {
    pattern
        .split('/')
        .map(|s| if s.starts_with('{') { "1" } else { s })
        .collect::<Vec<_>>()
        .join("/")
}

/// A table is well formed: methods and paths are plain, no row is written twice, and looking a row up by its own
/// sample path finds that row and not an earlier one that covers it.
#[cfg(test)]
pub(crate) fn assert_well_formed<I>(rows: &'static [Row<I>]) {
    let mut seen = std::collections::BTreeSet::new();
    for row in rows {
        assert!(
            matches!(row.method, "GET" | "HEAD" | "POST" | "PUT" | "DELETE"),
            "{}",
            row.path
        );
        assert!(row.path.starts_with('/'), "{}", row.path);
        if row.path != "/" {
            for seg in row.path.split('/').skip(1) {
                assert!(!seg.is_empty(), "{}", row.path);
            }
        }
        for seg in row.path.split('/').skip(1) {
            assert!(
                !seg.contains(['{', '}']) || (seg.starts_with('{') && seg.ends_with('}')),
                "{}",
                row.path
            );
        }
        assert!(
            seen.insert((row.method, row.path)),
            "twice: {} {}",
            row.method,
            row.path
        );
        let sample = sample_path(row.path);
        let segs: Vec<&str> = sample.trim_matches('/').split('/').collect();
        let (hit, _) = find(rows, row.method, &segs)
            .unwrap_or_else(|| panic!("{} {} is not found", row.method, row.path));
        assert!(
            hit.path == row.path,
            "{} {} is hidden by {}",
            row.method,
            row.path,
            hit.path
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_parameter_is_the_segment_in_its_place() {
        assert_eq!(
            fit("/tools/{name}/tier", &["tools", "a b", "tier"]),
            Some(["a b", ""])
        );
        assert_eq!(fit("/tools/{name}/tier", &["tools", "x", "other"]), None);
        assert_eq!(fit("/tools/{name}", &["tools", "x", "tier"]), None);
        assert_eq!(fit("/tools", &["tools"]), Some(["", ""]));
        assert_eq!(fit("/tools/{name}", &["tools", ""]), Some(["", ""]));
        assert_eq!(
            fit("/tools/{name}", &["tools", "%61"]),
            Some(["%61", ""]),
            "a parameter is never decoded"
        );
        assert_eq!(fit("/tools", &[]), None);
        assert_eq!(fit("/", &[""]), Some(["", ""]));
    }

    #[test]
    fn two_parameters_are_returned_and_an_endpoint_segment_must_be_a_valid_name() {
        let p = "/bridge/{endpoint}/replies/{id}/ack";
        let ok = ["bridge", "claude-code", "replies", "7", "ack"];
        assert_eq!(fit(p, &ok), Some(["claude-code", "7"]));
        // An invalid name is not this route at all (the caller sees 404, not 401).
        assert_eq!(fit(p, &["bridge", "UPPER", "replies", "7", "ack"]), None);
        // The first character is a letter or digit, the same rule that creating an endpoint applies.
        assert_eq!(fit(p, &["bridge", "_x", "replies", "7", "ack"]), None);
        assert_eq!(fit(p, &["bridge", "-x", "replies", "7", "ack"]), None);
        assert!(fit(p, &["bridge", "x_-", "replies", "7", "ack"]).is_some());
        assert_eq!(fit(p, &["bridge", "%61", "replies", "7", "ack"]), None);
        assert_eq!(fit(p, &["bridge", "", "replies", "7", "ack"]), None);
        let long = "a".repeat(33);
        assert_eq!(fit(p, &["bridge", &long, "replies", "7", "ack"]), None);
        // The id is any segment.
        assert!(fit(p, &["bridge", "x", "replies", "not a number", "ack"]).is_some());
    }
}
