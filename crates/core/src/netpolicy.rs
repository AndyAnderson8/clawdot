//! Where plain `http://` may go (REQ-SEC-001, REQ-SEC-002).
//!
//! Audio, API keys and conversations must not cross the internet unencrypted, so the clients that carry them
//! (the language model, speech to text and text to speech, the model and voice lists, realtime voice) accept
//! plain `http://` only for this machine and the private network, such as a self-hosted Whisper server or a
//! local Ollama. This is the one place that says which hosts those are. Each client words its own refusal,
//! because what must not travel (audio, a key, a conversation) differs, but the decision is
//! [`crosses_internet_unencrypted`] for all of them.
//!
//! Two outbound clients deliberately follow a different rule: the updater is stricter (https, or loopback for
//! its tests) and the MCP and HTTP-tool clients are looser (the administrator names the server and the tool).

use crate::transport::Endpoint;
use std::net::IpAddr;

/// Whether plain HTTP to this host stays on the local machine or private network.
#[must_use]
pub fn is_private_host(host: &str) -> bool {
    let h = host.trim_matches(['[', ']']);
    if h.eq_ignore_ascii_case("localhost") || h.to_ascii_lowercase().ends_with(".local") {
        return true;
    }
    h.parse::<IpAddr>().is_ok_and(is_private_ip)
}

/// Whether an address is loopback, private (RFC 1918, or unique-local for IPv6) or IPv4 link-local: the
/// addresses that plain HTTP may go to, and the only ones local-network discovery listens to.
#[must_use]
pub fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => a.is_loopback() || a.is_private() || a.is_link_local(),
        IpAddr::V6(a) => a.is_loopback() || (a.segments()[0] & 0xfe00) == 0xfc00,
    }
}

/// Whether talking to `endpoint` would send plain text beyond the private network: no TLS and a host that is not
/// local. A client that carries a key, audio or a conversation refuses such an endpoint.
#[must_use]
pub fn crosses_internet_unencrypted(endpoint: &Endpoint) -> bool {
    !endpoint.tls && !is_private_host(&endpoint.host)
}

/// The refusal for a realtime voice address that would send the key and the voice over the internet in plain
/// text. Used when the address is saved, when the configuration file is read, and when a session connects.
pub const REALTIME_PLAIN_REFUSAL: &str =
    "the key and your voice must not cross the internet unencrypted; use wss://";

/// Check a realtime voice address against the rule of [`crosses_internet_unencrypted`], whether or not the build
/// can open a realtime session. Only `ws://` can fail: `wss://` is encrypted, and an address with another start
/// is the caller's to refuse.
///
/// # Errors
/// [`REALTIME_PLAIN_REFUSAL`] for plain `ws://` to a host beyond the private network.
pub fn check_realtime_url(url: &str) -> Result<(), String> {
    let Some(rest) = url.strip_prefix("ws://") else {
        return Ok(());
    };
    match Endpoint::parse(&format!("http://{rest}")) {
        Ok(ep) if crosses_internet_unencrypted(&ep) => Err(REALTIME_PLAIN_REFUSAL.into()),
        Ok(_) => Ok(()),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn req_sec_001_plain_ws_beyond_the_private_network_is_refused_at_check_time() {
        assert!(check_realtime_url("wss://api.openai.com/v1/realtime").is_ok());
        assert!(check_realtime_url("ws://192.168.1.9:8000/v1/realtime").is_ok());
        assert!(check_realtime_url("ws://localhost:8000").is_ok());
        assert_eq!(
            check_realtime_url("ws://api.openai.com/v1/realtime").unwrap_err(),
            REALTIME_PLAIN_REFUSAL
        );
        assert!(check_realtime_url("ws://8.8.8.8:9000").is_err());
    }

    #[test]
    fn only_plain_http_beyond_the_private_network_is_refused() {
        let refused = |u: &str| crosses_internet_unencrypted(&Endpoint::parse(u).unwrap());
        assert!(refused("http://api.openai.com/v1"));
        assert!(refused("http://8.8.8.8:9000"));
        assert!(!refused("https://api.openai.com/v1"));
        assert!(!refused("http://localhost:11434/v1"));
        assert!(!refused("http://192.168.1.9:9000/v1"));
        assert!(!refused("http://dot.local/v1"));
    }
}
