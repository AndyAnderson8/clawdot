//! The hive, milestone H0 (docs/research/14-hive.md, docs/guides/hive.md): a device identity and discovery of
//! other ClawDot speakers on the home network. Optional (`hive` cargo feature, off by default) and silent
//! unless started with `--hive-discovery`. There is no pairing, no data sharing and no trust in anything heard
//! on the network yet: the peer list is advisory.

mod avahi;
mod discovery;
mod dns;
pub mod identity;

pub use avahi::{Announcer, AvahiFile, DEFAULT_FILE as AVAHI_FILE};
pub use discovery::{Config as DiscoveryConfig, Discovery, MAX_PEERS, Peer, bind_multicast};
pub use dns::Advert;
pub use identity::Identity;

use serde_json::{Value, json};
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Instant;

/// How this speaker is made findable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// Discovery is off; the identity still exists.
    Off,
    /// ClawDot runs its own mDNS responder on UDP 5353.
    Native,
    /// The system's Avahi announces (service file at this path); ClawDot only browses.
    Avahi(PathBuf),
}

impl Mode {
    /// Parse the flag value: `off`, `native`, `avahi` or `avahi:/path/to/file.service`.
    ///
    /// # Errors
    /// A message naming the accepted values.
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "off" => Ok(Self::Off),
            "native" => Ok(Self::Native),
            "avahi" => Ok(Self::Avahi(PathBuf::from(AVAHI_FILE))),
            _ => s
                .strip_prefix("avahi:")
                .filter(|p| !p.is_empty())
                .map(|p| Self::Avahi(PathBuf::from(p)))
                .ok_or_else(|| "expected off, native, avahi or avahi:PATH".to_owned()),
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Native => "native",
            Self::Avahi(_) => "avahi",
        }
    }
}

/// What `main` hands over at start-up.
#[derive(Debug, Clone)]
pub struct Options {
    /// Discovery mode.
    pub mode: Mode,
    /// The portal port other speakers should be told about.
    pub port: u16,
    /// Address for the A record, when the listen address is a specific one.
    pub addr: Option<Ipv4Addr>,
    /// Put the device name in the announcement. Off by default: the name is whatever the owner typed.
    pub publish_name: bool,
    /// The device name, used only when `publish_name` is on.
    pub name: String,
}

/// This speaker's hive state.
#[derive(Debug)]
pub struct Hive {
    identity: Identity,
    mode: Mode,
    port: u16,
    discovery: Option<Discovery>,
}

static HIVE: OnceLock<Arc<Hive>> = OnceLock::new();
// Keeps the announcer alive for the life of the process.
static AVAHI: Mutex<Option<AvahiFile>> = Mutex::new(None);

/// The advertisement for `id`. The TXT set is exactly `v`, `id`, `p`, plus `n` only if the owner opted in.
#[must_use]
pub fn advert_for(id: &str, port: u16, name: Option<&str>, addr: Option<Ipv4Addr>) -> Advert {
    let label = format!("clawdot-{}", id.get(..12).unwrap_or(id));
    let mut txt = vec![
        ("v".to_owned(), "1".to_owned()),
        ("id".to_owned(), id.to_owned()),
        ("p".to_owned(), port.to_string()),
    ];
    if let Some(n) = name {
        // One TXT string is at most 255 bytes; the setting is capped at 40 characters already.
        txt.push(("n".to_owned(), n.chars().take(40).collect()));
    }
    Advert {
        instance: label.clone(),
        host: label,
        port,
        txt,
        addr,
    }
}

/// Open the identity in `dir` and, if `opts.mode` asks for it, start discovery. Call once, inside the runtime.
///
/// # Errors
/// A plain message when the identity cannot be opened or the network socket cannot be set up. A failed
/// socket is not fatal to the caller: the identity is still registered so the API works.
pub fn start(dir: &Path, opts: &Options) -> Result<(), String> {
    let identity = Identity::open(dir).map_err(|e| format!("hive identity: {e}"))?;
    let name = opts.publish_name.then_some(opts.name.as_str());
    let advert = advert_for(identity.id(), opts.port, name, opts.addr);
    let mut problem = None;
    let discovery = match &opts.mode {
        Mode::Off => None,
        Mode::Native => match bind_multicast() {
            Ok(sock) => Some(Discovery::start(
                sock,
                DiscoveryConfig::multicast(Some(advert), Some(identity.id().to_owned())),
            )),
            Err(e) => {
                problem = Some(format!("cannot open the mDNS socket: {e}"));
                None
            }
        },
        Mode::Avahi(path) => {
            let mut file = AvahiFile::new(path);
            if let Err(e) = file.announce(&advert) {
                problem = Some(format!("cannot write {}: {e}", path.display()));
            } else {
                *AVAHI.lock().unwrap_or_else(PoisonError::into_inner) = Some(file);
            }
            // Browsing shares the port with Avahi. If that is refused, the list stays empty.
            match bind_multicast() {
                Ok(sock) => Some(Discovery::start(
                    sock,
                    DiscoveryConfig::multicast(None, Some(identity.id().to_owned())),
                )),
                Err(e) => {
                    problem.get_or_insert(format!("cannot listen for other speakers: {e}"));
                    None
                }
            }
        }
    };
    let hive = Hive {
        identity,
        mode: opts.mode.clone(),
        port: opts.port,
        discovery,
    };
    let _ = HIVE.set(Arc::new(hive));
    problem.map_or(Ok(()), Err)
}

/// The body of `GET /api/hive/peers`, or `None` when the hive was not started. No secret is in it: the
/// private key never leaves `identity`, and the public key and id are public by design.
#[must_use]
pub fn peers_json(device_name: &str) -> Option<Value> {
    let h = HIVE.get()?;
    let now = Instant::now();
    let peers: Vec<Value> = h
        .discovery
        .as_ref()
        .map(Discovery::peers)
        .unwrap_or_default()
        .iter()
        .map(|p| {
            json!({
                "id": p.id,
                "name": p.name,
                "address": p.address.to_string(),
                "port": p.port,
                "last_seen_secs": now.saturating_duration_since(p.last_seen).as_secs(),
            })
        })
        .collect();
    Some(json!({
        "self": {
            "id": h.identity.id(),
            "name": device_name,
            "public_key": h.identity.public_key_hex(),
            "port": h.port,
        },
        "discovery": h.mode.label(),
        "peers": peers,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_parse() {
        assert_eq!(Mode::parse("off"), Ok(Mode::Off));
        assert_eq!(Mode::parse("native"), Ok(Mode::Native));
        assert_eq!(Mode::parse("avahi"), Ok(Mode::Avahi(AVAHI_FILE.into())));
        assert_eq!(
            Mode::parse("avahi:/x/y.service"),
            Ok(Mode::Avahi("/x/y.service".into()))
        );
        assert!(Mode::parse("avahi:").is_err());
        assert!(Mode::parse("on").is_err());
    }

    #[test]
    fn the_advert_publishes_only_the_agreed_set() {
        let id = "ab".repeat(32);
        let keys = |a: &Advert| a.txt.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>();
        let a = advert_for(&id, 3335, None, None);
        assert_eq!(keys(&a), ["v", "id", "p"]);
        assert_eq!(a.instance, "clawdot-abababababab");
        assert_eq!(
            keys(&advert_for(&id, 3335, Some("Den"), None)),
            ["v", "id", "p", "n"]
        );
    }
}
