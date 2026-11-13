//! A speaker's identity: an Ed25519 key pair made on first start and kept with the other private data files
//! (REQ-HIV-002, REQ-HIV-010). The device id is the SHA-256 of the public key, in lower-case hex.
//!
//! The private key is stored as a PKCS#8 document in `hive-identity.json` (mode 0600 in a 0700 directory, the same
//! protection as `secrets.json`). It is never returned by any API, never logged, and never part of a settings export.
//! A file that exists but cannot be read is an error: silently making a new key would change the speaker's id.

use crate::store::{JsonStore, StoreError};
use ring::rand::SystemRandom;
use ring::signature::{ED25519, Ed25519KeyPair, KeyPair, UnparsedPublicKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;
use std::path::Path;

/// Schema version of `hive-identity.json`.
pub const SCHEMA: u32 = 1;

/// File name inside the data directory.
pub const FILE: &str = "hive-identity.json";

#[derive(Default, Serialize, Deserialize)]
struct Stored {
    /// PKCS#8 private key, hex. Empty until first start.
    pkcs8: String,
}

/// Why the identity could not be opened.
#[derive(Debug)]
pub enum IdentityError {
    /// The file could not be read or written.
    Store(StoreError),
    /// The file holds something that is not an Ed25519 key.
    BadKey,
    /// The system could not make random bytes.
    Random,
}

impl fmt::Display for IdentityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(e) => write!(f, "{e}"),
            Self::BadKey => f.write_str("the device identity file does not hold a valid key"),
            Self::Random => f.write_str("could not make a device identity key"),
        }
    }
}

impl std::error::Error for IdentityError {}

impl From<StoreError> for IdentityError {
    fn from(e: StoreError) -> Self {
        Self::Store(e)
    }
}

/// This speaker's key pair and the id derived from it.
pub struct Identity {
    key: Ed25519KeyPair,
    id: String,
}

impl fmt::Debug for Identity {
    // The private key must never reach a log line.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Identity").field("id", &self.id).finish()
    }
}

impl Identity {
    /// Load the identity from `dir`, making and saving one on first start.
    ///
    /// # Errors
    /// [`IdentityError`] when the file is unreadable, corrupt, or cannot be written.
    pub fn open(dir: &Path) -> Result<Self, IdentityError> {
        let store = JsonStore::new(dir.join(FILE), SCHEMA);
        let stored: Stored = store.load()?;
        if stored.pkcs8.is_empty() {
            // A file that is there with no key in it was damaged or edited: a new key would change the
            // speaker's id without a word, so it is the same error as a file that cannot be read.
            if dir.join(FILE).exists() {
                return Err(IdentityError::BadKey);
            }
            let doc = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new())
                .map_err(|_| IdentityError::Random)?;
            let key =
                Ed25519KeyPair::from_pkcs8(doc.as_ref()).map_err(|_| IdentityError::BadKey)?;
            store.save(&Stored {
                pkcs8: to_hex(doc.as_ref()),
            })?;
            return Ok(Self::from_key(key));
        }
        let bytes = from_hex(&stored.pkcs8).ok_or(IdentityError::BadKey)?;
        let key = Ed25519KeyPair::from_pkcs8(&bytes).map_err(|_| IdentityError::BadKey)?;
        Ok(Self::from_key(key))
    }

    fn from_key(key: Ed25519KeyPair) -> Self {
        let id = device_id(key.public_key().as_ref());
        Self { key, id }
    }

    /// The device id: hex SHA-256 of the public key (64 characters).
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The public key, hex (64 characters). Public by design.
    #[must_use]
    pub fn public_key_hex(&self) -> String {
        to_hex(self.key.public_key().as_ref())
    }

    /// Sign `msg`. Not used by H0 itself; later milestones sign records with it.
    #[must_use]
    pub fn sign(&self, msg: &[u8]) -> Vec<u8> {
        self.key.sign(msg).as_ref().to_vec()
    }
}

/// The device id for a public key.
#[must_use]
pub fn device_id(public_key: &[u8]) -> String {
    to_hex(&Sha256::digest(public_key))
}

/// Whether `sig` is a valid Ed25519 signature of `msg` by `public_key`.
#[must_use]
pub fn verify(public_key: &[u8], msg: &[u8], sig: &[u8]) -> bool {
    UnparsedPublicKey::new(&ED25519, public_key)
        .verify(msg, sig)
        .is_ok()
}

fn to_hex(b: &[u8]) -> String {
    use fmt::Write;
    b.iter()
        .fold(String::with_capacity(b.len() * 2), |mut s, x| {
            let _ = write!(s, "{x:02x}");
            s
        })
}

fn from_hex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 || !s.is_ascii() {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

/// Whether `s` looks like a device id (64 lower-case hex characters).
#[must_use]
pub fn is_device_id(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::testutil::TempDir;

    #[test]
    fn identity_is_stable_across_restarts() {
        let dir = TempDir::new("hive-id");
        let a = Identity::open(&dir.0).unwrap();
        let b = Identity::open(&dir.0).unwrap();
        assert_eq!(a.id(), b.id());
        assert_eq!(a.public_key_hex(), b.public_key_hex());
        assert!(is_device_id(a.id()));
        let other = TempDir::new("hive-id2");
        assert_ne!(Identity::open(&other.0).unwrap().id(), a.id());
    }

    #[test]
    fn id_is_the_hash_of_the_public_key() {
        let dir = TempDir::new("hive-hash");
        let a = Identity::open(&dir.0).unwrap();
        let pk = from_hex(&a.public_key_hex()).unwrap();
        assert_eq!(a.id(), device_id(&pk));
    }

    #[test]
    fn signatures_verify_and_tampering_fails() {
        let dir = TempDir::new("hive-sig");
        let a = Identity::open(&dir.0).unwrap();
        let pk = from_hex(&a.public_key_hex()).unwrap();
        let sig = a.sign(b"hello");
        assert!(verify(&pk, b"hello", &sig));
        assert!(!verify(&pk, b"hellp", &sig));
    }

    #[cfg(unix)]
    #[test]
    fn the_key_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new("hive-perm");
        let data = dir.0.join("data");
        Identity::open(&data).unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&data.join(FILE)), 0o600);
        assert_eq!(mode(&data), 0o700);
    }

    #[test]
    fn a_damaged_file_is_an_error_not_a_new_identity() {
        let dir = TempDir::new("hive-bad");
        let a = Identity::open(&dir.0).unwrap();
        std::fs::write(dir.0.join(FILE), r#"{"schema":1,"data":{"pkcs8":"zz"}}"#).unwrap();
        assert!(matches!(Identity::open(&dir.0), Err(IdentityError::BadKey)));
        std::fs::write(dir.0.join(FILE), "not json").unwrap();
        assert!(Identity::open(&dir.0).is_err());
        // A file with an empty key is damaged too, not a first start.
        std::fs::write(dir.0.join(FILE), r#"{"schema":1,"data":{"pkcs8":""}}"#).unwrap();
        assert!(matches!(Identity::open(&dir.0), Err(IdentityError::BadKey)));
        assert!(!a.id().is_empty());
    }

    #[test]
    fn debug_does_not_show_the_key() {
        let dir = TempDir::new("hive-dbg");
        let a = Identity::open(&dir.0).unwrap();
        let s = format!("{a:?}");
        assert!(!s.contains("pkcs8") && !s.contains("key"));
    }
}
