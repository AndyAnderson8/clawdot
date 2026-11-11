//! Announcing through the system's Avahi instead of opening UDP 5353 a second time.
//!
//! On the TECHO5 Dot image `avahi-daemon` already owns the mDNS port and answers for `linux.local`. Two
//! responders on one host answer the same question twice and fight over names, so ClawDot there describes its
//! service in a file under `/etc/avahi/services/`, which the daemon watches and publishes itself. Browsing still
//! needs to hear multicast; that uses a separate socket that shares the port (see `discovery::bind_multicast`).
//!
//! Writing under `/etc` needs the rights the Dot's service already has (root); elsewhere pass another path.

use super::dns::Advert;
use std::io;
use std::path::PathBuf;

/// Something that makes this speaker findable. The native responder is built into the discovery loop because
/// it shares the loop's socket; this trait is the seam for responders that live outside the process.
pub trait Announcer: Send {
    /// Start or update the announcement.
    ///
    /// # Errors
    /// When the announcement cannot be made.
    fn announce(&mut self, advert: &Advert) -> io::Result<()>;
    /// Take the announcement down.
    ///
    /// # Errors
    /// When the announcement cannot be removed.
    fn withdraw(&mut self) -> io::Result<()>;
}

/// Where Avahi looks for static service definitions.
pub const DEFAULT_FILE: &str = "/etc/avahi/services/clawdot.service";

/// Announces by writing an Avahi service file.
#[derive(Debug)]
pub struct AvahiFile {
    path: PathBuf,
}

impl AvahiFile {
    /// An announcer that writes `path`.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

fn esc(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '&' => "&amp;".to_owned(),
            '<' => "&lt;".to_owned(),
            '>' => "&gt;".to_owned(),
            '"' => "&quot;".to_owned(),
            '\'' => "&apos;".to_owned(),
            c if c.is_control() => String::new(),
            c => c.to_string(),
        })
        .collect()
}

/// The service-group document for `advert`.
#[must_use]
pub fn service_xml(advert: &Advert) -> String {
    use std::fmt::Write;
    let mut x = String::from(
        "<?xml version=\"1.0\" standalone='no'?>\n<!DOCTYPE service-group SYSTEM \"avahi-service.dtd\">\n<service-group>\n",
    );
    let _ = writeln!(x, "  <name>{}</name>", esc(&advert.instance));
    let _ = writeln!(x, "  <service>\n    <type>_clawdot._tcp</type>");
    let _ = writeln!(x, "    <port>{}</port>", advert.port);
    for (k, v) in &advert.txt {
        let _ = writeln!(x, "    <txt-record>{}={}</txt-record>", esc(k), esc(v));
    }
    x.push_str("  </service>\n</service-group>\n");
    x
}

impl Announcer for AvahiFile {
    fn announce(&mut self, advert: &Advert) -> io::Result<()> {
        // Written beside the target and renamed, so the daemon never reads half a file.
        let tmp = self.path.with_extension("service.tmp");
        std::fs::write(&tmp, service_xml(advert))?;
        std::fs::rename(&tmp, &self.path)
    }

    fn withdraw(&mut self) -> io::Result<()> {
        match std::fs::remove_file(&self.path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::testutil::TempDir;

    fn advert() -> Advert {
        Advert {
            instance: "clawdot-0123456789ab".into(),
            host: "x".into(),
            port: 3335,
            txt: vec![("v".into(), "1".into()), ("n".into(), "A&B <\"x\">".into())],
            addr: None,
        }
    }

    #[test]
    fn the_xml_lists_type_port_and_escaped_txt() {
        let x = service_xml(&advert());
        assert!(x.contains("<type>_clawdot._tcp</type>"));
        assert!(x.contains("<port>3335</port>"));
        assert!(x.contains("<txt-record>v=1</txt-record>"));
        assert!(x.contains("n=A&amp;B &lt;&quot;x&quot;&gt;"));
    }

    #[test]
    fn announce_writes_and_withdraw_removes() {
        let dir = TempDir::new("hive-avahi");
        let mut a = AvahiFile::new(dir.0.join("clawdot.service"));
        a.announce(&advert()).unwrap();
        assert!(dir.0.join("clawdot.service").exists());
        assert!(!dir.0.join("clawdot.service.tmp").exists());
        a.withdraw().unwrap();
        assert!(!dir.0.join("clawdot.service").exists());
        a.withdraw().unwrap();
    }
}
