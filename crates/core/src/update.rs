//! Update checks and confirmed updates (REQ-UPD-001 to REQ-UPD-010).
//!
//! ClawDot looks for a newer release about once a day (and whenever the person asks), tells them, and installs it
//! only when they confirm. The check is a plain `GET` of a public page that reveals nothing but the address the
//! request comes from. Installing needs a signed release: the release carries a `SHA256SUMS` file and its
//! `SHA256SUMS.sig`, an Ed25519 signature made with the maintainer's key, whose public half is compiled in or set
//! in the configuration. A build with no key never installs anything by itself; it only says a new version exists.
//! The old program is kept as `<name>.prev` so a bad update can be put back.

use crate::transport::{Connector, Endpoint};
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Bytes;
use hyper::header::{ACCEPT, HOST, LOCATION, USER_AGENT};
use hyper::{Method, Request};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The version this program is.
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// Where releases are listed unless the configuration says otherwise.
pub const DEFAULT_URL: &str = "https://api.github.com/repos/AndyAnderson8/clawdot/releases/latest";

/// Ask for a newer release at most this often.
const CHECK_EVERY_SECS: u64 = 24 * 60 * 60;
/// The release listing is small; anything bigger is not one.
const MAX_FEED: usize = 512 * 1024;
/// The signed list of hashes and its signature are tiny.
const MAX_SMALL: usize = 64 * 1024;
/// The largest program file accepted.
const MAX_BINARY: usize = 96 * 1024 * 1024;
/// How long the install may take from start to finish.
const APPLY_DEADLINE: Duration = Duration::from_secs(15 * 60);
/// The names of the files a release must carry besides the program itself.
const SUMS_NAME: &str = "SHA256SUMS";
const SIG_NAME: &str = "SHA256SUMS.sig";
/// Marks an update that has not yet proved itself by running for a while.
const PENDING_FILE: &str = "update-pending";
/// How long a new version must run before the old one is no longer needed.
const PROVE_SECS: u64 = 120;

/// One file attached to a release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    /// File name, for example `clawdot-linux-armv7`.
    pub name: String,
    /// Where to download it (always https).
    pub url: String,
    /// Size in bytes, as the release says.
    pub size: u64,
}

/// A published version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// For example `0.2.0`, with no leading `v`.
    pub version: String,
    /// What changed, as text. Never interpreted as markup.
    pub notes: String,
    /// The page that describes the release.
    pub page: String,
    /// When it was published, as the release says.
    pub published: String,
    /// The files attached.
    pub assets: Vec<Asset>,
}

/// A version number: `1.2.3`, optionally with `-pre` and `+build`.
fn parse_version(s: &str) -> Option<([u64; 3], Option<String>)> {
    let s = s.trim().trim_start_matches(['v', 'V']);
    let s = s.split_once('+').map_or(s, |(a, _)| a);
    let (core, pre) = match s.split_once('-') {
        Some((c, p)) => (c, Some(p.to_owned())),
        None => (s, None),
    };
    let mut n = [0_u64; 3];
    let mut parts = core.split('.');
    for slot in &mut n {
        *slot = parts.next()?.parse().ok()?;
    }
    if parts.next().is_some() {
        return None;
    }
    Some((n, pre))
}

/// Whether `candidate` is a later version than `current`. Anything that is not a version number is never newer, and
/// a pre-release is older than the release it leads to.
#[must_use]
pub fn is_newer(current: &str, candidate: &str) -> bool {
    let (Some((a, ap)), Some((b, bp))) = (parse_version(current), parse_version(candidate)) else {
        return false;
    };
    match a.cmp(&b) {
        Ordering::Less => true,
        Ordering::Greater => false,
        Ordering::Equal => ap.is_some() && bp.is_none(),
    }
}

