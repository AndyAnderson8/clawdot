//! The web UI administrator: one password or deliberate open mode, never accounts
//! (REQ-CFG-018..024, ADR-0013).
//!
//! * [`hash_password`] / [`verify_password`]: PBKDF2-HMAC-SHA256 on the `sha2` crate that is already in the
//!   build. Constant memory, no new dependency. The iteration count and salt travel with the hash.
//! * [`Admin`]: the setting (unset, open, or password), and the in-memory sessions.
//! * [`is_private`]: which peers may complete setup or use open mode.
//! * Forgotten password (REQ-CFG-069): a short in-memory *reset window* opened by a device button. There is no
//!   recovery code (REQ-CFG-067, 068, 071..073 are removed).

use crate::inbox::endpoint::{constant_time_eq, generate_token, hash_token, verify_token};
use crate::sched::Ts;
use crate::store::{JsonStore, StoreError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::net::IpAddr;

/// Schema version of `admin.json`.
pub const SCHEMA: u32 = 1;
/// PBKDF2 iterations for new hashes (REQ-CFG-019 sets 100,000 as the floor).
pub const ITERATIONS: u32 = 100_000;
/// Shortest and longest accepted password, in characters.
pub const MIN_PASSWORD: usize = 8;
/// See [`MIN_PASSWORD`].
pub const MAX_PASSWORD: usize = 128;
/// Live sign-ins kept at once; the oldest is dropped beyond this.
pub const MAX_SESSIONS: usize = 8;
/// How long a sign-in lasts.
pub const SESSION_SECS: Ts = 12 * 3600;

/// How long the local reset window stays open (REQ-CFG-069).
pub const RESET_WINDOW_SECS: Ts = 300;
/// After a window was used or ran out, a new one cannot be opened for this long.
pub const RESET_COOLDOWN_SECS: Ts = 60;

const PREFIX: &str = "pbkdf2-sha256";
const BLOCK: usize = 64;

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 || !s.is_ascii() {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

/// PBKDF2-HMAC-SHA256 with a 32-byte output (one block).
fn pbkdf2(password: &[u8], salt: &[u8], iterations: u32) -> [u8; 32] {
    // HMAC key: hash it if longer than a block, then pad.
    let mut key = [0u8; BLOCK];
    if password.len() > BLOCK {
        key[..32].copy_from_slice(&Sha256::digest(password));
    } else {
        key[..password.len()].copy_from_slice(password);
    }
    let pad = |x: u8| key.map(|b| b ^ x);
    let inner = Sha256::new().chain_update(pad(0x36));
    let outer = Sha256::new().chain_update(pad(0x5c));
    let mac = |data: &[u8]| -> [u8; 32] {
        let i = inner.clone().chain_update(data).finalize();
        outer.clone().chain_update(i).finalize().into()
    };
    let mut first = salt.to_vec();
    first.extend_from_slice(&1u32.to_be_bytes());
    let mut u = mac(&first);
    let mut out = u;
    for _ in 1..iterations {
        u = mac(&u);
        for (o, x) in out.iter_mut().zip(u) {
            *o ^= x;
        }
    }
    out
}

/// Hash a password for storage: `pbkdf2-sha256$<iterations>$<salt hex>$<hash hex>`.
///
/// # Errors
/// If the OS cannot supply random bytes.
pub fn hash_password(password: &str) -> Result<String, AdminError> {
    hash_password_with(password, ITERATIONS)
}

fn hash_password_with(password: &str, iterations: u32) -> Result<String, AdminError> {
    let mut salt = [0u8; 16];
    getrandom::fill(&mut salt).map_err(|_| AdminError::NoRandomness)?;
    let h = pbkdf2(password.as_bytes(), &salt, iterations);
    Ok(format!("{PREFIX}${iterations}${}${}", hex(&salt), hex(&h)))
}

/// Whether `password` matches a stored value. A bare 64-digit hex string is an old administrator token hash
/// (SHA-256 of a random token), which keeps working (REQ-CFG-023).
#[must_use]
pub fn verify_password(password: &str, stored: &str) -> bool {
    let mut parts = stored.split('$');
    if parts.next() != Some(PREFIX) {
        return verify_token(password, stored);
    }
    let (Some(iters), Some(salt), Some(want), None) = (
        parts.next().and_then(|v| v.parse::<u32>().ok()),
        parts.next().and_then(unhex),
        parts.next(),
        parts.next(),
    ) else {
        return false;
    };
    if iters == 0 || iters > 10_000_000 {
        return false;
    }
    constant_time_eq(&hex(&pbkdf2(password.as_bytes(), &salt, iters)), want)
}

/// Whether a stored value is an old-style token hash rather than a password hash.
#[must_use]
pub fn is_legacy(stored: &str) -> bool {
    !stored.starts_with(PREFIX)
}

/// Whether `ip` is loopback, private, link-local, or in 100.64.0.0/10 (shared space used by Tailscale and
/// carrier-grade NAT). Open mode and setup are limited to these (REQ-CFG-020, REQ-CFG-021).
#[must_use]
pub fn is_private(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => {
            let o = v.octets();
            v.is_loopback()
                || v.is_private()
                || v.is_link_local()
                || (o[0] == 100 && (o[1] & 0xc0) == 64)
        }
        IpAddr::V6(v) => {
            if let Some(m) = v.to_ipv4_mapped() {
                return is_private(IpAddr::V4(m));
            }
            let s = v.segments()[0];
            v.is_loopback() || (s & 0xfe00) == 0xfc00 || (s & 0xffc0) == 0xfe80
        }
    }
}

/// Why an administrator operation failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdminError {
    /// No administrator setting yet: finish setup first.
    SetupRequired,
    /// Not signed in, or wrong credentials.
    Unauthorized,
    /// Too many failed attempts.
    RateLimited,
    /// The request came from an address that is not on a private network.
    NotPrivate,
    /// Setup is already done.
    AlreadySetUp,
    /// Open mode is not allowed with this listen address.
    OpenRefused,
    /// The password is shorter or longer than allowed.
    BadPassword,
    /// The setting could not be saved.
    Storage,
    /// No randomness from the OS.
    NoRandomness,
    /// There is no password to reset (open mode or setup pending).
    NoPassword,
    /// The local reset window is not open.
    WindowClosed,
    /// A password change did not prove the current password: it was missing or wrong.
    WrongCurrent,
}

