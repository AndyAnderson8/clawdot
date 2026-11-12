//! Configuration for the optional ESPHome voice-satellite client (spec 0004, REQ-ESP-002).
//!
//! Only built with the `esphome-satellite` feature, so the default binary carries no code for it (a default
//! build keeps the section unparsed and refuses it unless it is empty; see `config.rs`). The Noise key is never
//! in the file: `psk_env` names the environment variable that holds it.

use serde::Deserialize;

/// Most devices one core dials.
pub const MAX_DEVICES: usize = 4;
/// ESPHome's native API port.
pub const DEFAULT_PORT: u16 = 6053;

/// How a spoken reply to a person reaches the device.
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ReplyMode {
    /// The reply is collected and delivered once it is complete (the behaviour before sentence-by-sentence
    /// replies existed).
    #[default]
    Whole,
    /// The reply is delivered while it is still being made, starting with its first sentence.
    Streamed,
}

/// One ESPHome voice satellite to dial.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EsphomeDevice {
    /// A short name, used as the satellite's device id (letters, digits, `.`, `_`, `-`).
    pub name: String,
    /// `host` or `host:port` (port 6053 when omitted).
    pub address: String,
    /// Name of the environment variable holding the base64 Noise key. Plaintext when unset.
    pub psk_env: Option<String>,
    /// `"whole"` (the default) or `"streamed"`: whether a spoken reply starts on the device with its first
    /// sentence. See `docs/architecture/esphome-satellite.md`.
    #[serde(default)]
    pub reply: ReplyMode,
}

impl EsphomeDevice {
    /// The address with the default port filled in.
    #[must_use]
    pub fn host_port(&self) -> String {
        if split_port(&self.address).1.is_some() {
            self.address.clone()
        } else if self.address.contains(':') && !self.address.starts_with('[') {
            // A bare IPv6 literal.
            format!("[{}]:{DEFAULT_PORT}", self.address)
        } else {
            format!("{}:{DEFAULT_PORT}", self.address)
        }
    }
}

/// `host:port` into its parts; the port is `None` when absent. Bracketed IPv6 is understood.
fn split_port(a: &str) -> (&str, Option<&str>) {
    if let Some(rest) = a.strip_prefix('[') {
        return match rest.split_once("]:") {
            Some((h, p)) => (h, Some(p)),
            None => (rest.trim_end_matches(']'), None),
        };
    }
    if a.matches(':').count() == 1 {
        if let Some((h, p)) = a.split_once(':') {
            return (h, Some(p));
        }
    }
    (a, None)
}

/// The `esphome` section.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EsphomeConfig {
    /// Devices to dial.
    #[serde(default)]
    pub devices: Vec<EsphomeDevice>,
    /// Address the audio file server listens on, for example `0.0.0.0:3337`. Without it, announcements and
    /// URL-based replies cannot be played.
    pub media_listen: Option<String>,
    /// The address devices use to reach the file server, for example `http://192.168.1.10:3337`.
    pub media_url: Option<String>,
}