/// Read a GitHub-style "latest release" document.
///
/// # Errors
/// A plain description of what is wrong with it.
pub fn parse_release(body: &str) -> Result<Release, String> {
    let v: Value = serde_json::from_str(body).map_err(|_| "the release list is not readable")?;
    if v["draft"].as_bool() == Some(true) || v["prerelease"].as_bool() == Some(true) {
        return Err("the newest release is a draft or a pre-release".into());
    }
    let version = v["tag_name"]
        .as_str()
        .map(|t| t.trim().trim_start_matches(['v', 'V']).to_owned())
        .filter(|t| parse_version(t).is_some())
        .ok_or("the release has no version number")?;
    let page = v["html_url"]
        .as_str()
        .filter(|u| url_ok(u))
        .unwrap_or("")
        .to_owned();
    let notes: String = v["body"]
        .as_str()
        .unwrap_or("")
        .trim()
        .chars()
        .take(4000)
        .collect();
    let published = v["published_at"]
        .as_str()
        .unwrap_or("")
        .chars()
        .take(40)
        .collect();
    let assets = v["assets"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| {
                    let url = x["browser_download_url"].as_str()?;
                    let name = x["name"].as_str()?;
                    (url_ok(url) && !name.is_empty() && name.len() < 120).then(|| Asset {
                        name: name.to_owned(),
                        url: url.to_owned(),
                        size: x["size"].as_u64().unwrap_or(0),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Release {
        version,
        notes,
        page,
        published,
        assets,
    })
}

/// The name of the program file for this machine in a release, for example `clawdot-linux-armv7`.
#[must_use]
pub fn asset_name() -> String {
    let arch = match std::env::consts::ARCH {
        "arm" => "armv7",
        a => a,
    };
    let ext = if cfg!(windows) { ".exe" } else { "" };
    format!("clawdot-{}-{arch}{ext}", std::env::consts::OS)
}

/// Read a `SHA256SUMS` file: lines of `<64 hex digits>  <file name>`.
#[must_use]
pub fn parse_sums(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|l| {
            let (hash, name) = l.trim().split_once(char::is_whitespace)?;
            let name = name.trim().trim_start_matches('*');
            (hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()) && !name.is_empty())
                .then(|| (name.to_owned(), hash.to_ascii_lowercase()))
        })
        .collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if s.len() % 2 != 0 || !s.is_ascii() {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

/// Whether `sig_hex` is the Ed25519 signature of `message` made by the holder of `key`.
#[must_use]
pub fn verify_signature(key: &[u8; 32], message: &[u8], sig_hex: &str) -> bool {
    let Some(sig) = unhex(sig_hex) else {
        return false;
    };
    ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, key)
        .verify(message, &sig)
        .is_ok()
}

fn sha256_hex(data: &[u8]) -> String {
    use std::fmt::Write;
    Sha256::digest(data).iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// An address that may be fetched: https, or this machine (for tests and a local update server).
fn url_ok(u: &str) -> bool {
    u.starts_with("https://") || Endpoint::parse(u).is_ok_and(|e| !e.tls && is_loopback(&e.host))
}

fn is_loopback(host: &str) -> bool {
    host == "localhost" || host == "127.0.0.1" || host == "[::1]"
}

/// Settings for the updater that come from the configuration file.
#[derive(Debug, Clone)]
pub struct UpdateConfig {
    /// Where to ask for the newest release.
    pub url: String,
    /// The maintainer's public key; without it nothing is installed automatically.
    pub public_key: Option<[u8; 32]>,
    /// Whether something restarts the program when it exits (a service manager or the speaker's own supervisor).
    pub supervised: bool,
}

impl UpdateConfig {
    /// Build from the optional configuration values, falling back to the compiled-in address and key.
    ///
    /// # Errors
    /// A plain description of a bad key or address.
    pub fn new(url: Option<&str>, public_key_hex: Option<&str>) -> Result<Self, String> {
        let url = url.map_or(DEFAULT_URL, str::trim).to_owned();
        let ep = Endpoint::parse(&url).map_err(|e| format!("updates.url: {e}"))?;
        if !ep.tls && !is_loopback(&ep.host) {
            return Err("updates.url must be an https:// address".into());
        }
        let key_text = public_key_hex.or(option_env!("CLAWDOT_UPDATE_PUBKEY"));
        let public_key = key_text
            .filter(|k| !k.trim().is_empty())
            .map(|k| {
                unhex(k).and_then(|b| <[u8; 32]>::try_from(b).ok()).ok_or(
                    "updates.public_key must be 64 hexadecimal digits (an Ed25519 public key)",
                )
            })
            .transpose()?;
        Ok(Self {
            url,
            public_key,
            supervised: std::env::var_os("CLAWDOT_SUPERVISED").is_some()
                || std::env::var_os("INVOCATION_ID").is_some(),
        })
    }
}

#[derive(Debug, Clone, Default)]
struct State {
    last_checked: Option<u64>,
    latest: Option<Release>,
    error: Option<String>,
    /// `downloading`, `verifying`, `installing`, `installed`, `restarting`, `failed`; empty when idle.
    phase: String,
    apply_error: Option<String>,
    installed: Option<String>,
}

/// Looks for updates and installs one when told to.
pub struct Updater {
    cfg: UpdateConfig,
    state: Mutex<State>,
    enabled: AtomicBool,
    busy: AtomicBool,
    data_dir: Option<PathBuf>,
    exe: PathBuf,
    connector: Connector,
    /// Run the new program with `--version` before it replaces the old one (off in tests).
    probe: bool,
    /// Exit after installing so the supervisor starts the new version (off in tests).
    exit_after: bool,
}

impl std::fmt::Debug for Updater {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Updater")
            .field("url", &self.cfg.url)
            .finish_non_exhaustive()
    }
}

/// Make the updater of this run, start its daily check, and settle any update that was waiting to prove itself.
/// The caller keeps the returned handle (the core holds it); the check task holds its own clone.
///
/// # Errors
/// The updater could not be made (a TLS configuration failure).
pub fn start(
    cfg: UpdateConfig,
    enabled: bool,
    data_dir: Option<PathBuf>,
) -> Result<Arc<Updater>, String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot find this program: {e}"))?;
    let u = Updater::new(cfg, enabled, data_dir, exe, true, true)?;
    u.load();
    u.settle_pending();
    let u = Arc::new(u);
    tokio::spawn(Arc::clone(&u).run());
    Ok(u)
}

impl Updater {
    /// Turn the daily check on or off (the person's choice in the portal).
    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, AtomicOrdering::Relaxed);
    }
}

