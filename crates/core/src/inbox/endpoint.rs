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
/// Longest spoken name, in characters.
pub const MAX_SPOKEN_NAME: usize = 40;
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
    /// The spoken name is too long or has characters that are not plain words.
    InvalidSpokenName,
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
            Self::InvalidSpokenName => write!(
                f,
                "the spoken name is plain words of up to {MAX_SPOKEN_NAME} characters: letters, digits, spaces, - _ ' and ."
            ),
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
    /// Stable identity: 32 hex digits chosen at random when the endpoint is created and never changed or reused.
    /// Everything the program holds on an endpoint's behalf (events, replies waiting for its bridge, a question
    /// being asked, its rate-limit state) is keyed by this and not by the name, so deleting an endpoint and
    /// creating another with the same name cannot hand the new one the old one's things. Empty only in a file
    /// saved before ids existed, until [`Endpoints::assign_missing_ids`] runs at load.
    #[serde(default)]
    pub id: String,
    /// Unique name among live endpoints, used in the URL and spoken as the origin unless a spoken name is set.
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
    /// What the speaker says as the origin of this endpoint's messages ("From the kitchen: ..."). Set only by
    /// the administrator; when absent the endpoint's own name is spoken. A sender can never choose it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spoken_name: Option<String>,
}

impl Endpoint {
    /// The words spoken as the origin of this endpoint's messages: its spoken name, or its name.
    #[must_use]
    pub fn origin(&self) -> &str {
        self.spoken_name.as_deref().unwrap_or(&self.name)
    }
}

/// Check a spoken name: one line of plain words, at most [`MAX_SPOKEN_NAME`] characters (letters, digits, single
/// spaces, `-`, `_`, `'` and `.`). No control characters, markup, brackets, quotes or links, so the name is safe to
/// say and to place in a prompt. Blank means "none" (`Ok(None)`).
///
/// # Errors
/// [`EndpointError::InvalidSpokenName`].
pub fn clean_spoken_name(raw: &str) -> Result<Option<String>, EndpointError> {
    let t = raw.trim();
    if t.is_empty() {
        return Ok(None);
    }
    let plain = |c: char| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '\'' | '.');
    if t.chars().count() > MAX_SPOKEN_NAME
        || !t.chars().all(plain)
        || t.contains("  ")
        || !t.starts_with(char::is_alphanumeric)
    {
        return Err(EndpointError::InvalidSpokenName);
    }
    Ok(Some(t.to_owned()))
}

/// All endpoints.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Endpoints {
    list: Vec<Endpoint>,
}

/// Schema version of the persisted endpoint list.
pub const SCHEMA: u32 = 1;

/// An endpoint name: lowercase letters and digits to start, then also `-` and `_`, 1 to [`MAX_NAME`] bytes. The one
/// definition: the route matcher uses it for `{endpoint}` segments too.
pub(crate) fn valid_name(name: &str) -> bool {
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

/// A fresh random endpoint id: 128 bits as 32 lowercase hex digits.
///
/// # Errors
/// [`EndpointError::NoRandomness`] if the OS cannot supply random bytes.
pub fn generate_id() -> Result<String, EndpointError> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| EndpointError::NoRandomness)?;
    Ok(hex(&bytes))
}

/// Hash a token for storage.
#[must_use]
pub fn hash_token(token: &str) -> String {
    hex(&Sha256::digest(token.as_bytes()))
}