impl std::fmt::Display for AdminError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::SetupRequired => "setup required",
            Self::Unauthorized => "sign in required",
            Self::RateLimited => "too many attempts",
            Self::NotPrivate => "this is only available from a private network",
            Self::AlreadySetUp => "setup is already complete",
            Self::OpenRefused => "open mode is not allowed on a public listen address",
            Self::BadPassword => "the password must be 8 to 128 characters",
            Self::Storage => "the setting could not be saved",
            Self::NoRandomness => "no randomness available",
            Self::NoPassword => "no password is set",
            Self::WindowClosed => "the reset window is not open",
            Self::WrongCurrent => "the current password is missing or wrong",
        })
    }
}

impl std::error::Error for AdminError {}

/// What `admin.json` holds.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum Setting {
    /// Nothing chosen yet: setup is shown.
    #[default]
    Unset,
    /// Deliberately unprotected, private networks only.
    Open,
    /// Protected by a password hash.
    Password {
        /// See [`hash_password`].
        hash: String,
        // Files written by older versions may carry a `recovery` hash; it is ignored on load and dropped on save.
    },
}

/// Written by hand so that no `{:?}` of an [`Admin`] (or of anything holding one) can put the password hash in a log.
impl std::fmt::Debug for Setting {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unset => f.write_str("Unset"),
            Self::Open => f.write_str("Open"),
            Self::Password { .. } => f.write_str("Password { hash: <hidden> }"),
        }
    }
}

