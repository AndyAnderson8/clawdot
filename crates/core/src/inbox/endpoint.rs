//! Endpoints: named, token-protected event sources (REQ-INB-001, REQ-INB-002, REQ-INB-012, REQ-INB-014).
//!
//! Tokens are shown once at creation and stored only as SHA-256 hashes, so a leaked config or backup
//! does not leak credentials (threat model T3, REQ-CFG-006).

use super::event::{EventKind, Priority};
use crate::sched::Ts;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;

/// Longest endpoint name.
pub const MAX_NAME: usize = 32;
const TOKEN_PREFIX: &str = "cdt_";

/// Why an endpoint operation failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndpointError {
    /// The name is empty, too long, or has characters outside `a-z 0-9 - _`.
    InvalidName,
    /// An endpoint with that name already exists.
    Exists,
    /// No endpoint with that name.
    NotFound,
    /// The operating system could not provide randomness.
    NoRandomness,
}

impl fmt::Display for EndpointError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidName => write!(
                f,
                "endpoint names use lowercase letters, digits, - and _ (max {MAX_NAME})"
            ),
            Self::Exists => write!(f, "an endpoint with that name already exists"),
            Self::NotFound => write!(f, "no such endpoint"),
            Self::NoRandomness => write!(f, "could not generate a secure token"),
        }
    }
}

impl std::error::Error for EndpointError {}

/// Quiet hours in local minutes since midnight. Only urgent events are announced during them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuietHours {
    /// Start, minutes after local midnight (for example 22:00 is 1320).
    pub start: u16,
    /// End, minutes after local midnight. May be earlier than `start` to span midnight.
    pub end: u16,
}

impl QuietHours {
    /// Whether `minute_of_day` falls inside the quiet window.
    #[must_use]
    pub fn contains(&self, minute_of_day: u16) -> bool {
        if self.start <= self.end {
            (self.start..self.end).contains(&minute_of_day)
        } else {
            minute_of_day >= self.start || minute_of_day < self.end
        }
    }
}

/// Per-endpoint filtering and priority rules (REQ-INB-012, REQ-INB-014).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EndpointFilter {
    /// Drop everything from this endpoint.
    pub muted: bool,
    /// Drop events below this priority.
    pub min_priority: Priority,
    /// Drop these kinds.
    pub ignore_kinds: Vec<EventKind>,
    /// Merge repeats from the same session and kind arriving within this many seconds.
    pub coalesce_window_secs: i64,
    /// Announce only urgent events during these hours.
    pub quiet_hours: Option<QuietHours>,
}

impl Default for EndpointFilter {
    fn default() -> Self {
        Self {
            muted: false,
            min_priority: Priority::Low,
            ignore_kinds: Vec::new(),
            coalesce_window_secs: 30,
            quiet_hours: None,
        }
    }
}

/// A named event source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Endpoint {
    /// Unique name, used in the URL.
    pub name: String,
    /// Which adapter maps its payloads (`claude-code`, `generic`, or a custom one).
    pub adapter: String,
    /// Hex SHA-256 of the token. The token itself is never stored.
    pub token_hash: String,
    /// Creation time.
    pub created_at: Ts,
    /// Disabled endpoints reject all requests.
    pub enabled: bool,
    /// Filtering rules.
    #[serde(default)]
    pub filter: EndpointFilter,
}

/// All endpoints.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Endpoints {
    list: Vec<Endpoint>,
}

/// Schema version of the persisted endpoint list.
pub const SCHEMA: u32 = 1;

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME
        && name.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from(DIGITS[usize::from(b >> 4)]));
        s.push(char::from(DIGITS[usize::from(b & 0x0f)]));
    }
    s
}

fn base64url(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..=chunk.len() {
            out.push(char::from(A[((n >> (18 - 6 * i)) & 0x3f) as usize]));
        }
    }
    out
}

/// Hash a token for storage.
#[must_use]
pub fn hash_token(token: &str) -> String {
    hex(&Sha256::digest(token.as_bytes()))
}

/// Compare two equal-length strings without an early exit on the first difference.
fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes()
        .zip(b.bytes())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

/// Whether `token` matches a stored hash.
#[must_use]
pub fn verify_token(token: &str, stored_hash: &str) -> bool {
    constant_time_eq(&hash_token(token), stored_hash)
}

/// A fresh random token: `cdt_` plus 256 bits of URL-safe randomness.
///
/// # Errors
/// [`EndpointError::NoRandomness`] if the OS cannot supply random bytes.
pub fn generate_token() -> Result<String, EndpointError> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| EndpointError::NoRandomness)?;
    Ok(format!("{TOKEN_PREFIX}{}", base64url(&bytes)))
}

impl Endpoints {
    /// Create an endpoint. Returns it with the plaintext token, which is shown once and not kept.
    ///
    /// # Errors
    /// Bad or duplicate name; no randomness.
    pub fn create(
        &mut self,
        name: &str,
        adapter: &str,
        now: Ts,
    ) -> Result<(Endpoint, String), EndpointError> {
        if !valid_name(name) {
            return Err(EndpointError::InvalidName);
        }
        if self.get(name).is_some() {
            return Err(EndpointError::Exists);
        }
        let token = generate_token()?;
        let ep = Endpoint {
            name: name.to_owned(),
            adapter: adapter.to_owned(),
            token_hash: hash_token(&token),
            created_at: now,
            enabled: true,
            filter: EndpointFilter::default(),
        };
        self.list.push(ep.clone());
        Ok((ep, token))
    }