/// Compare two equal-length strings without an early exit on the first difference.
pub(crate) fn constant_time_eq(a: &str, b: &str) -> bool {
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
        self.create_with(name, adapter, None, now)
    }

    /// [`Endpoints::create`] with a spoken name (checked by [`clean_spoken_name`]), all or nothing.
    ///
    /// # Errors
    /// As [`Endpoints::create`], and [`EndpointError::InvalidSpokenName`].
    pub fn create_with(
        &mut self,
        name: &str,
        adapter: &str,
        spoken_name: Option<&str>,
        now: Ts,
    ) -> Result<(Endpoint, String), EndpointError> {
        let spoken_name = spoken_name.map(clean_spoken_name).transpose()?.flatten();
        if !valid_name(name) {
            return Err(EndpointError::InvalidName);
        }
        if self.get(name).is_some() {
            return Err(EndpointError::Exists);
        }
        let token = generate_token()?;
        let ep = Endpoint {
            id: generate_id()?,
            name: name.to_owned(),
            adapter: adapter.to_owned(),
            token_hash: hash_token(&token),
            created_at: now,
            enabled: true,
            filter: EndpointFilter::default(),
            spoken_name,
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

    /// Set (or, with a blank value, clear) an endpoint's spoken name.
    ///
    /// # Errors
    /// [`EndpointError::NotFound`]; [`EndpointError::InvalidSpokenName`].
    pub fn set_spoken_name(&mut self, name: &str, spoken: &str) -> Result<(), EndpointError> {
        let spoken = clean_spoken_name(spoken)?;
        self.get_mut(name)
            .ok_or(EndpointError::NotFound)?
            .spoken_name = spoken;
        Ok(())
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

    /// Give an id to every endpoint that has none (a file saved before ids existed). Returns whether any was
    /// given, so the caller knows the list must be saved: an id that is not on disk would be a different one
    /// after the next restart, and what was keyed by it meanwhile would be lost.
    ///
    /// # Errors
    /// [`EndpointError::NoRandomness`]. Nothing is changed then.
    pub fn assign_missing_ids(&mut self) -> Result<bool, EndpointError> {
        let ids: Vec<String> = self
            .list
            .iter()
            .filter(|e| e.id.is_empty())
            .map(|_| generate_id())
            .collect::<Result<_, _>>()?;
        let changed = !ids.is_empty();
        let mut ids = ids.into_iter();
        for e in self.list.iter_mut().filter(|e| e.id.is_empty()) {
            e.id = ids.next().unwrap_or_default();
        }
        Ok(changed)
    }

    /// Look an endpoint up by id. An empty id matches nothing.
    #[must_use]
    pub fn get_by_id(&self, id: &str) -> Option<&Endpoint> {
        if id.is_empty() {
            return None;
        }
        self.list.iter().find(|e| e.id == id)
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

    /// The enabled endpoint that owns `token`, when the caller does not say which one (the announce API).
    /// Every endpoint is compared, without an early exit, so timing does not reveal which one matched.
    #[must_use]
    pub fn authenticate_token(&self, token: &str) -> Option<&Endpoint> {
        let presented = hash_token(token);
        let mut found = None;
        for ep in &self.list {
            if constant_time_eq(&presented, &ep.token_hash) && ep.enabled {
                found = Some(ep);
            }
        }
        found
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
    fn spoken_names_are_plain_words() {
        assert_eq!(
            clean_spoken_name("  the kitchen "),
            Ok(Some("the kitchen".into()))
        );
        assert_eq!(clean_spoken_name("   "), Ok(None));
        for bad in [
            "a\nb",
            "<b>x</b>",
            "[x](http://e)",
            "x`y",
            "a  b",
            "-x",
            "say \"hi\"",
            "a|b",
            "a\u{7}b",
            &"x".repeat(41),
        ] {
            assert_eq!(
                clean_spoken_name(bad),
                Err(EndpointError::InvalidSpokenName),
                "{bad:?}"
            );
        }
        assert!(clean_spoken_name(&"x".repeat(40)).is_ok());
        let mut eps = Endpoints::default();
        assert_eq!(
            eps.create_with("k", "generic", Some("<x>"), 0).unwrap_err(),
            EndpointError::InvalidSpokenName
        );
        assert!(eps.all().is_empty(), "nothing is created on a bad name");
        let (ep, _) = eps.create_with("k", "generic", Some("Kitchen"), 0).unwrap();
        assert_eq!(ep.origin(), "Kitchen");
        eps.set_spoken_name("k", "").unwrap();
        assert_eq!(eps.get("k").unwrap().origin(), "k");
        assert_eq!(eps.set_spoken_name("zz", "x"), Err(EndpointError::NotFound));
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

    // REQ-INB-013: the id is what an endpoint is known by; a name used again is a different endpoint.
    #[test]
    fn ids_are_random_and_a_name_used_again_gets_a_new_one() {
        let mut eps = Endpoints::default();
        let (a, _) = eps.create("x", "generic", 0).unwrap();
        assert_eq!(a.id.len(), 32);
        assert!(a.id.bytes().all(|b| b.is_ascii_hexdigit()));
        assert!(eps.get_by_id(&a.id).is_some());
        assert!(eps.get_by_id("").is_none());
        eps.revoke("x").unwrap();
        assert!(eps.get_by_id(&a.id).is_none(), "the id died with it");
        let (b, _) = eps.create("x", "generic", 0).unwrap();
        assert_ne!(a.id, b.id);
    }

    // REQ-NFR-010: a list saved before ids existed loads; its endpoints get ids, once, and keep them.
    #[test]
    fn a_list_saved_without_ids_loads_and_gets_them_once() {
        let old = r#"{"list":[
            {"name":"a","adapter":"generic","token_hash":"h1","created_at":1,"enabled":true},
            {"name":"b","adapter":"generic","token_hash":"h2","created_at":2,"enabled":false,"spoken_name":"Bee"}]}"#;
        let mut eps: Endpoints = serde_json::from_str(old).unwrap();
        assert!(eps.all().iter().all(|e| e.id.is_empty()));
        assert!(eps.get_by_id("").is_none(), "no id matches the empty one");
        assert!(eps.assign_missing_ids().unwrap());
        let ids: Vec<String> = eps.all().iter().map(|e| e.id.clone()).collect();
        assert!(ids.iter().all(|i| i.len() == 32) && ids[0] != ids[1]);
        assert!(!eps.assign_missing_ids().unwrap(), "nothing to give twice");
        assert_eq!(
            eps.all().iter().map(|e| e.id.clone()).collect::<Vec<_>>(),
            ids
        );
        assert_eq!(eps.get("b").unwrap().origin(), "Bee", "the rest is kept");
        let again: Endpoints = serde_json::from_str(&serde_json::to_string(&eps).unwrap()).unwrap();
        assert_eq!(again, eps);
    }

    #[test]
    fn serialisation_never_contains_the_plaintext_token() {
        let mut eps = Endpoints::default();
        let (_, token) = eps.create("x", "generic", 0).unwrap();
        let json = serde_json::to_string(&eps).unwrap();
        assert!(!json.contains(&token));
    }
}
