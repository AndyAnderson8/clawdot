//! Which `Host` names the portal answers to (threat T5, DNS rebinding).
//!
//! In open mode (REQ-CFG-005) the only credential is the caller's private address, and a browser on that network
//! lends its address to any page it shows. DNS rebinding makes this worse than a cross-site request: the attacker's
//! own name is first resolved to the attacker's server, which serves a page, and then re-resolved to the device's
//! address. The page and the device now share one origin, so the `Origin` check cannot tell them apart. What does
//! not change is the `Host` header, which still carries the attacker's name. So the portal (the page and the
//! `/api` routes that take no token) is served only for names that really are the device's:
//!
//! * an IP address written out (an attacker's name is never one),
//! * `localhost`,
//! * the host of the public address setting (`public_url`),
//! * the machine's own host name, with or without `.local` (what mDNS answers for).
//!
//! Routes that carry their own token (`/hook`, `/mcp`, `/bridge`, `/api/announce` with a token) are not checked:
//! a rebound page does not have the token.

use std::net::IpAddr;

/// The host part of an `authority` (a `Host` header or the part of a URL after `://`): lower case, no port, no
/// brackets round an IPv6 address, no trailing dot. `None` when it is not a plausible name or address.
#[must_use]
pub(crate) fn host_of(authority: &str) -> Option<String> {
    let authority = authority.trim();
    let host = if let Some(rest) = authority.strip_prefix('[') {
        // [v6] or [v6]:port
        let (inside, after) = rest.split_once(']')?;
        if !(after.is_empty() || after.strip_prefix(':').is_some_and(is_port)) {
            return None;
        }
        inside
    } else if let Some((h, port)) = authority.rsplit_once(':') {
        if !is_port(port) || h.contains(':') {
            return None;
        }
        h
    } else {
        authority
    };
    let host = host.strip_suffix('.').unwrap_or(host).to_ascii_lowercase();
    let plausible = !host.is_empty()
        && host.len() <= 253
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b':'));
    plausible.then_some(host)
}

fn is_port(s: &str) -> bool {
    !s.is_empty() && s.len() <= 5 && s.bytes().all(|b| b.is_ascii_digit())
}

/// The host of a URL such as `http://speaker.local:3335`; `None` for an empty or unparseable value.
#[must_use]
pub(crate) fn host_of_url(url: &str) -> Option<String> {
    let rest = url.trim().split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    host_of(authority.rsplit_once('@').map_or(authority, |(_, h)| h))
}

/// Whether a request that arrived for `host_header` is addressed to this device. `public_url` is the public
/// address setting (may be empty); `machine` is the machine's host name, if it could be read.
#[must_use]
pub(crate) fn host_is_ours(host_header: &str, public_url: &str, machine: Option<&str>) -> bool {
    let Some(host) = host_of(host_header) else {
        return false;
    };
    if host.parse::<IpAddr>().is_ok() || host == "localhost" {
        return true;
    }
    if host_of_url(public_url).is_some_and(|p| p == host) {
        return true;
    }
    let Some(machine) = machine.and_then(host_of) else {
        return false;
    };
    // A full name (`dot.example.lan`) is answered to as written, and by its first label, which is what the
    // network's search domain and mDNS (`dot.local`) add to.
    let first = machine.split('.').next().unwrap_or(&machine);
    host == machine || host == first || host == format!("{first}.local")
}