/// The state of the administrator setting, for the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Setup is shown.
    Setup,
    /// Open mode.
    Open,
    /// Password mode.
    Password,
}

impl Mode {
    /// The word the API uses.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Setup => "setup",
            Self::Open => "open",
            Self::Password => "password",
        }
    }
}

/// The administrator setting and sessions.
#[derive(Debug)]
pub struct Admin {
    store: Option<JsonStore>,
    setting: Setting,
    /// (SHA-256 of the session token, expiry)
    sessions: Vec<(String, Ts)>,
    listen: IpAddr,
    /// When the local reset window closes, if it is open. In memory only (REQ-CFG-069).
    window: Option<Ts>,
    /// When the last window ended (used or ran out); a new one waits for the cooldown.
    window_ended: Option<Ts>,
}

impl Admin {
    /// A fresh, unset administrator (tests and demos), listening on loopback.
    #[must_use]
    pub fn new() -> Self {
        Self {
            store: None,
            setting: Setting::Unset,
            sessions: Vec::new(),
            listen: IpAddr::from([127, 0, 0, 1]),
            window: None,
            window_ended: None,
        }
    }

    /// Load the setting from `store` (unset if absent).
    ///
    /// # Errors
    /// Storage errors, including a file written by a newer version.
    pub fn open(store: JsonStore) -> Result<Self, StoreError> {
        let setting = store.load::<Setting>()?;
        Ok(Self {
            store: Some(store),
            setting,
            ..Self::new()
        })
    }

    fn save(&self) -> Result<(), AdminError> {
        if let Some(s) = &self.store {
            s.save(&self.setting).map_err(|_| AdminError::Storage)?;
        }
        Ok(())
    }

    /// Where the core is listening, which decides whether open mode is allowed.
    pub fn set_listen_ip(&mut self, ip: IpAddr) {
        self.listen = ip;
    }

    /// The current mode.
    #[must_use]
    pub fn mode(&self) -> Mode {
        match self.setting {
            Setting::Unset => Mode::Setup,
            Setting::Open => Mode::Open,
            Setting::Password { .. } => Mode::Password,
        }
    }

    /// Whether the stored credential is an old administrator token hash.
    #[must_use]
    pub fn legacy(&self) -> bool {
        matches!(&self.setting, Setting::Password { hash, .. } if is_legacy(hash))
    }

    /// Whether open mode may be chosen with this listen address.
    #[must_use]
    pub fn open_allowed(&self) -> bool {
        self.listen.is_unspecified() || is_private(self.listen)
    }

    /// Complete setup with open mode.
    ///
    /// # Errors
    /// See [`AdminError`].
    pub fn setup_open(&mut self, peer: IpAddr) -> Result<(), AdminError> {
        self.check_setup(peer)?;
        if !self.open_allowed() {
            return Err(AdminError::OpenRefused);
        }
        self.setting = Setting::Open;
        self.finish_setup()
    }

    /// Complete setup with a password; returns a session token (the administrator is signed in).
    ///
    /// # Errors
    /// See [`AdminError`].
    pub fn setup_password(
        &mut self,
        peer: IpAddr,
        hash: String,
        now: Ts,
    ) -> Result<String, AdminError> {
        self.check_setup(peer)?;
        self.setting = Setting::Password { hash };
        self.finish_setup()?;
        self.new_session(now)
    }

    /// Whether setup may proceed from `peer`.
    ///
    /// # Errors
    /// [`AdminError::AlreadySetUp`] or [`AdminError::NotPrivate`].
    pub fn check_setup(&self, peer: IpAddr) -> Result<(), AdminError> {
        if self.mode() != Mode::Setup {
            return Err(AdminError::AlreadySetUp);
        }
        if !is_private(peer) {
            return Err(AdminError::NotPrivate);
        }
        Ok(())
    }