struct Busy<'a>(&'a AtomicBool);
impl Drop for Busy<'_> {
    fn drop(&mut self) {
        self.0.store(false, AtomicOrdering::Relaxed);
    }
}

impl Updater {
    fn new(
        cfg: UpdateConfig,
        enabled: bool,
        data_dir: Option<PathBuf>,
        exe: PathBuf,
        probe: bool,
        exit_after: bool,
    ) -> Result<Self, String> {
        Ok(Self {
            cfg,
            state: Mutex::new(State::default()),
            enabled: AtomicBool::new(enabled),
            busy: AtomicBool::new(false),
            data_dir,
            exe,
            connector: Connector::new()?,
            probe,
            exit_after,
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn file(&self, name: &str) -> Option<PathBuf> {
        self.data_dir.as_ref().map(|d| d.join(name))
    }

    /// Wake once an hour; ask for the newest release when it has been a day.
    async fn run(self: Arc<Self>) {
        tokio::time::sleep(Duration::from_secs(90)).await;
        loop {
            let due = self
                .lock()
                .last_checked
                .is_none_or(|t| now_secs().saturating_sub(t) >= CHECK_EVERY_SECS);
            if due && self.enabled.load(AtomicOrdering::Relaxed) {
                let _ = self.check().await;
            }
            tokio::time::sleep(Duration::from_secs(3600)).await;
        }
    }

    /// Fetch a web address with a size limit, following a few redirects (a download address hands over to a file
    /// server) but only to https, or to this machine in tests.
    async fn fetch(
        &self,
        url: &str,
        max: usize,
        accept: &str,
        deadline: Duration,
    ) -> Result<Vec<u8>, String> {
        let mut url = url.to_owned();
        for _ in 0..6 {
            let ep = Endpoint::parse(&url)?;
            if !ep.tls && !is_loopback(&ep.host) {
                return Err("updates are only fetched over https".into());
            }
            let default_port = if ep.tls { 443 } else { 80 };
            let host = if ep.port == default_port {
                ep.host.clone()
            } else {
                format!("{}:{}", ep.host, ep.port)
            };
            let path = if ep.base.is_empty() {
                "/"
            } else {
                ep.base.as_str()
            };
            let req = Request::builder()
                .method(Method::GET)
                .uri(path)
                .header(HOST, host)
                .header(USER_AGENT, format!("clawdot/{CURRENT}"))
                .header(ACCEPT, accept)
                .body(Full::new(Bytes::new()))
                .map_err(|e| format!("bad request: {e}"))?;
            let resp = tokio::time::timeout(deadline, self.connector.request(&ep, req))
                .await
                .map_err(|_| "the update server did not answer in time".to_owned())??;
            let status = resp.status();
            if status.is_redirection() {
                let to = resp
                    .headers()
                    .get(LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .ok_or("the update server sent a redirect with nowhere to go")?;
                url = if to.starts_with('/') {
                    format!(
                        "{}://{}:{}{}",
                        if ep.tls { "https" } else { "http" },
                        ep.host,
                        ep.port,
                        to
                    )
                } else {
                    to.to_owned()
                };
                continue;
            }
            if !status.is_success() {
                return Err(format!("the update server answered {}", status.as_u16()));
            }
            let body =
                tokio::time::timeout(deadline, Limited::new(resp.into_body(), max).collect())
                    .await
                    .map_err(|_| "the download took too long".to_owned())?
                    .map_err(|_| "the download was larger than allowed or broke off".to_owned())?;
            return Ok(body.to_bytes().to_vec());
        }
        Err("the update server redirected too many times".into())
    }

    /// Ask for the newest release now.
    ///
    /// # Errors
    /// A plain description of why it could not be found out; the same text is kept for the portal.
    pub async fn check(&self) -> Result<(), String> {
        if self.busy.swap(true, AtomicOrdering::Relaxed) {
            return Err("a check is already running".into());
        }
        let _busy = Busy(&self.busy);
        let result = self
            .fetch(
                &self.cfg.url,
                MAX_FEED,
                "application/vnd.github+json",
                Duration::from_secs(20),
            )
            .await
            .and_then(|b| parse_release(&String::from_utf8_lossy(&b)));
        {
            let mut s = self.lock();
            s.last_checked = Some(now_secs());
            match &result {
                Ok(r) => {
                    s.latest = Some(r.clone());
                    s.error = None;
                }
                Err(e) => s.error = Some(format!("Could not check for updates: {e}.")),
            }
        }
        self.save();
        result.map(|_| ())
    }

    fn save(&self) {
        let Some(path) = self.file("update.json") else {
            return;
        };
        let s = self.lock();
        let doc = json!({
            "last_checked": s.last_checked,
            "latest": s.latest.as_ref().map(release_json),
        });
        drop(s);
        let tmp = path.with_extension("tmp");
        if std::fs::write(&tmp, doc.to_string()).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }

    fn load(&self) {
        let Some(text) = self
            .file("update.json")
            .and_then(|p| std::fs::read_to_string(p).ok())
        else {
            return;
        };
        let Ok(v) = serde_json::from_str::<Value>(&text) else {
            return;
        };
        let mut s = self.lock();
        s.last_checked = v["last_checked"].as_u64();
        s.latest = v.get("latest").and_then(release_from_json);
    }

    /// What the portal shows.
    #[must_use]
    pub fn status(&self) -> Value {
        let s = self.lock();
        let available = s
            .latest
            .as_ref()
            .is_some_and(|r| is_newer(CURRENT, &r.version));
        let (can_apply, why_not) = match (&s.latest, available) {
            (Some(r), true) => self.can_apply(r),
            _ => (false, None),
        };
        json!({
            "current": CURRENT,
            "enabled": self.enabled.load(AtomicOrdering::Relaxed),
            "source": Endpoint::parse(&self.cfg.url).map_or(String::new(), |e| e.host),
            "checking": self.busy.load(AtomicOrdering::Relaxed),
            "last_checked": s.last_checked,
            "error": s.error,
            "available": available,
            "latest": s.latest.as_ref().map(release_json),
            "can_apply": can_apply,
            "cannot_apply_because": why_not,
            "supervised": self.cfg.supervised,
            "phase": s.phase,
            "apply_error": s.apply_error,
            "installed": s.installed,
        })
    }

    fn can_apply(&self, r: &Release) -> (bool, Option<String>) {
        if self.cfg.public_key.is_none() {
            return (
                false,
                Some("This build has no update key, so an update cannot be checked for authenticity here. Download the new version from the release page instead.".into()),
            );
        }
        let have = |n: &str| r.assets.iter().any(|a| a.name == n);
        if !have(&asset_name()) || !have(SUMS_NAME) || !have(SIG_NAME) {
            return (
                false,
                Some(format!(
                    "The release does not carry a signed file for this device ({}).",
                    asset_name()
                )),
            );
        }
        (true, None)
    }

    /// Start installing the newest release, after the person confirmed. It runs in the background; the portal watches
    /// [`Updater::status`].
    ///
    /// # Errors
    /// A plain reason why nothing was started.
    pub fn start_apply(self: &Arc<Self>, version: &str) -> Result<(), String> {
        let release = {
            let s = self.lock();
            if !s.phase.is_empty() && s.phase != "failed" {
                return Err("An update is already being installed.".into());
            }
            s.latest
                .clone()
                .ok_or("No update has been found yet. Check for updates first.")?
        };
        if release.version != version.trim_start_matches(['v', 'V'])
            || !is_newer(CURRENT, &release.version)
        {
            return Err(
                "That version is not a newer one than this. Check for updates again.".into(),
            );
        }
        if let (false, Some(why)) = self.can_apply(&release) {
            return Err(why);
        }
        {
            let mut s = self.lock();
            s.phase = "downloading".into();
            s.apply_error = None;
        }
        let this = Arc::clone(self);
        tokio::spawn(async move {
            let r = tokio::time::timeout(APPLY_DEADLINE, this.apply(&release)).await;
            let mut s = this.lock();
            match r {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    crate::diag::error(&format!("update to {} failed: {e}", release.version));
                    s.phase = "failed".into();
                    s.apply_error = Some(format!("The update was not installed: {e}"));
                }
                Err(_) => {
                    s.phase = "failed".into();
                    s.apply_error = Some(
                        "The update took too long and was stopped. Nothing was changed.".into(),
                    );
                }
            }
        });
        Ok(())
    }

    fn set_phase(&self, p: &str) {
        self.lock().phase = p.to_owned();
    }

    async fn apply(&self, r: &Release) -> Result<(), String> {
        let key = self.cfg.public_key.ok_or("this build has no update key")?;
        let asset = |name: &str| {
            r.assets
                .iter()
                .find(|a| a.name == name)
                .ok_or_else(|| format!("the release has no {name}"))
        };
        let (bin, sums, sig) = (asset(&asset_name())?, asset(SUMS_NAME)?, asset(SIG_NAME)?);
        let small = Duration::from_secs(60);
        let sums_bytes = self
            .fetch(&sums.url, MAX_SMALL, "application/octet-stream", small)
            .await?;
        let sig_text = self
            .fetch(&sig.url, MAX_SMALL, "application/octet-stream", small)
            .await?;
        self.set_phase("verifying");
        if !verify_signature(&key, &sums_bytes, &String::from_utf8_lossy(&sig_text)) {
            return Err("the list of file hashes is not signed by the project's key".into());
        }
        let expected = parse_sums(&String::from_utf8_lossy(&sums_bytes))
            .remove(&bin.name)
            .ok_or_else(|| format!("the signed list has no hash for {}", bin.name))?;
        self.set_phase("downloading");
        let data = self
            .fetch(
                &bin.url,
                MAX_BINARY,
                "application/octet-stream",
                Duration::from_secs(10 * 60),
            )
            .await?;
        self.set_phase("verifying");
        if sha256_hex(&data) != expected {
            return Err("the downloaded file does not match its signed hash".into());
        }
        self.set_phase("installing");
        self.install(&data, &r.version)?;
        self.set_phase("installed");
        {
            let mut s = self.lock();
            s.installed = Some(r.version.clone());
        }
        crate::diag::warn(&format!(
            "updated to {}; the previous program is kept as .prev",
            r.version
        ));
        if self.exit_after && self.cfg.supervised {
            self.set_phase("restarting");
            tokio::time::sleep(Duration::from_millis(1500)).await;
            std::process::exit(0);
        }
        Ok(())
    }

    /// Put the new program next to the running one, check that it starts, and swap them. The old one stays as
    /// `.prev`. A failure at any step leaves the running program exactly as it was.
    fn install(&self, data: &[u8], version: &str) -> Result<(), String> {
        let name = self
            .exe
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or("cannot tell where this program is")?
            .to_owned();
        let new = self.exe.with_file_name(format!("{name}.new"));
        let prev = self.exe.with_file_name(format!("{name}.prev"));
        std::fs::write(&new, data)
            .map_err(|e| format!("cannot write the new program next to this one: {e}"))?;
        let cleanup = |e: String| {
            let _ = std::fs::remove_file(&new);
            e
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&new, std::fs::Permissions::from_mode(0o755))
                .map_err(|e| cleanup(format!("cannot make the new program runnable: {e}")))?;
        }
        if self.probe {
            let out = std::process::Command::new(&new)
                .arg("--version")
                .output()
                .map_err(|e| {
                    cleanup(format!(
                        "the new program does not start on this device: {e}"
                    ))
                })?;
            let said = String::from_utf8_lossy(&out.stdout);
            if !out.status.success() || !said.contains(version) {
                return Err(cleanup(
                    "the new program did not report the expected version".into(),
                ));
            }
        }
        if let Some(p) = self.file(PENDING_FILE) {
            let _ = std::fs::write(p, version);
        }
        let _ = std::fs::remove_file(&prev);
        std::fs::rename(&self.exe, &prev)
            .map_err(|e| cleanup(format!("cannot set the old program aside: {e}")))?;
        if let Err(e) = std::fs::rename(&new, &self.exe) {
            let _ = std::fs::rename(&prev, &self.exe);
            return Err(cleanup(format!("cannot put the new program in place: {e}")));
        }
        Ok(())
    }

    /// At start: an update that was just installed must run for a while before the old program is let go.
    fn settle_pending(&self) {
        let Some(path) = self.file(PENDING_FILE) else {
            return;
        };
        let Ok(version) = std::fs::read_to_string(&path) else {
            return;
        };
        if version.trim() != CURRENT {
            let _ = std::fs::remove_file(&path);
            return;
        }
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(PROVE_SECS));
            let _ = std::fs::remove_file(&path);
        });
    }
}