/// The name other machines on the network use for this one, as printed in snippets (REQ-CFG-016): the first label
/// of the machine's host name with `.local` added, which is what mDNS answers for (the same form `host_is_ours`
/// accepts). `clawdot.local` only when the machine's name is not known or is not a usable name.
#[must_use]
pub(crate) fn local_name(machine: Option<&str>) -> String {
    machine
        .and_then(host_of)
        .filter(|h| h.parse::<IpAddr>().is_err())
        .and_then(|h| h.split('.').next().map(str::to_owned))
        .filter(|l| {
            !l.is_empty()
                && l.len() <= 63
                && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
        .map_or_else(|| "clawdot.local".to_owned(), |l| format!("{l}.local"))
}

/// The machine's host name, from the system's host name files and then the start-up `env` snapshot (`None` when it
/// cannot be found, so only the other forms match). Called once at start-up (`Core::with_env`); the request path uses
/// the kept value and does no file or environment access.
#[must_use]
pub(crate) fn machine_name(env: &crate::env::Env) -> Option<String> {
    #[cfg(unix)]
    {
        for path in ["/proc/sys/kernel/hostname", "/etc/hostname"] {
            if let Ok(s) = std::fs::read_to_string(path) {
                let s = s.trim();
                if !s.is_empty() {
                    return Some(s.to_owned());
                }
            }
        }
    }
    name_from_env(env)
}

/// The host name a start-up environment snapshot gives (`HOSTNAME`, then `COMPUTERNAME`).
fn name_from_env(env: &crate::env::Env) -> Option<String> {
    ["HOSTNAME", "COMPUTERNAME"]
        .iter()
        .find_map(|k| env.get(k))
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ours(h: &str) -> bool {
        host_is_ours(h, "", Some("dot"))
    }

    #[test]
    fn the_ways_a_person_reaches_the_portal_are_accepted() {
        for h in [
            "192.168.1.20:3335",
            "10.0.0.5",
            "127.0.0.1:3335",
            "[::1]:3335",
            "[fe80::1]",
            "localhost:3335",
            "LOCALHOST",
            "localhost.:3335",
            "dot.local:3335",
            "DOT.local:3335",
            "dot:3335",
            "dot",
        ] {
            assert!(ours(h), "{h}");
        }
    }

    #[test]
    fn a_name_an_attacker_controls_is_refused_however_it_is_written() {
        for h in [
            "evil.example:3335",
            "evil.example",
            "dot.evil.example",
            "dot.local.evil.example",
            "localhost.evil.example",
            "xdot.local",
            "dot.localx",
            "1.2.3.4.evil.example",
            "192.168.1.20.nip.io",
            "",
            " ",
            "dot:notaport",
            "dot:99999999",
            "a b",
            "dot/evil",
            "[::1",
            "::1",
            "evil@dot",
        ] {
            assert!(!ours(h), "{h:?}");
        }
    }

    #[test]
    fn the_public_address_names_one_more_host() {
        let yes = |h: &str, url: &str| host_is_ours(h, url, Some("dot"));
        assert!(yes("portal.example.net:3335", "https://portal.example.net"));
        assert!(yes("portal.example.net", "http://portal.example.net:3335/"));
        assert!(yes("clawdot.local:3335", "http://clawdot.local:3335"));
        assert!(!yes("other.example.net", "http://portal.example.net"));
        assert!(!yes("portal.example.net", ""));
        assert!(!yes("portal.example.net", "not a url"));
        assert_eq!(
            host_of_url("http://user:pw@Dot.Local:3335/x?y#z").as_deref(),
            Some("dot.local")
        );
    }

    #[test]
    fn the_environment_part_of_the_name_comes_from_the_snapshot() {
        use crate::env::Env;
        assert_eq!(
            name_from_env(&Env::from_pairs([("COMPUTERNAME", " BOX ")])).as_deref(),
            Some("BOX")
        );
        assert_eq!(
            name_from_env(&Env::from_pairs([("HOSTNAME", "a"), ("COMPUTERNAME", "b")])).as_deref(),
            Some("a")
        );
        assert_eq!(name_from_env(&Env::empty()), None);
    }

    #[test]
    fn a_full_machine_name_answers_to_itself_its_first_label_and_dot_local() {
        let yes = |h: &str| host_is_ours(h, "", Some("Dot.Example.Lan"));
        assert!(yes("dot.example.lan"));
        assert!(yes("dot"));
        assert!(yes("dot.local"));
        assert!(!yes("example.lan"));
        assert!(!host_is_ours("dot", "", None), "no machine name, no match");
    }

    // REQ-CFG-016: the printed name is the first label plus .local, which the portal also answers to.
    #[test]
    fn the_printed_local_name_is_the_first_label_or_the_default() {
        assert_eq!(local_name(Some("Dot.Example.Lan")), "dot.local");
        assert_eq!(local_name(Some("kitchen-pi")), "kitchen-pi.local");
        assert!(host_is_ours(
            &local_name(Some("Dot.Example.Lan")),
            "",
            Some("Dot.Example.Lan")
        ));
        for unusable in [
            None,
            Some(""),
            Some("."),
            Some("192.168.1.5"),
            Some("a b"),
            Some("my_box"),
        ] {
            assert_eq!(local_name(unusable), "clawdot.local", "{unusable:?}");
        }
    }
}