    fn finish_setup(&mut self) -> Result<(), AdminError> {
        if let Err(e) = self.save() {
            self.setting = Setting::Unset;
            return Err(e);
        }
        Ok(())
    }

    /// The stored credential, for verification outside the lock (the hash is slow).
    #[must_use]
    pub fn password_hash(&self) -> Option<String> {
        match &self.setting {
            Setting::Password { hash, .. } => Some(hash.clone()),
            _ => None,
        }
    }

    /// Open a session after a successful password check.
    ///
    /// # Errors
    /// If the OS cannot supply random bytes.
    pub fn new_session(&mut self, now: Ts) -> Result<String, AdminError> {
        let token = generate_token().map_err(|_| AdminError::NoRandomness)?;
        self.sessions.retain(|(_, exp)| *exp > now);
        if self.sessions.len() >= MAX_SESSIONS {
            self.sessions.remove(0);
        }
        self.sessions.push((hash_token(&token), now + SESSION_SECS));
        Ok(token)
    }

    /// Whether `bearer` is a live session.
    #[must_use]
    pub fn session_ok(&mut self, bearer: &str, now: Ts) -> bool {
        self.sessions.retain(|(_, exp)| *exp > now);
        let h = hash_token(bearer);
        self.sessions.iter().any(|(s, _)| constant_time_eq(s, &h))
    }

    /// End a session.
    pub fn end_session(&mut self, bearer: &str) {
        let h = hash_token(bearer);
        self.sessions.retain(|(s, _)| !constant_time_eq(s, &h));
    }

    /// Whether a request may use the administrator API. In open mode any private peer may; in password mode
    /// the bearer must be a live session.
    ///
    /// # Errors
    /// [`AdminError::SetupRequired`], [`AdminError::NotPrivate`], [`AdminError::Unauthorized`].
    pub fn authorize(
        &mut self,
        bearer: Option<&str>,
        peer: IpAddr,
        now: Ts,
    ) -> Result<(), AdminError> {
        match self.mode() {
            Mode::Setup => Err(AdminError::SetupRequired),
            Mode::Open if is_private(peer) => Ok(()),
            Mode::Open => Err(AdminError::NotPrivate),
            Mode::Password => match bearer {
                Some(b) if self.session_ok(b, now) => Ok(()),
                _ => Err(AdminError::Unauthorized),
            },
        }
    }

    /// Replace the credential with a new password hash, ending every session. Returns a fresh session.
    ///
    /// # Errors
    /// Storage and randomness failures.
    pub fn set_password(&mut self, hash: String, now: Ts) -> Result<String, AdminError> {
        let before = std::mem::replace(&mut self.setting, Setting::Password { hash });
        if let Err(e) = self.save() {
            self.setting = before;
            return Err(e);
        }
        self.sessions.clear();
        self.new_session(now)
    }

    // ----- local reset window (REQ-CFG-069) -----

    fn window_tick(&mut self, now: Ts) {
        if let Some(end) = self.window
            && now >= end
        {
            self.window = None;
            self.window_ended = Some(end);
        }
    }

    /// Seconds the local reset window has left (0 when closed).
    pub fn window_left(&mut self, now: Ts) -> Ts {
        self.window_tick(now);
        self.window.map_or(0, |end| end - now)
    }

    /// Open the local reset window. Returns the seconds it stays open. Asking again while it is open reports the
    /// time left and does not extend it.
    ///
    /// # Errors
    /// [`AdminError::NoPassword`] unless a password is set; [`AdminError::RateLimited`] during the cooldown.
    pub fn open_window(&mut self, now: Ts) -> Result<Ts, AdminError> {
        if self.mode() != Mode::Password {
            return Err(AdminError::NoPassword);
        }
        let left = self.window_left(now);
        if left > 0 {
            return Ok(left);
        }
        if self
            .window_ended
            .is_some_and(|e| now < e + RESET_COOLDOWN_SECS)
        {
            return Err(AdminError::RateLimited);
        }
        self.window = Some(now + RESET_WINDOW_SECS);
        Ok(RESET_WINDOW_SECS)
    }