/// Read back what [`release_json`] wrote. Anything that is not a version number or an https address is dropped.
fn release_from_json(v: &Value) -> Option<Release> {
    let version = v["version"]
        .as_str()
        .filter(|t| parse_version(t).is_some())?
        .to_owned();
    let text = |k: &str, max: usize| {
        v[k].as_str()
            .unwrap_or("")
            .chars()
            .take(max)
            .collect::<String>()
    };
    Some(Release {
        version,
        notes: text("notes", 4000),
        page: v["page"]
            .as_str()
            .filter(|u| url_ok(u))
            .unwrap_or("")
            .to_owned(),
        published: text("published", 40),
        assets: v["assets"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| {
                        let (name, url) = (x["name"].as_str()?, x["url"].as_str()?);
                        (url_ok(url) && !name.is_empty() && name.len() < 120).then(|| Asset {
                            name: name.to_owned(),
                            url: url.to_owned(),
                            size: x["size"].as_u64().unwrap_or(0),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default(),
    })
}

fn release_json(r: &Release) -> Value {
    json!({
        "version": r.version,
        "notes": r.notes,
        "page": r.page,
        "published": r.published,
        "assets": r.assets.iter().map(|a| json!({"name": a.name, "url": a.url, "size": a.size})).collect::<Vec<_>>(),
    })
}

/// Whether `path` is an update that has been installed but not yet proven (used by the supervisor script).
#[must_use]
pub fn pending_marker(data_dir: &Path) -> bool {
    data_dir.join(PENDING_FILE).exists()
}

#[cfg(test)]
mod tests;
