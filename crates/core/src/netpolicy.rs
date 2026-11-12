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
    match h.parse::<IpAddr>() {
        Ok(IpAddr::V4(a)) => a.is_loopback() || a.is_private() || a.is_link_local(),
        Ok(IpAddr::V6(a)) => a.is_loopback() || (a.segments()[0] & 0xfe00) == 0xfc00,
        Err(_) => false,
    }
}

/// Whether talking to `endpoint` would send plain text beyond the private network: no TLS and a host that is not
/// local. A client that carries a key, audio or a conversation refuses such an endpoint.
#[must_use]
pub fn crosses_internet_unencrypted(endpoint: &Endpoint) -> bool {
    !endpoint.tls && !is_private_host(&endpoint.host)
}

#[cfg(test)]
mod tests {
    use super::*;

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