impl EsphomeConfig {
    /// Whether nothing is configured.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.devices.is_empty() && self.media_listen.is_none() && self.media_url.is_none()
    }

    /// Check the section.
    ///
    /// # Errors
    /// A message naming the problem.
    pub fn validate(&self) -> Result<(), String> {
        if self.is_empty() {
            return Ok(());
        }
        if self.devices.is_empty() {
            return Err("config: `esphome.media_listen` and `media_url` need at least one `esphome.devices` entry".into());
        }
        if self.devices.len() > MAX_DEVICES {
            return Err(format!(
                "config: at most {MAX_DEVICES} `esphome.devices` are supported"
            ));
        }
        let mut seen = std::collections::HashSet::new();
        for d in &self.devices {
            let ok_name = !d.name.is_empty()
                && d.name.len() <= 64
                && d.name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
            if !ok_name {
                return Err(format!(
                    "config: `esphome.devices` name {:?} must be 1 to 64 letters, digits, `.`, `_` or `-`",
                    d.name
                ));
            }
            if !seen.insert(d.name.as_str()) {
                return Err(format!(
                    "config: `esphome.devices` name {:?} is used twice",
                    d.name
                ));
            }
            let (host, port) = split_port(&d.address);
            if host.is_empty()
                || host.len() > 253
                || host
                    .bytes()
                    .any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
            {
                return Err(format!(
                    "config: `esphome.devices` {:?} has a bad `address`",
                    d.name
                ));
            }
            if let Some(p) = port {
                if p.parse::<u16>().ok().filter(|p| *p != 0).is_none() {
                    return Err(format!(
                        "config: `esphome.devices` {:?} has a bad port in `address`",
                        d.name
                    ));
                }
            }
            if let Some(e) = &d.psk_env {
                if e.is_empty() || !e.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
                    return Err(format!(
                        "config: `esphome.devices` {:?} `psk_env` must be an environment variable name",
                        d.name
                    ));
                }
            }
        }
        match (&self.media_listen, &self.media_url) {
            (Some(l), Some(u)) => {
                l.parse::<std::net::SocketAddr>().map_err(|e| {
                    format!(
                        "config: `esphome.media_listen` is not an address like 0.0.0.0:3337: {e}"
                    )
                })?;
                if !u.starts_with("http://")
                    || u.len() > 200
                    || u.bytes().any(|b| b.is_ascii_whitespace())
                {
                    return Err(
                        "config: `esphome.media_url` must be a plain http:// address".into(),
                    );
                }
            }
            (None, None) => {}
            _ => {
                return Err(
                    "config: `esphome.media_listen` and `esphome.media_url` go together".into(),
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> Result<EsphomeConfig, String> {
        let c: EsphomeConfig = serde_json::from_str(json).map_err(|e| e.to_string())?;
        c.validate().map(|()| c)
    }

    #[test]
    fn an_empty_section_is_valid_in_every_build() {
        assert!(parse("{}").unwrap().is_empty());
    }

    #[test]
    fn host_port_fills_in_the_default() {
        let d = |a: &str| EsphomeDevice {
            name: "d".into(),
            address: a.into(),
            ..EsphomeDevice::default()
        };
        assert_eq!(d("dot.local").host_port(), "dot.local:6053");
        assert_eq!(d("10.0.0.2:7000").host_port(), "10.0.0.2:7000");
        assert_eq!(d("fe80::1").host_port(), "[fe80::1]:6053");
        assert_eq!(d("[fe80::1]:99").host_port(), "[fe80::1]:99");
    }

    #[test]
    fn the_reply_mode_defaults_to_whole_and_accepts_streamed() {
        let c = parse(
            r#"{"devices":[{"name":"a","address":"h"},{"name":"b","address":"h","reply":"streamed"},
                           {"name":"c","address":"h","reply":"whole"}]}"#,
        )
        .unwrap();
        let modes: Vec<_> = c.devices.iter().map(|d| d.reply).collect();
        assert_eq!(
            modes,
            [ReplyMode::Whole, ReplyMode::Streamed, ReplyMode::Whole]
        );
    }

    #[test]
    fn a_good_section_parses_and_bad_ones_say_why() {
        let c = parse(
            r#"{"devices":[{"name":"kitchen","address":"dot.local","psk_env":"KITCHEN_PSK"}],
                "media_listen":"0.0.0.0:3337","media_url":"http://192.168.1.10:3337"}"#,
        )
        .unwrap();
        assert_eq!(c.devices[0].host_port(), "dot.local:6053");
        for (text, needle) in [
            (r#"{"devices":[{"name":"","address":"h"}]}"#, "name"),
            (r#"{"devices":[{"name":"a b","address":"h"}]}"#, "name"),
            (
                r#"{"devices":[{"name":"a","address":"h"},{"name":"a","address":"h"}]}"#,
                "twice",
            ),
            (r#"{"devices":[{"name":"a","address":""}]}"#, "address"),
            (r#"{"devices":[{"name":"a","address":"h:0"}]}"#, "port"),
            (r#"{"devices":[{"name":"a","address":"h:99999"}]}"#, "port"),
            (
                r#"{"devices":[{"name":"a","address":"h","psk_env":"A B"}]}"#,
                "psk_env",
            ),
            (r#"{"media_listen":"0.0.0.0:1"}"#, "devices"),
            (
                r#"{"devices":[{"name":"a","address":"h"}],"media_listen":"0.0.0.0:1"}"#,
                "go together",
            ),
            (
                r#"{"devices":[{"name":"a","address":"h"}],"media_listen":"x","media_url":"http://h"}"#,
                "media_listen",
            ),
            (
                r#"{"devices":[{"name":"a","address":"h"}],"media_listen":"0.0.0.0:1","media_url":"https://h"}"#,
                "http://",
            ),
            (
                r#"{"devices":[{"name":"a","address":"h","typo":1}]}"#,
                "typo",
            ),
            (
                r#"{"devices":[{"name":"a","address":"h","reply":"fast"}]}"#,
                "fast",
            ),
            (
                r#"{"devices":[{"name":"a","address":"h","reply":true}]}"#,
                "expected value",
            ),
        ] {
            let e = parse(text).unwrap_err();
            assert!(e.contains(needle), "{text}: {e}");
        }
        let many = (0..5)
            .map(|i| format!(r#"{{"name":"d{i}","address":"h"}}"#))
            .collect::<Vec<_>>()
            .join(",");
        assert!(
            parse(&format!(r#"{{"devices":[{many}]}}"#))
                .unwrap_err()
                .contains("at most")
        );
    }
}