    /// Replace an endpoint's token; the old one stops working immediately (REQ-INB-002).
    ///
    /// # Errors
    /// [`EndpointError::NotFound`]; no randomness.
    pub fn rotate(&mut self, name: &str) -> Result<String, EndpointError> {
        let token = generate_token()?;
        let ep = self
            .list
            .iter_mut()
            .find(|e| e.name == name)
            .ok_or(EndpointError::NotFound)?;
        ep.token_hash = hash_token(&token);
        Ok(token)
    }

    /// Delete an endpoint.
    ///
    /// # Errors
    /// [`EndpointError::NotFound`].
    pub fn revoke(&mut self, name: &str) -> Result<(), EndpointError> {
        let before = self.list.len();
        self.list.retain(|e| e.name != name);
        if self.list.len() == before {
            Err(EndpointError::NotFound)
        } else {
            Ok(())
        }
    }

    /// Look an endpoint up by name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Endpoint> {
        self.list.iter().find(|e| e.name == name)
    }

    /// Mutable access, for changing filters.
    pub fn get_mut(&mut self, name: &str) -> Option<&mut Endpoint> {
        self.list.iter_mut().find(|e| e.name == name)
    }

    /// All endpoints.
    #[must_use]
    pub fn all(&self) -> &[Endpoint] {
        &self.list
    }

    /// The endpoint named `name` if it is enabled and `token` is right. A missing endpoint and a wrong
    /// token are indistinguishable to the caller, and both cost a hash.
    #[must_use]
    pub fn authenticate(&self, name: &str, token: &str) -> Option<&Endpoint> {
        let presented = hash_token(token);
        match self.get(name) {
            Some(ep) if ep.enabled && constant_time_eq(&presented, &ep.token_hash) => Some(ep),
            Some(_) => None,
            None => {
                // Burn comparable work so timing does not reveal which endpoints exist.
                let _ = constant_time_eq(&presented, &presented);
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_long_prefixed_and_unique() {
        let a = generate_token().unwrap();
        let b = generate_token().unwrap();
        assert!(a.starts_with("cdt_") && a.len() >= 4 + 43);
        assert_ne!(a, b);
        assert!(
            a.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        );
    }

    #[test]
    fn hashing_matches_a_known_sha256_vector() {
        assert_eq!(
            hash_token("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(base64url(&[0xfb, 0xff, 0xfe]), "-__-");
    }

    #[test]
    fn create_authenticate_rotate_revoke() {
        let mut eps = Endpoints::default();
        let (ep, token) = eps.create("claude-code", "claude-code", 100).unwrap();
        assert!(ep.token_hash != token, "the token is stored hashed");
        assert!(eps.authenticate("claude-code", &token).is_some());
        assert!(eps.authenticate("claude-code", "cdt_wrong").is_none());
        assert!(eps.authenticate("nope", &token).is_none());

        let new_token = eps.rotate("claude-code").unwrap();
        assert!(
            eps.authenticate("claude-code", &token).is_none(),
            "old token is dead"
        );
        assert!(eps.authenticate("claude-code", &new_token).is_some());

        eps.get_mut("claude-code").unwrap().enabled = false;
        assert!(
            eps.authenticate("claude-code", &new_token).is_none(),
            "disabled endpoints reject all"
        );

        eps.revoke("claude-code").unwrap();
        assert_eq!(eps.revoke("claude-code"), Err(EndpointError::NotFound));
        assert!(eps.all().is_empty());
    }

    #[test]
    fn names_are_validated_and_unique() {
        let mut eps = Endpoints::default();
        for bad in [
            "",
            "Has Caps",
            "has space",
            "-leading",
            "a/b",
            "../x",
            &"x".repeat(33),
        ] {
            assert_eq!(
                eps.create(bad, "generic", 0).unwrap_err(),
                EndpointError::InvalidName,
                "{bad:?}"
            );
        }
        eps.create("ci_main-1", "generic", 0).unwrap();
        assert_eq!(
            eps.create("ci_main-1", "generic", 0).unwrap_err(),
            EndpointError::Exists
        );
    }

    #[test]
    fn quiet_hours_can_span_midnight() {
        let night = QuietHours {
            start: 22 * 60,
            end: 7 * 60,
        };
        assert!(night.contains(23 * 60) && night.contains(60) && !night.contains(12 * 60));
        let nap = QuietHours {
            start: 13 * 60,
            end: 15 * 60,
        };
        assert!(nap.contains(14 * 60) && !nap.contains(15 * 60));
    }

    #[test]
    fn serialisation_never_contains_the_plaintext_token() {
        let mut eps = Endpoints::default();
        let (_, token) = eps.create("x", "generic", 0).unwrap();
        let json = serde_json::to_string(&eps).unwrap();
        assert!(!json.contains(&token));
    }
}