    /// Whether the window is open for `peer`: a private address and time left.
    ///
    /// # Errors
    /// [`AdminError::NotPrivate`], [`AdminError::NoPassword`] or [`AdminError::WindowClosed`].
    pub fn check_window(&mut self, peer: IpAddr, now: Ts) -> Result<(), AdminError> {
        if self.mode() != Mode::Password {
            return Err(AdminError::NoPassword);
        }
        if !is_private(peer) {
            return Err(AdminError::NotPrivate);
        }
        if self.window_left(now) == 0 {
            return Err(AdminError::WindowClosed);
        }
        Ok(())
    }

    /// Set a new password through the open window: the window closes and every session ends.
    ///
    /// # Errors
    /// As [`Admin::check_window`], plus storage and randomness failures.
    pub fn reset_in_window(
        &mut self,
        peer: IpAddr,
        hash: String,
        now: Ts,
    ) -> Result<String, AdminError> {
        self.check_window(peer, now)?;
        self.window = None;
        self.window_ended = Some(now);
        self.set_password(hash, now)
    }
}

impl Default for Admin {
    fn default() -> Self {
        Self::new()
    }
}

/// Whether a password has an acceptable length.
#[must_use]
pub fn password_ok(p: &str) -> bool {
    (MIN_PASSWORD..=MAX_PASSWORD).contains(&p.chars().count())
}

/// Convert an old `admin-token.hash` file into `admin.json` (REQ-CFG-023) and remove the old file.
/// Does nothing when `admin.json` already exists or there is no old file. Returns whether it converted.
///
/// # Errors
/// Storage errors.
pub fn migrate_legacy(dir: &std::path::Path) -> Result<bool, StoreError> {
    let store = JsonStore::new(dir.join("admin.json"), SCHEMA);
    let old = dir.join("admin-token.hash");
    if store.path().exists() {
        return Ok(false);
    }
    let Ok(text) = std::fs::read_to_string(&old) else {
        return Ok(false);
    };
    let hash = text.trim().to_owned();
    if hash.len() != 64 || unhex(&hash).is_none() {
        return Ok(false);
    }
    store.save(&Setting::Password { hash })?;
    std::fs::remove_file(&old)?;
    Ok(true)
}

/// Remove the administrator setting and any old token file (`--reset-admin`, REQ-CFG-022). Returns whether
/// anything was removed.
///
/// # Errors
/// I/O errors other than "not found".
pub fn reset(dir: &std::path::Path) -> std::io::Result<bool> {
    let mut removed = false;
    let store_file = dir.join("admin.json");
    let doomed = [
        store_file.clone(),
        dir.join("admin-token.hash"),
        // The temporary file of a save cut short, and the name older versions gave it.
        crate::store::temp_path(&store_file),
        dir.join("admin.tmp"),
    ];
    for path in doomed {
        match std::fs::remove_file(path) {
            Ok(()) => removed = true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::testutil::TempDir;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    /// Known answers for PBKDF2-HMAC-SHA256 (RFC 7914 section 11 vector with c=1 is for "passwd"/"salt"; these
    /// are the widely published "password"/"salt" vectors).
    #[test]
    fn pbkdf2_matches_published_vectors() {
        assert_eq!(
            hex(&pbkdf2(b"password", b"salt", 1)),
            "120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b"
        );
        assert_eq!(
            hex(&pbkdf2(b"password", b"salt", 4096)),
            "c5e478d59288c841aa530db6845c4c8d962893a001ce4e11a4963873aa98134a"
        );
    }

    // REQ-CFG-019
    #[test]
    fn the_password_hash_never_appears_in_debug_output() {
        let hash = hash_password("correct horse").unwrap();
        let setting = Setting::Password { hash: hash.clone() };
        assert!(!format!("{setting:?}").contains(&hash));
        let mut admin = Admin::new();
        admin.setting = setting;
        assert!(!format!("{admin:?}").contains(&hash));
        assert!(format!("{:?}", Setting::Open).contains("Open"));
    }

    // REQ-CFG-019
    #[test]
    fn passwords_are_salted_checked_and_never_stored_plain() {
        let a = hash_password("correct horse").unwrap();
        let b = hash_password("correct horse").unwrap();
        assert_ne!(a, b, "a fresh salt each time");
        assert!(a.starts_with("pbkdf2-sha256$100000$"));
        assert!(!a.contains("correct"));
        assert!(verify_password("correct horse", &a));
        assert!(!verify_password("correct horsf", &a));
        assert!(!verify_password("", &a));
        assert!(!verify_password("x", "pbkdf2-sha256$0$00$00"));
        assert!(!verify_password("x", "pbkdf2-sha256$5"));
        assert!(!verify_password("x", "garbage"));
    }

    #[test]
    fn password_length_is_bounded() {
        assert!(!password_ok("short"));
        assert!(password_ok("long enough"));
        assert!(password_ok(&"é".repeat(128)));
        assert!(!password_ok(&"a".repeat(129)));
    }

    // REQ-CFG-020, REQ-CFG-021
    #[test]
    fn private_addresses_are_recognised() {
        for a in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.9",
            "192.168.1.20",
            "169.254.1.1",
            "100.64.0.10",
            "100.127.255.1",
            "::1",
            "fd12::1",
            "fe80::1",
            "::ffff:192.168.0.5",
        ] {
            assert!(is_private(ip(a)), "{a}");
        }
        for a in [
            "8.8.8.8",
            "100.128.0.1",
            "100.63.0.1",
            "172.32.0.1",
            "2001:db8::1",
            "::ffff:8.8.8.8",
        ] {
            assert!(!is_private(ip(a)), "{a}");
        }
    }

    // REQ-CFG-018
    #[test]
    fn nothing_works_before_setup() {
        let mut a = Admin::new();
        assert_eq!(a.mode(), Mode::Setup);
        assert_eq!(
            a.authorize(Some("x"), ip("127.0.0.1"), 0),
            Err(AdminError::SetupRequired)
        );
    }

    // REQ-CFG-021
    #[test]
    fn setup_needs_a_private_peer_but_no_code() {
        let mut a = Admin::new();
        a.set_listen_ip(ip("192.168.1.20"));
        assert_eq!(a.setup_open(ip("8.8.8.8")), Err(AdminError::NotPrivate));
        a.setup_open(ip("192.168.1.5")).unwrap();
        assert_eq!(a.mode(), Mode::Open);
        assert_eq!(
            a.setup_open(ip("192.168.1.5")),
            Err(AdminError::AlreadySetUp)
        );
    }

    // REQ-CFG-020
    #[test]
    fn open_mode_is_refused_on_a_public_listen_address_and_for_public_peers() {
        let mut a = Admin::new();
        a.set_listen_ip(ip("203.0.113.9"));
        assert_eq!(
            a.setup_open(ip("192.168.1.5")),
            Err(AdminError::OpenRefused)
        );
        let mut a = Admin::new();
        a.set_listen_ip(ip("0.0.0.0"));
        a.setup_open(ip("10.0.0.2")).unwrap();
        assert_eq!(a.authorize(None, ip("10.0.0.3"), 0), Ok(()));
        assert_eq!(
            a.authorize(None, ip("8.8.8.8"), 0),
            Err(AdminError::NotPrivate)
        );
    }

    // REQ-CFG-019
    #[test]
    fn sessions_expire_are_capped_and_can_end() {
        let mut a = Admin::new();
        let t = a.setup_password(ip("127.0.0.1"), "h".into(), 100).unwrap();
        assert_eq!(a.authorize(Some(&t), ip("127.0.0.1"), 101), Ok(()));
        assert_eq!(
            a.authorize(Some("nope"), ip("127.0.0.1"), 101),
            Err(AdminError::Unauthorized)
        );
        assert_eq!(
            a.authorize(None, ip("127.0.0.1"), 101),
            Err(AdminError::Unauthorized)
        );
        assert_eq!(
            a.authorize(Some(&t), ip("127.0.0.1"), 100 + SESSION_SECS),
            Err(AdminError::Unauthorized),
            "expired"
        );
        let first = a.new_session(1000).unwrap();
        let mut last = String::new();
        for _ in 0..MAX_SESSIONS {
            last = a.new_session(1000).unwrap();
        }
        assert!(!a.session_ok(&first, 1001), "the oldest was dropped");
        assert!(a.session_ok(&last, 1001));
        a.end_session(&last);
        assert!(!a.session_ok(&last, 1001));
    }

    // REQ-CFG-024
    #[test]
    fn a_new_password_ends_other_sessions() {
        let mut a = Admin::new();
        let old = a.setup_password(ip("127.0.0.1"), "h1".into(), 0).unwrap();
        let fresh = a.set_password("h2".into(), 1).unwrap();
        assert!(!a.session_ok(&old, 2));
        assert!(a.session_ok(&fresh, 2));
        assert_eq!(a.password_hash().as_deref(), Some("h2"));
    }

    #[test]
    fn the_setting_survives_a_restart_and_reset_clears_it() {
        let dir = TempDir::new("admin");
        let store = JsonStore::new(dir.0.join("admin.json"), SCHEMA);
        let mut a = Admin::open(store.clone()).unwrap();
        assert_eq!(a.mode(), Mode::Setup);
        a.setup_open(ip("127.0.0.1")).unwrap();
        assert_eq!(Admin::open(store).unwrap().mode(), Mode::Open);
        assert!(reset(&dir.0).unwrap());
        assert!(!reset(&dir.0).unwrap());
        // A save cut short leaves its temporary file; reset clears that too, under the name the store gives it.
        let tmp = crate::store::temp_path(&dir.0.join("admin.json"));
        std::fs::write(&tmp, b"{").unwrap();
        assert!(reset(&dir.0).unwrap());
        assert!(!tmp.exists());
        let store = JsonStore::new(dir.0.join("admin.json"), SCHEMA);
        assert_eq!(Admin::open(store).unwrap().mode(), Mode::Setup);
    }

    // REQ-CFG-023
    #[test]
    fn an_old_token_hash_becomes_a_working_password() {
        let dir = TempDir::new("admin-legacy");
        let old = hash_token("cdt_oldtoken");
        std::fs::write(dir.0.join("admin-token.hash"), format!("{old}\n")).unwrap();
        assert!(migrate_legacy(&dir.0).unwrap());
        assert!(!dir.0.join("admin-token.hash").exists());
        assert!(!migrate_legacy(&dir.0).unwrap());
        let a = Admin::open(JsonStore::new(dir.0.join("admin.json"), SCHEMA)).unwrap();
        assert_eq!(a.mode(), Mode::Password);
        assert!(a.legacy());
        let h = a.password_hash().unwrap();
        assert!(verify_password("cdt_oldtoken", &h));
        assert!(!verify_password("cdt_other", &h));
    }

    #[test]
    fn migration_never_overrides_a_new_setting_or_junk() {
        let dir = TempDir::new("admin-legacy2");
        std::fs::write(dir.0.join("admin-token.hash"), "not a hash").unwrap();
        assert!(!migrate_legacy(&dir.0).unwrap());
        assert!(dir.0.join("admin-token.hash").exists());
    }

    fn with_password() -> (Admin, String) {
        let mut a = Admin::new();
        let t = a
            .setup_password(ip("127.0.0.1"), "pw-hash".into(), 0)
            .unwrap();
        (a, t)
    }

    // REQ-CFG-067 (removed): an old file that still holds a recovery hash loads; the field is dropped on save.
    #[test]
    fn an_old_file_with_a_recovery_hash_loads_and_the_field_is_dropped_on_save() {
        let dir = TempDir::new("admin-oldrec");
        std::fs::write(
            dir.0.join("admin.json"),
            r#"{"schema":1,"data":{"mode":"password","hash":"pbkdf2-sha256$1$00$00","recovery":"pbkdf2-sha256$1$00$11"}}"#,
        )
        .unwrap();
        let store = JsonStore::new(dir.0.join("admin.json"), SCHEMA);
        let mut a = Admin::open(store).unwrap();
        assert_eq!(a.mode(), Mode::Password);
        a.set_password("h2".into(), 1).unwrap();
        let text = std::fs::read_to_string(dir.0.join("admin.json")).unwrap();
        assert!(!text.contains("recovery"), "{text}");
        assert!(text.contains("h2"));
    }

    // REQ-CFG-072
    #[test]
    fn open_and_setup_modes_have_no_reset_window() {
        let mut a = Admin::new();
        assert_eq!(a.open_window(0), Err(AdminError::NoPassword));
        a.setup_open(ip("127.0.0.1")).unwrap();
        assert_eq!(a.open_window(0), Err(AdminError::NoPassword));
        assert_eq!(
            a.check_window(ip("127.0.0.1"), 0),
            Err(AdminError::NoPassword)
        );
    }

    // REQ-CFG-069
    #[test]
    fn the_reset_window_is_short_private_single_use_and_cooled_down() {
        let (mut a, old) = with_password();
        // Closed by default.
        assert_eq!(
            a.check_window(ip("192.168.1.9"), 10),
            Err(AdminError::WindowClosed)
        );
        assert_eq!(a.open_window(10), Ok(RESET_WINDOW_SECS));
        // Asking again does not extend it.
        assert_eq!(a.open_window(100), Ok(RESET_WINDOW_SECS - 90));
        assert_eq!(a.window_left(100), RESET_WINDOW_SECS - 90);
        // A public address is refused even while it is open.
        assert_eq!(
            a.check_window(ip("8.8.8.8"), 100),
            Err(AdminError::NotPrivate)
        );
        // It runs out.
        assert_eq!(
            a.check_window(ip("192.168.1.9"), 10 + RESET_WINDOW_SECS),
            Err(AdminError::WindowClosed)
        );
        // After running out, the cooldown holds off a new one.
        assert_eq!(
            a.open_window(10 + RESET_WINDOW_SECS + 5),
            Err(AdminError::RateLimited)
        );
        let t0 = 10 + RESET_WINDOW_SECS + RESET_COOLDOWN_SECS;
        assert_eq!(a.open_window(t0), Ok(RESET_WINDOW_SECS));
        // Use: new password, sessions end, the window closes, a replay fails.
        let s = a
            .reset_in_window(ip("10.0.0.7"), "pw2".into(), t0 + 1)
            .unwrap();
        assert!(!a.session_ok(&old, t0 + 2));
        assert!(a.session_ok(&s, t0 + 2));
        assert_eq!(
            a.reset_in_window(ip("10.0.0.7"), "pw3".into(), t0 + 2),
            Err(AdminError::WindowClosed)
        );
        assert_eq!(a.password_hash().as_deref(), Some("pw2"));
        assert_eq!(a.open_window(t0 + 3), Err(AdminError::RateLimited));
    }

    #[test]
    fn the_window_is_not_stored_so_a_restart_closes_it() {
        let dir = TempDir::new("admin-window");
        let store = JsonStore::new(dir.0.join("admin.json"), SCHEMA);
        let mut a = Admin::open(store.clone()).unwrap();
        a.setup_password(ip("127.0.0.1"), "h".into(), 0).unwrap();
        a.open_window(1).unwrap();
        let mut again = Admin::open(store).unwrap();
        assert_eq!(again.window_left(2), 0);
    }
}
