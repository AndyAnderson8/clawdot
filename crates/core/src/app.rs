//! The application core: the inbox, endpoints, and reply queue behind one lock, with every operation a
//! front end needs. The HTTP server and the voice session are thin layers over this, so the logic is
//! testable without sockets or audio.

use crate::admin::{Admin, AdminError, Mode as AdminMode};
use crate::inbox::adapter::Adapter;
use crate::inbox::endpoint::{self, Endpoint, Endpoints, QuietHours};
use crate::inbox::event::{Event, EventState};
use crate::inbox::queue::{FilterReason, Inbox, InboxLimits, Ingest, SourceStatus};
use crate::inbox::reply::{self, Reply, ReplyKind, ReplyQueue};
use crate::limit::{FailureBudget, FailureLimiter};
use crate::sched::{Clock, Ts};
use crate::store::{JsonStore, StoreError};
use std::collections::{BTreeMap, HashMap};
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use tokio::sync::Notify;

mod announce;
mod ask;
pub mod persist;
mod portal;
pub(crate) use announce::AuditAs;
pub use announce::{AnnounceError, Announcement, Caller, outcome_json};
pub use ask::{AskEnd, AskError, AskTicket, MAX_PENDING_ASKS};
use persist::Dirty;
pub use portal::{Fail, Job, Op, SchedOp};

/// The address of a request from this machine, for tests and local callers that have no peer to name.
pub const LOCALHOST_PEER: IpAddr = IpAddr::V4(std::net::Ipv4Addr::LOCALHOST);

/// Where the saves stood when a change began (see [`Core::save_mark`]).
#[derive(Debug, Clone, Copy)]
pub struct SaveMark {
    submitted: u64,
    errors: u64,
}

/// Adapters available to endpoints, by name.
#[derive(Debug, Clone, Default)]
pub struct Adapters(BTreeMap<String, Adapter>);

impl Adapters {
    /// The built-in adapters (`claude-code`, `generic`).
    #[must_use]
    pub fn builtin() -> Self {
        let mut a = Self::default();
        for adapter in [Adapter::claude_code(), Adapter::generic()]
            .into_iter()
            .flatten()
        {
            a.insert(adapter);
        }
        a
    }

    /// Add or replace an adapter.
    pub fn insert(&mut self, adapter: Adapter) {
        self.0.insert(adapter.name.clone(), adapter);
    }

    /// Look one up.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Adapter> {
        self.0.get(name)
    }
}

/// Operational limits for the front end (REQ-SEC-004, REQ-NFR-110).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limits {
    /// Largest accepted request body, in bytes.
    pub max_body: usize,
    /// Requests an endpoint may burst.
    pub hook_burst: f64,
    /// Sustained requests per second per endpoint.
    pub hook_per_sec: f64,
    /// Failed authentications one source address may make at once before its further attempts are refused.
    pub auth_fail_burst: f64,
    /// Sustained failed authentications per second, per source address.
    pub auth_fail_per_sec: f64,
    /// The backstop over all sources together: failures that may burst. Larger than one source can spend.
    pub auth_fail_overall_burst: f64,
    /// The backstop's sustained rate per second.
    pub auth_fail_overall_per_sec: f64,
    /// Source addresses remembered at once; the table never grows past this.
    pub auth_fail_sources: usize,
    /// Replies held for bridges.
    pub max_pending_replies: usize,
    /// Seconds a reply waits for a bridge before expiring.
    pub reply_ttl_secs: i64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_body: 256 * 1024,
            hook_burst: 20.0,
            hook_per_sec: 5.0,
            auth_fail_burst: 20.0,
            auth_fail_per_sec: 1.0,
            auth_fail_overall_burst: 100.0,
            auth_fail_overall_per_sec: 5.0,
            auth_fail_sources: 64,
            max_pending_replies: 64,
            reply_ttl_secs: 24 * 3600,
        }
    }
}

/// Where state is persisted.
#[derive(Debug, Clone)]
pub struct Stores {
    /// Endpoint list (tokens hashed).
    pub endpoints: JsonStore,
    /// Inbox events.
    pub inbox: JsonStore,
    /// Replies awaiting bridges.
    pub replies: JsonStore,
    /// The web UI administrator setting (password hash or open mode).
    pub admin: JsonStore,
    /// The keys pasted into the portal (see [`crate::secrets`]).
    pub secrets: JsonStore,
}

impl Stores {
    /// Stores under a directory.
    #[must_use]
    pub fn in_dir(dir: &std::path::Path) -> Self {
        Self {
            endpoints: JsonStore::new(dir.join("endpoints.json"), endpoint::SCHEMA),
            inbox: JsonStore::new(dir.join("inbox.json"), crate::inbox::queue::SCHEMA),
            replies: JsonStore::new(dir.join("replies.json"), reply::SCHEMA),
            admin: JsonStore::new(dir.join("admin.json"), crate::admin::SCHEMA),
            secrets: JsonStore::new(dir.join("secrets.json"), crate::secrets::SCHEMA),
        }
    }
}

/// What happened to a posted event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookOutcome {
    /// Queued.
    Accepted(u64),
    /// Merged into a recent event.
    Coalesced(u64),
    /// Deliberately dropped by the endpoint's filter.
    Filtered(FilterReason),
}

/// Why a posted event was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookError {
    /// Unknown endpoint or wrong token (indistinguishable on purpose).
    Unauthorized,
    /// Too many requests.
    RateLimited,
    /// The body is not valid JSON.
    BadJson,
    /// The adapter has no rule for this payload.
    Unmappable,
    /// The endpoint names an adapter that does not exist.
    NoAdapter,
}

/// Why a voice-side reply could not be queued.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplyError {
    /// There is nothing announced to answer.
    NothingToReplyTo,
    /// The bridge queue is full.
    QueueFull,
}

/// The result of answering an announced event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replied {
    /// The event answered.
    pub event_id: u64,
    /// Its headline.
    pub title: String,
    /// Who the speaker named it as (the endpoint's spoken name or name).
    pub origin: String,
}

/// An event to speak, and how many others wait behind it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NextAnnouncement {
    /// The event to announce.
    pub event: Event,
    /// Others waiting.
    pub more_waiting: usize,
}

/// A token bucket; `last` is a monotonic reading ([`Clock::monotonic_secs`]), so a step of the wall clock
/// cannot refill it.
#[derive(Debug)]
struct Bucket {
    tokens: f64,
    last: Ts,
}

fn take(bucket: &mut Bucket, now: Ts, burst: f64, per_sec: f64) -> bool {
    let dt = (now - bucket.last).max(0) as f64;
    bucket.tokens = (bucket.tokens + dt * per_sec).min(burst);
    bucket.last = now;
    if bucket.tokens >= 1.0 {
        bucket.tokens -= 1.0;
        true
    } else {
        false
    }
}

#[derive(Debug)]
struct Inner {
    endpoints: Endpoints,
    inbox: Inbox,
    replies: ReplyQueue,
    buckets: HashMap<String, Bucket>,
    /// Failed attempts, counted per source address (REQ-SEC-004).
    auth_failures: FailureLimiter,
}

/// Test phrases that may wait to be spoken.
pub const MAX_PENDING_SPEECH: usize = 4;
/// Longest test phrase.
pub const MAX_SPEECH_CHARS: usize = 200;

/// The shared core.
pub struct Core {
    clock: Arc<dyn Clock + Send + Sync>,
    adapters: Adapters,
    limits: Limits,
    stores: Option<Stores>,
    /// Writes the stores in the background; `None` without storage, or when no thread could be started (then
    /// writes happen inline).
    writer: Option<Arc<persist::Writer>>,
    inner: Mutex<Inner>,
    notify: Notify,
    admin: Mutex<crate::admin::Admin>,
    status: Mutex<serde_json::Value>,
    speech: Mutex<std::collections::VecDeque<(String, Option<String>)>>,
    tools: Mutex<serde_json::Value>,
    settings: Mutex<Option<crate::settings::SettingsState>>,
    jobs: Mutex<Option<tokio::sync::mpsc::Sender<portal::Job>>>,
    audit: std::sync::OnceLock<Arc<crate::audit::AuditLog>>,
    notes: std::sync::OnceLock<Arc<crate::notes::NoteStore>>,
    asks: Mutex<Vec<ask::PendingAsk>>,
    secrets: crate::secrets::Secrets,
    env: crate::env::Env,
    /// The machine's host name, read once by [`Core::with_env`]; the Host check reads this, not the system.
    machine: Option<String>,
    speech_handles: crate::speechio::SpeechHandles,
    speech_origin: std::sync::OnceLock<crate::speech::SpeechOrigin>,
    /// Saves of the core's own three files that could not be made inline or encoded.
    save_errors: AtomicU64,
    #[cfg(feature = "hive")]
    hive: std::sync::OnceLock<crate::hive::Hive>,
    #[cfg(feature = "tls")]
    updater: std::sync::OnceLock<Arc<crate::update::Updater>>,
}

impl std::fmt::Debug for Core {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Core")
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl Core {
    /// An in-memory core (tests and demos).
    #[must_use]
    pub fn in_memory(
        clock: Arc<dyn Clock + Send + Sync>,
        adapters: Adapters,
        limits: Limits,
    ) -> Self {
        Self::build(
            clock,
            adapters,
            limits,
            None,
            Endpoints::default(),
            Inbox::new(InboxLimits::default()),
            ReplyQueue::new(limits.max_pending_replies, limits.reply_ttl_secs),
        )
    }

    /// A core whose state lives in `stores`, restoring whatever is there.
    ///
    /// # Errors
    /// Storage errors, including files written by a newer version.
    pub fn open(
        clock: Arc<dyn Clock + Send + Sync>,
        adapters: Adapters,
        limits: Limits,
        stores: Stores,
    ) -> Result<Self, StoreError> {
        let endpoints: Endpoints = stores.endpoints.load()?;
        let inbox = Inbox::load(&stores.inbox, InboxLimits::default())?;
        let replies = ReplyQueue::load(
            &stores.replies,
            limits.max_pending_replies,
            limits.reply_ttl_secs,
        )?;
        let admin = crate::admin::Admin::open(stores.admin.clone())?;
        let mut core = Self::build(
            clock,
            adapters,
            limits,
            Some(stores),
            endpoints,
            inbox,
            replies,
        );
        core.admin = Mutex::new(admin);
        Ok(core)
    }

    fn build(
        clock: Arc<dyn Clock + Send + Sync>,
        adapters: Adapters,
        limits: Limits,
        stores: Option<Stores>,
        endpoints: Endpoints,
        inbox: Inbox,
        replies: ReplyQueue,
    ) -> Self {
        let mono = clock.monotonic_secs();
        let writer = stores
            .as_ref()
            .and_then(|_| persist::Writer::start().map(Arc::new));
        let secrets = stores
            .as_ref()
            .map_or_else(crate::secrets::Secrets::none, |s| {
                crate::secrets::Secrets::at(s.secrets.clone())
            });
        Self {
            clock,
            adapters,
            limits,
            writer,
            stores,
            inner: Mutex::new(Inner {
                endpoints,
                inbox,
                replies,
                buckets: HashMap::new(),
                auth_failures: FailureLimiter::new(
                    FailureBudget {
                        burst: limits.auth_fail_burst,
                        per_sec: limits.auth_fail_per_sec,
                    },
                    FailureBudget {
                        burst: limits.auth_fail_overall_burst,
                        per_sec: limits.auth_fail_overall_per_sec,
                    },
                    limits.auth_fail_sources,
                    mono,
                ),
            }),
            notify: Notify::new(),
            admin: Mutex::new(crate::admin::Admin::new()),
            status: Mutex::new(serde_json::Value::Null),
            speech: Mutex::new(std::collections::VecDeque::new()),
            tools: Mutex::new(serde_json::Value::Null),
            settings: Mutex::new(None),
            jobs: Mutex::new(None),
            audit: std::sync::OnceLock::new(),
            notes: std::sync::OnceLock::new(),
            asks: Mutex::new(Vec::new()),
            secrets,
            env: crate::env::Env::empty(),
            machine: None,
            speech_handles: crate::speechio::SpeechHandles::default(),
            speech_origin: std::sync::OnceLock::new(),
            save_errors: AtomicU64::new(0),
            #[cfg(feature = "hive")]
            hive: std::sync::OnceLock::new(),
            #[cfg(feature = "tls")]
            updater: std::sync::OnceLock::new(),
        }
    }

    /// Keep the started hive for the life of this core (set once; a later call is ignored).
    #[cfg(feature = "hive")]
    pub fn set_hive(&self, hive: crate::hive::Hive) {
        let _ = self.hive.set(hive);
    }

    /// The hive, when it was started.
    #[cfg(feature = "hive")]
    #[must_use]
    pub fn hive(&self) -> Option<&crate::hive::Hive> {
        self.hive.get()
    }

    /// Keep the running updater for the life of this core (set once; a later call is ignored).
    #[cfg(feature = "tls")]
    pub fn set_updater(&self, updater: Arc<crate::update::Updater>) {
        let _ = self.updater.set(updater);
    }

    /// The updater, when updates are running on this device.
    #[cfg(feature = "tls")]
    #[must_use]
    pub fn updater(&self) -> Option<&Arc<crate::update::Updater>> {
        self.updater.get()
    }

    /// The speech clients' voice and language registry, owned by this core.
    #[must_use]
    pub fn speech_handles(&self) -> &crate::speechio::SpeechHandles {
        &self.speech_handles
    }

    /// Remember which speech server the portal may ask for its voices (set once at start-up; a later call is
    /// ignored).
    pub fn set_speech_origin(&self, origin: crate::speech::SpeechOrigin) {
        let _ = self.speech_origin.set(origin);
    }

    /// The speech server set up at start, with the key it would use now.
    #[must_use]
    pub fn speech_source(&self) -> Option<crate::speech::SpeechSource> {
        self.speech_origin
            .get()
            .map(|o| o.resolve(&self.secrets, &self.env))
    }

    /// Give this core the environment snapshot taken at start-up (see [`crate::env`]). A core that is not
    /// given one sees no variables, so tests never depend on the machine they run on.
    #[must_use]
    pub fn with_env(mut self, env: crate::env::Env) -> Self {
        self.machine = crate::hostguard::machine_name(&env);
        self.env = env;
        self
    }

    /// Set the machine host name the portal answers to, instead of the one read from the system (tests, embedding).
    #[must_use]
    pub fn with_machine_name(mut self, name: Option<&str>) -> Self {
        self.machine = name.map(str::to_owned);
        self
    }

    /// The machine's host name as read at start-up, if it could be.
    #[must_use]
    pub fn machine_name(&self) -> Option<&str> {
        self.machine.as_deref()
    }

    /// The environment as it was at start-up. Look variables up here, never in the process.
    #[must_use]
    pub fn env(&self) -> &crate::env::Env {
        &self.env
    }

    /// The keys pasted into the portal, owned by this core.
    #[must_use]
    pub fn secrets(&self) -> &crate::secrets::Secrets {
        &self.secrets
    }

    /// The current time.
    #[must_use]
    pub fn now(&self) -> Ts {
        self.clock.now()
    }

    /// Whether an adapter with this name exists.
    #[must_use]
    pub fn has_adapter(&self, name: &str) -> bool {
        self.adapters.get(name).is_some()
    }

    /// The configured limits.
    #[must_use]
    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    /// Wakes whenever the inbox or reply queue changes.
    #[must_use]
    pub fn changed(&self) -> &Notify {
        &self.notify
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Save the stores `dirty` names. The state is serialised here, under the lock; the write itself happens on
    /// the writer thread (see [`persist`]), so the lock and the runtime do not wait for flash.
    fn persist(&self, inner: &Inner, dirty: Dirty) {
        let Some(s) = &self.stores else { return };
        if dirty.endpoints {
            self.queue_write(&s.endpoints, s.endpoints.encode(&inner.endpoints));
        }
        if dirty.inbox {
            self.queue_write(&s.inbox, inner.inbox.encode(&s.inbox));
        }
        if dirty.replies {
            self.queue_write(&s.replies, inner.replies.encode(&s.replies));
        }
    }

    fn queue_write(&self, store: &JsonStore, bytes: Result<Vec<u8>, StoreError>) {
        // A failed write must not lose the in-memory event, but it must not be silent either, and the caller
        // that waits for it must hear that it failed.
        let bytes = match bytes {
            Ok(b) => b,
            Err(e) => {
                crate::diag::error(&format!("could not save {}: {e}", store.path().display()));
                self.save_errors.fetch_add(1, Ordering::SeqCst);
                return;
            }
        };
        match &self.writer {
            Some(w) => {
                w.submit(store, bytes);
            }
            None => {
                if let Err(e) = store.write(&bytes) {
                    crate::diag::error(&format!("could not save {}: {e}", store.path().display()));
                    self.save_errors.fetch_add(1, Ordering::SeqCst);
                }
            }
        }
    }

    /// Note where the saves stand, before a change that will be saved. Hand it to [`Core::saved_since`] after
    /// the change.
    #[must_use]
    pub fn save_mark(&self) -> SaveMark {
        SaveMark {
            submitted: self.writer.as_ref().map_or(0, |w| w.mark()),
            errors: self.save_errors.load(Ordering::SeqCst),
        }
    }

    /// Resolves once every change made to the endpoints, inbox and replies since `mark` is on disk. The HTTP layer
    /// awaits this before it answers a webhook, announcement, acknowledgement or endpoint change, so one reported
    /// as accepted survives a power cut (REQ-INB-004) and one that could not be saved is not reported as a success.
    /// Returns at once when nothing is waiting or the core has no storage.
    ///
    /// # Errors
    /// [`persist::SaveError`] when a write failed. The change stays in memory (it is still announced, answered or
    /// delivered), but it will not survive a restart.
    pub async fn saved_since(&self, mark: SaveMark) -> Result<(), persist::SaveError> {
        if self.save_errors.load(Ordering::SeqCst) != mark.errors {
            return Err(persist::SaveError { disk_full: false });
        }
        let (Some(w), Some(s)) = (&self.writer, &self.stores) else {
            return Ok(());
        };
        let ours = |p: &std::path::Path| {
            p == s.endpoints.path() || p == s.inbox.path() || p == s.replies.path()
        };
        w.flushed_since(mark.submitted, &ours).await?;
        // An encoding or inline failure that happened while the writer was busy.
        if self.save_errors.load(Ordering::SeqCst) != mark.errors {
            return Err(persist::SaveError { disk_full: false });
        }
        Ok(())
    }

    /// The background writer, for stores outside the core's three (timers, usage counters) that hand it bytes.
    /// `None` without storage, or when no thread could be started: those callers then write inline.
    #[must_use]
    pub fn writer(&self) -> Option<Arc<persist::Writer>> {
        self.writer.clone()
    }

    /// Block until every change made so far is on disk. For tests and shutdown paths, not for the runtime.
    pub fn flush_blocking(&self) {
        if let Some(w) = &self.writer {
            w.flush_blocking();
        }
    }

    // ----- endpoint management (used by the portal and CLI) -----

    /// Create an endpoint; returns it with its plaintext token, shown once.
    ///
    /// # Errors
    /// See [`endpoint::EndpointError`].
    pub fn create_endpoint(
        &self,
        name: &str,
        adapter: &str,
    ) -> Result<(Endpoint, String), endpoint::EndpointError> {
        let now = self.clock.now();
        let mut g = self.lock();
        let made = g.endpoints.create(name, adapter, now)?;
        self.persist(&g, Dirty::ENDPOINTS);
        Ok(made)
    }

    /// Replace an endpoint's token.
    ///
    /// # Errors
    /// See [`endpoint::EndpointError`].
    pub fn rotate_endpoint(&self, name: &str) -> Result<String, endpoint::EndpointError> {
        let mut g = self.lock();
        let t = g.endpoints.rotate(name)?;
        self.persist(&g, Dirty::ENDPOINTS);
        Ok(t)
    }

    /// Delete an endpoint, the replies waiting for its bridge and its events that are still open. Those, and
    /// its rate-limit bucket, are keyed by the endpoint's name, so an endpoint made later under the same name would otherwise inherit
    /// the old agent's instructions and have its queued events announced and answered.
    ///
    /// # Errors
    /// See [`endpoint::EndpointError`].
    pub fn revoke_endpoint(&self, name: &str) -> Result<(), endpoint::EndpointError> {
        let mut g = self.lock();
        g.endpoints.revoke(name)?;
        // The rate-limit bucket is keyed by name too; a new endpoint starts with a full burst.
        g.buckets.remove(name);
        let replies = g.replies.drop_for(name);
        let events = g.inbox.drop_open_for(name);
        self.persist(
            &g,
            Dirty {
                endpoints: true,
                inbox: events > 0,
                replies: replies > 0,
            },
        );
        Ok(())
    }

    /// Change an endpoint's filter rules.
    pub fn update_filter(&self, name: &str, f: impl FnOnce(&mut endpoint::EndpointFilter)) -> bool {
        let mut g = self.lock();
        let found = g
            .endpoints
            .get_mut(name)
            .map(|e| f(&mut e.filter))
            .is_some();
        if found {
            self.persist(&g, Dirty::ENDPOINTS);
        }
        found
    }

    /// All endpoints.
    #[must_use]
    pub fn endpoints(&self) -> Vec<Endpoint> {
        self.lock().endpoints.all().to_vec()
    }

    // ----- inbound events -----

    fn authenticate(
        &self,
        g: &mut Inner,
        name: &str,
        token: &str,
        peer: IpAddr,
        mono: i64,
    ) -> Result<Endpoint, HookError> {
        if let Some(ep) = g.endpoints.authenticate(name, token) {
            return Ok(ep.clone());
        }
        let ok = g.auth_failures.spend(peer, mono);
        Err(if ok {
            HookError::Unauthorized
        } else {
            HookError::RateLimited
        })
    }

    /// Handle a posted event: authenticate, rate-limit, parse, map, filter, queue, persist. `peer` is the
    /// address the request came from; a wrong token is counted against it alone.
    ///
    /// # Errors
    /// See [`HookError`].
    pub fn ingest_hook(
        &self,
        name: &str,
        token: &str,
        peer: IpAddr,
        body: &[u8],
    ) -> Result<HookOutcome, HookError> {
        let (now, mono) = (self.clock.now(), self.clock.monotonic_secs());
        let mut g = self.lock();
        let ep = self.authenticate(&mut g, name, token, peer, mono)?;
        let bucket = g.buckets.entry(ep.name.clone()).or_insert(Bucket {
            tokens: self.limits.hook_burst,
            last: mono,
        });
        if !take(
            bucket,
            mono,
            self.limits.hook_burst,
            self.limits.hook_per_sec,
        ) {
            return Err(HookError::RateLimited);
        }
        let payload: serde_json::Value =
            serde_json::from_slice(body).map_err(|_| HookError::BadJson)?;
        let adapter = self.adapters.get(&ep.adapter).ok_or(HookError::NoAdapter)?;
        let mapped = adapter.map(&payload).ok_or(HookError::Unmappable)?;
        let outcome = match g.inbox.ingest(now, &ep, mapped) {
            Ingest::Accepted(id) => HookOutcome::Accepted(id),
            Ingest::Coalesced(id) => HookOutcome::Coalesced(id),
            Ingest::Filtered(r) => HookOutcome::Filtered(r),
        };
        if !matches!(outcome, HookOutcome::Filtered(_)) {
            self.persist(&g, Dirty::INBOX);
            self.notify.notify_waiters();
        }
        Ok(outcome)
    }

    // ----- bridge side -----

    /// Authenticate a bridge and return the unacknowledged replies after `after`.
    ///
    /// # Errors
    /// [`HookError::Unauthorized`] or [`HookError::RateLimited`].
    pub fn bridge_pending(
        &self,
        name: &str,
        token: &str,
        peer: IpAddr,
        after: u64,
    ) -> Result<Vec<Reply>, HookError> {
        let (now, mono) = (self.clock.now(), self.clock.monotonic_secs());
        let mut g = self.lock();
        let ep = self.authenticate(&mut g, name, token, peer, mono)?;
        g.replies.expire(now);
        Ok(g.replies
            .pending_for(&ep.name, after)
            .into_iter()
            .cloned()
            .collect())
    }

    /// Authenticate a bridge and confirm it handled a reply.
    ///
    /// # Errors
    /// [`HookError::Unauthorized`] or [`HookError::RateLimited`].
    pub fn bridge_ack(
        &self,
        name: &str,
        token: &str,
        peer: IpAddr,
        reply_id: u64,
    ) -> Result<bool, HookError> {
        let mono = self.clock.monotonic_secs();
        let mut g = self.lock();
        let ep = self.authenticate(&mut g, name, token, peer, mono)?;
        let acked = g.replies.ack(&ep.name, reply_id);
        if acked {
            self.persist(&g, Dirty::REPLIES);
        }
        Ok(acked)
    }

    /// Whether a bridge's credentials are valid (used before opening a stream).
    ///
    /// # Errors
    /// [`HookError::Unauthorized`] or [`HookError::RateLimited`].
    pub fn bridge_check(&self, name: &str, token: &str, peer: IpAddr) -> Result<(), HookError> {
        let mono = self.clock.monotonic_secs();
        let mut g = self.lock();
        self.authenticate(&mut g, name, token, peer, mono)
            .map(|_| ())
    }

    // ----- web UI administration (REQ-CFG-018..024) -----

    fn admin_lock(&self) -> MutexGuard<'_, Admin> {
        self.admin.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Spend one attempt from `peer`'s authentication budget (and the backstop over everyone). False when either
    /// is used up.
    fn spend_attempt(&self, peer: IpAddr) -> bool {
        let mono = self.clock.monotonic_secs();
        self.lock().auth_failures.spend(peer, mono)
    }

    /// Give back an attempt that turned out to be a success, so signing in with the right password costs nothing.
    fn refund_attempt(&self, peer: IpAddr) {
        self.lock().auth_failures.refund(peer);
    }

    /// Tell the administrator logic where the core listens (decides whether open mode is allowed).
    pub fn set_listen_ip(&self, ip: IpAddr) {
        self.admin_lock().set_listen_ip(ip);
    }

    /// The administrator state the page needs to decide what to show.
    #[must_use]
    pub fn admin_state(&self) -> serde_json::Value {
        let mono = self.clock.monotonic_secs();
        let mut a = self.admin_lock();
        serde_json::json!({
            "mode": a.mode().label(),
            "legacy": a.legacy(),
            "open_allowed": a.open_allowed(),
            "reset_window_secs": a.window_left(mono),
        })
    }

    /// Which mode the administrator setting is in.
    #[must_use]
    pub fn admin_mode(&self) -> AdminMode {
        self.admin_lock().mode()
    }

    /// Whether a request may use the administrator API. A wrong session counts against the caller's address.
    ///
    /// # Errors
    /// [`AdminError::SetupRequired`], [`AdminError::NotPrivate`], [`AdminError::Unauthorized`], or
    /// [`AdminError::RateLimited`].
    pub fn check_admin(&self, bearer: Option<&str>, peer: IpAddr) -> Result<(), AdminError> {
        let mono = self.clock.monotonic_secs();
        let r = self.admin_lock().authorize(bearer, peer, mono);
        match r {
            Err(AdminError::Unauthorized) if !self.spend_attempt(peer) => {
                Err(AdminError::RateLimited)
            }
            r => r,
        }
    }

    /// Whether setup may proceed from `peer`. Every call spends an attempt from the peer's budget (call this
    /// before doing any expensive hashing).
    ///
    /// # Errors
    /// See [`AdminError`].
    pub fn admin_check_setup(&self, peer: IpAddr) -> Result<(), AdminError> {
        if !self.spend_attempt(peer) {
            return Err(AdminError::RateLimited);
        }
        self.admin_lock().check_setup(peer)
    }

    /// Finish setup in open mode.
    ///
    /// # Errors
    /// See [`AdminError`].
    pub fn admin_setup_open(&self, peer: IpAddr) -> Result<(), AdminError> {
        self.admin_lock().setup_open(peer)
    }

    /// Finish setup with a password hash (from [`crate::admin::hash_password`]); returns a session token.
    ///
    /// # Errors
    /// See [`AdminError`].
    pub fn admin_setup_password(&self, peer: IpAddr, hash: String) -> Result<String, AdminError> {
        let mono = self.clock.monotonic_secs();
        self.admin_lock().setup_password(peer, hash, mono)
    }

    /// Start a sign-in from `peer`: spends an attempt from its budget and returns the stored hash to verify
    /// (do that off the async thread, it is deliberately slow). The attempt is spent before the check so that
    /// many sign-ins at once cannot all pass a budget that has one attempt left; [`Core::admin_login_finish`]
    /// gives it back when the password was right.
    ///
    /// # Errors
    /// [`AdminError::RateLimited`]; [`AdminError::SetupRequired`] or [`AdminError::Unauthorized`] when there
    /// is no password to check.
    pub fn admin_login_begin(&self, peer: IpAddr) -> Result<String, AdminError> {
        if !self.spend_attempt(peer) {
            return Err(AdminError::RateLimited);
        }
        let a = self.admin_lock();
        let stored = match a.mode() {
            AdminMode::Setup => Err(AdminError::SetupRequired),
            _ => a.password_hash().ok_or(AdminError::Unauthorized),
        };
        drop(a);
        if stored.is_err() {
            // Nothing was guessed: there was no password to check.
            self.refund_attempt(peer);
        }
        stored
    }

    /// Open a session after the password checked out, and give back the attempt [`Core::admin_login_begin`]
    /// spent for `peer`.
    ///
    /// # Errors
    /// If the OS cannot supply random bytes.
    pub fn admin_login_finish(&self, peer: IpAddr) -> Result<String, AdminError> {
        let mono = self.clock.monotonic_secs();
        self.refund_attempt(peer);
        self.admin_lock().new_session(mono)
    }

    /// End the session `bearer`.
    pub fn admin_logout(&self, bearer: &str) {
        self.admin_lock().end_session(bearer);
    }

    /// Start a password change from `peer` (REQ-CFG-024): spends an attempt from its budget and returns the
    /// credential the caller must prove it knows, to verify off the async thread (deliberately slow). `None` in
    /// open mode, where there is no password to prove and the private address is the credential. The caller has
    /// already passed [`Core::check_admin`].
    ///
    /// # Errors
    /// [`AdminError::RateLimited`]; [`AdminError::SetupRequired`] while no setting exists.
    pub fn admin_password_begin(&self, peer: IpAddr) -> Result<Option<String>, AdminError> {
        if !self.spend_attempt(peer) {
            return Err(AdminError::RateLimited);
        }
        let a = self.admin_lock();
        let stored = match a.mode() {
            AdminMode::Setup => Err(AdminError::SetupRequired),
            _ => Ok(a.password_hash()),
        };
        drop(a);
        if !matches!(stored, Ok(Some(_))) {
            // Open mode (or no setting): nothing was guessed.
            self.refund_attempt(peer);
        }
        stored
    }

    /// Replace the administrator password (the caller has passed [`Core::check_admin`], proved the current
    /// password against what [`Core::admin_password_begin`] returned, and hashed the new one). Ends every session
    /// and returns a new one. `proved` is that credential: if the stored one changed meanwhile (another change
    /// got in first), nothing is replaced. The attempt `begin` spent is given back. Setting a password in open
    /// mode, and changing one, are written to the audit log and the problem log.
    ///
    /// # Errors
    /// [`AdminError::WrongCurrent`] when the stored credential is no longer the one that was checked; storage and
    /// randomness failures.
    pub fn admin_set_password(
        &self,
        peer: IpAddr,
        proved: Option<&str>,
        hash: String,
    ) -> Result<String, AdminError> {
        let mono = self.clock.monotonic_secs();
        let mut a = self.admin_lock();
        if a.password_hash().as_deref() != proved {
            return Err(AdminError::WrongCurrent);
        }
        let r = a.set_password(hash, mono);
        drop(a);
        self.refund_attempt(peer);
        match (&r, proved) {
            (Ok(_), None) => self.audit_admin_event(
                "admin_password",
                "administrator password set while the portal was open",
                "password set in open mode",
                true,
            ),
            (Ok(_), Some(_)) => self.audit_admin_event(
                "admin_password",
                "administrator password changed",
                "password changed",
                true,
            ),
            (Err(_), _) => self.audit_admin_event(
                "admin_password",
                "administrator password change failed",
                "password change failed",
                false,
            ),
        }
        r
    }

    /// Open the local reset window on a device's request (REQ-CFG-069). Returns the seconds it is open.
    ///
    /// # Errors
    /// [`AdminError::NoPassword`] unless a password is set; [`AdminError::RateLimited`] during the cooldown.
    pub fn admin_open_reset_window(&self) -> Result<i64, AdminError> {
        let mono = self.clock.monotonic_secs();
        let r = self.admin_lock().open_window(mono);
        match &r {
            Ok(_) => self.audit_admin("reset window opened", true),
            Err(_) => self.audit_admin("reset window refused", false),
        }
        r
    }

    /// Start a reset through the window: spends an attempt and checks the peer and the window.
    ///
    /// # Errors
    /// [`AdminError::RateLimited`], [`AdminError::NotPrivate`], [`AdminError::WindowClosed`],
    /// [`AdminError::NoPassword`].
    pub fn admin_window_begin(&self, peer: IpAddr) -> Result<(), AdminError> {
        if !self.spend_attempt(peer) {
            return Err(AdminError::RateLimited);
        }
        let mono = self.clock.monotonic_secs();
        self.admin_lock().check_window(peer, mono)
    }

    /// Finish a reset through the window (REQ-CFG-069). Returns a new session.
    ///
    /// # Errors
    /// As [`Core::admin_window_begin`], plus storage and randomness failures.
    pub fn admin_window_finish(&self, peer: IpAddr, hash: String) -> Result<String, AdminError> {
        let mono = self.clock.monotonic_secs();
        let r = self.admin_lock().reset_in_window(peer, hash, mono);
        if r.is_ok() {
            self.audit_admin("reset window used", true);
        }
        r
    }

    /// One line in the problem log and one audit record for an administrator password-reset event. Never a secret.
    fn audit_admin(&self, what: &str, ok: bool) {
        self.audit_admin_event(
            "admin_reset",
            &format!("administrator reset: {what}"),
            what,
            ok,
        );
    }

    /// One line (`line`) in the problem log and one audit record under `tool`, with the short `what` as its note,
    /// for an administrator event. Never a secret.
    fn audit_admin_event(&self, tool: &'static str, line: &str, what: &str, ok: bool) {
        crate::diag::warn(line);
        if let Some(log) = self.audit.get() {
            let now = self.clock.now();
            log.record(
                now,
                now.div_euclid(86_400),
                0,
                "admin",
                None,
                &crate::audit::Act {
                    tool,
                    tier: None,
                    args: &serde_json::Value::Null,
                    decision: if ok {
                        crate::audit::decision::RAN
                    } else {
                        crate::audit::decision::INVALID
                    },
                    held_id: None,
                    outcome: None,
                    note: Some(what),
                },
            );
        }
    }

    // ----- status and test speech for the web UI -----

    /// Publish a snapshot of the assistant for the web UI to show (replaces the previous one).
    pub fn set_ui_status(&self, status: serde_json::Value) {
        *self.status.lock().unwrap_or_else(PoisonError::into_inner) = status;
    }

    /// The latest snapshot (`null` before the first).
    #[must_use]
    pub fn ui_status(&self) -> serde_json::Value {
        self.status
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Replace one top-level field of the published snapshot, so a change that happens while the main loop is busy
    /// (a realtime conversation) shows at once. Does nothing before the first snapshot.
    pub fn patch_ui_status(&self, key: &str, value: serde_json::Value) {
        let mut g = self.status.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(o) = g.as_object_mut() {
            o.insert(key.to_owned(), value);
        }
    }

    /// Ask the speaker to say `text` (the UI's "test speak"). At most [`MAX_PENDING_SPEECH`] requests wait, each
    /// up to [`MAX_SPEECH_CHARS`] characters. Returns false if refused.
    pub fn queue_speech(&self, text: &str) -> bool {
        self.queue_speech_as(text, None)
    }

    /// Like [`Core::queue_speech`], with the voice to try for this one phrase (a preview before it is saved).
    pub fn queue_speech_as(&self, text: &str, voice: Option<&str>) -> bool {
        let text = text.trim();
        if text.is_empty() || text.chars().count() > MAX_SPEECH_CHARS {
            return false;
        }
        let mut q = self.speech.lock().unwrap_or_else(PoisonError::into_inner);
        if q.len() >= MAX_PENDING_SPEECH {
            return false;
        }
        q.push_back((text.to_owned(), voice.map(str::to_owned)));
        true
    }

    /// The next queued test phrase, if any.
    pub fn take_speech(&self) -> Option<String> {
        self.take_speech_as().map(|(text, _)| text)
    }

    /// The next queued test phrase and the voice to say it in, if one was asked for.
    pub fn take_speech_as(&self) -> Option<(String, Option<String>)> {
        self.speech
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop_front()
    }

    // ----- voice side -----

    /// The next thing to announce, if the device is idle. Call [`Core::mark_announced`] after speaking.
    #[must_use]
    pub fn next_announcement(
        &self,
        device_idle: bool,
        minute_of_day: u16,
    ) -> Option<NextAnnouncement> {
        let now = self.clock.now();
        let g = self.lock();
        let plan = g
            .inbox
            .plan(now, device_idle, minute_of_day, &g.endpoints)?;
        let event = g.inbox.get(plan.lead)?.clone();
        Some(NextAnnouncement {
            event,
            more_waiting: plan.rest.len(),
        })
    }

    /// Set or clear the device-wide quiet-hours window (REQ-INB-040): while it lasts, only urgent events
    /// are announced. Takes effect on the next check, so no restart is needed.
    pub fn set_quiet_hours(&self, quiet: Option<QuietHours>) {
        self.lock().inbox.set_quiet(quiet);
    }

    /// The device-wide quiet-hours window, if one is set (REQ-INB-040).
    #[must_use]
    pub fn quiet_hours(&self) -> Option<QuietHours> {
        self.lock().inbox.quiet()
    }

    /// Record that events were spoken.
    pub fn mark_announced(&self, ids: &[u64]) {
        let mut g = self.lock();
        g.inbox.mark_announced(ids);
        self.persist(&g, Dirty::INBOX);
    }

    /// The event a spoken reply would go to.
    #[must_use]
    pub fn reply_target(&self) -> Option<Event> {
        self.lock().inbox.reply_target().cloned()
    }

    /// The user answers the most recently announced event: `Ack` ("I'll handle it") or `Instruct`
    /// (free-text instruction). Queued for the endpoint's bridge (REQ-INB-008, REQ-INB-017).
    ///
    /// # Errors
    /// [`ReplyError::NothingToReplyTo`] or [`ReplyError::QueueFull`].
    pub fn reply(&self, kind: ReplyKind, text: &str) -> Result<Replied, ReplyError> {
        let now = self.clock.now();
        let mut g = self.lock();
        let ev = g
            .inbox
            .reply_target()
            .cloned()
            .ok_or(ReplyError::NothingToReplyTo)?;
        // A question an agent asked out loud (REQ-EXT-012): the answer goes to the one waiting caller, as
        // data, and to no bridge.
        let end = if kind == ReplyKind::Ack {
            AskEnd::Acknowledged
        } else {
            AskEnd::Answered(text.to_owned())
        };
        if self.deliver_ask(ev.id, end) {
            g.inbox.acknowledge(ev.id);
            self.persist(&g, Dirty::INBOX);
            return Ok(Replied {
                event_id: ev.id,
                origin: ev.origin_name().to_owned(),
                title: ev.title,
            });
        }
        g.replies
            .push(now, &ev.endpoint, ev.id, ev.session_ref.clone(), kind, text)
            .map_err(|_| ReplyError::QueueFull)?;
        // Only an acknowledgement changes the event; an instruction only adds a reply for the bridge.
        let dirty = if kind == ReplyKind::Ack {
            g.inbox.acknowledge(ev.id);
            Dirty::INBOX_AND_REPLIES
        } else {
            Dirty::REPLIES
        };
        self.persist(&g, dirty);
        self.notify.notify_waiters();
        Ok(Replied {
            event_id: ev.id,
            origin: ev.origin_name().to_owned(),
            title: ev.title,
        })
    }

    /// Dismiss the current event without telling the agent.
    pub fn dismiss_current(&self) -> bool {
        let mut g = self.lock();
        let Some(id) = g.inbox.reply_target().map(|e| e.id) else {
            return false;
        };
        let done = g.inbox.dismiss(id);
        self.deliver_ask(id, AskEnd::Dismissed);
        self.persist(&g, Dirty::INBOX);
        done
    }

    /// Postpone the current event.
    pub fn snooze_current(&self, secs: i64) -> bool {
        let now = self.clock.now();
        let mut g = self.lock();
        let Some(id) = g.inbox.reply_target().map(|e| e.id) else {
            return false;
        };
        let done = g.inbox.snooze(id, now.saturating_add(secs));
        self.persist(&g, Dirty::INBOX);
        done
    }

    /// The current event, for "give me the details".
    #[must_use]
    pub fn current_event(&self) -> Option<Event> {
        self.reply_target()
    }

    /// The roll-up for "what are my agents doing?".
    #[must_use]
    pub fn status(&self) -> Vec<SourceStatus> {
        self.lock().inbox.status_by_source()
    }

    /// Everything in the inbox (portal, debugging).
    #[must_use]
    pub fn events(&self) -> Vec<Event> {
        self.lock().inbox.events().to_vec()
    }

    /// Housekeeping: drop expired replies and old handled events.
    pub fn maintain(&self) {
        let now = self.clock.now();
        let mut g = self.lock();
        let expired = g.replies.expire(now);
        let pruned = g.inbox.prune(now, 24 * 3600);
        let lapsed = g.inbox.expire(now);
        if expired > 0 {
            self.persist(&g, Dirty::REPLIES);
        }
        if pruned + lapsed > 0 {
            self.persist(&g, Dirty::INBOX);
        }
    }

    /// Count of events currently in a given state (tests, metrics).
    #[must_use]
    pub fn count_state(&self, f: impl Fn(&EventState) -> bool) -> usize {
        self.lock()
            .inbox
            .events()
            .iter()
            .filter(|e| f(&e.state))
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inbox::event::EventKind;
    use crate::store::testutil::TempDir;
    use std::sync::atomic::{AtomicI64, Ordering};

    pub(crate) struct TestClock(pub AtomicI64);
    impl Clock for TestClock {
        fn monotonic_secs(&self) -> i64 {
            self.now()
        }
        fn now(&self) -> Ts {
            self.0.load(Ordering::Relaxed)
        }
    }

    fn core() -> (Core, Arc<TestClock>) {
        let clock = Arc::new(TestClock(AtomicI64::new(1_800_000_000)));
        (
            Core::in_memory(clock.clone(), Adapters::builtin(), Limits::default()),
            clock,
        )
    }

    const STOP: &str = r#"{"session_id":"s1","cwd":"/home/a/clawdot","hook_event_name":"Stop","last_assistant_message":"Done. All tests pass."}"#;

    #[test]
    fn a_valid_claude_code_hook_lands_in_the_inbox() {
        let (c, _) = core();
        let (_, token) = c.create_endpoint("claude-code", "claude-code").unwrap();
        let out = c
            .ingest_hook("claude-code", &token, LOCALHOST_PEER, STOP.as_bytes())
            .unwrap();
        assert_eq!(out, HookOutcome::Accepted(1));
        let ev = &c.events()[0];
        assert_eq!(
            (ev.kind, ev.title.as_str(), ev.session_ref.as_deref()),
            (EventKind::Done, "clawdot finished", Some("s1"))
        );
    }

    #[test]
    fn bad_credentials_are_refused_identically_for_unknown_endpoints_and_wrong_tokens() {
        let (c, _) = core();
        let (_, token) = c.create_endpoint("claude-code", "claude-code").unwrap();
        assert_eq!(
            c.ingest_hook("claude-code", "cdt_wrong", LOCALHOST_PEER, b"{}"),
            Err(HookError::Unauthorized)
        );
        assert_eq!(
            c.ingest_hook("no-such", &token, LOCALHOST_PEER, b"{}"),
            Err(HookError::Unauthorized)
        );
        assert!(c.events().is_empty());
    }

    #[test]
    fn repeated_bad_guesses_get_rate_limited() {
        let (c, _) = core();
        c.create_endpoint("x", "generic").unwrap();
        let mut limited = false;
        for _ in 0..100 {
            if c.ingest_hook("x", "cdt_guess", LOCALHOST_PEER, b"{}") == Err(HookError::RateLimited)
            {
                limited = true;
                break;
            }
        }
        assert!(limited, "brute force must hit a limit");
    }

    #[test]
    fn a_flood_from_a_valid_endpoint_is_throttled_then_recovers() {
        let (c, clock) = core();
        let (_, token) = c.create_endpoint("x", "generic").unwrap();
        let mut accepted = 0;
        let mut limited = 0;
        for i in 0..60 {
            match c.ingest_hook(
                "x",
                &token,
                LOCALHOST_PEER,
                format!(r#"{{"title":"t{i}","session_ref":"s{i}"}}"#).as_bytes(),
            ) {
                Ok(_) => accepted += 1,
                Err(HookError::RateLimited) => limited += 1,
                Err(e) => panic!("unexpected {e:?}"),
            }
        }
        assert!(
            accepted >= 20 && limited > 0,
            "accepted {accepted}, limited {limited}"
        );
        clock.0.fetch_add(10, Ordering::Relaxed);
        assert!(
            c.ingest_hook("x", &token, LOCALHOST_PEER, br#"{"title":"later"}"#)
                .is_ok(),
            "tokens refill"
        );
    }

    #[test]
    fn malformed_input_is_rejected_cleanly() {
        let (c, _) = core();
        let (_, token) = c.create_endpoint("x", "generic").unwrap();
        assert_eq!(
            c.ingest_hook("x", &token, LOCALHOST_PEER, b"not json"),
            Err(HookError::BadJson)
        );
        let (_, t2) = c.create_endpoint("y", "no-such-adapter").unwrap();
        assert_eq!(
            c.ingest_hook("y", &t2, LOCALHOST_PEER, b"{}"),
            Err(HookError::NoAdapter)
        );
    }

    #[test]
    fn muted_endpoints_accept_but_do_not_queue() {
        let (c, _) = core();
        let (_, token) = c.create_endpoint("x", "generic").unwrap();
        c.update_filter("x", |f| f.muted = true);
        let out = c
            .ingest_hook("x", &token, LOCALHOST_PEER, br#"{"title":"hi"}"#)
            .unwrap();
        assert!(matches!(out, HookOutcome::Filtered(FilterReason::Muted)));
        assert!(c.events().is_empty());
    }

    #[test]
    fn announce_reply_and_the_bridge_round_trip() {
        let (c, _) = core();
        let (_, token) = c.create_endpoint("claude-code", "claude-code").unwrap();
        c.ingest_hook("claude-code", &token, LOCALHOST_PEER, STOP.as_bytes())
            .unwrap();

        let next = c.next_announcement(true, 600).unwrap();
        assert_eq!(next.event.title, "clawdot finished");
        assert!(c.next_announcement(false, 600).is_none(), "not while busy");
        c.mark_announced(&[next.event.id]);
        assert!(
            c.next_announcement(true, 600).is_none(),
            "announced events are not repeated"
        );

        let replied = c
            .reply(ReplyKind::Instruct, "keep going and add tests")
            .unwrap();
        assert_eq!(replied.event_id, next.event.id);

        // The bridge pulls it with its own credentials, and only its own.
        let pending = c
            .bridge_pending("claude-code", &token, LOCALHOST_PEER, 0)
            .unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].session_ref.as_deref(), Some("s1"));
        assert_eq!(pending[0].text, "keep going and add tests");
        assert!(
            c.bridge_ack("claude-code", &token, LOCALHOST_PEER, pending[0].id)
                .unwrap()
        );
        assert!(
            c.bridge_pending("claude-code", &token, LOCALHOST_PEER, 0)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn saying_thanks_ill_handle_it_acknowledges_locally_and_tells_the_agent() {
        let (c, _) = core();
        let (_, token) = c.create_endpoint("claude-code", "claude-code").unwrap();
        c.ingest_hook("claude-code", &token, LOCALHOST_PEER, STOP.as_bytes())
            .unwrap();
        let id = c.next_announcement(true, 0).unwrap().event.id;
        c.mark_announced(&[id]);
        c.reply(ReplyKind::Ack, "").unwrap();
        assert_eq!(c.events()[0].state, EventState::Acknowledged);
        assert_eq!(
            c.bridge_pending("claude-code", &token, LOCALHOST_PEER, 0)
                .unwrap()[0]
                .kind,
            ReplyKind::Ack
        );
        assert_eq!(
            c.reply(ReplyKind::Ack, ""),
            Err(ReplyError::NothingToReplyTo),
            "nothing open any more"
        );
    }

    #[test]
    fn replying_with_nothing_announced_is_an_error_not_a_guess() {
        let (c, _) = core();
        assert_eq!(
            c.reply(ReplyKind::Instruct, "do it"),
            Err(ReplyError::NothingToReplyTo)
        );
    }

    #[test]
    fn a_bridge_never_sees_another_endpoints_replies() {
        let (c, _) = core();
        let (_, a) = c.create_endpoint("a", "generic").unwrap();
        let (_, b) = c.create_endpoint("b", "generic").unwrap();
        c.ingest_hook(
            "a",
            &a,
            LOCALHOST_PEER,
            br#"{"title":"for a","session_ref":"sa"}"#,
        )
        .unwrap();
        let id = c.next_announcement(true, 0).unwrap().event.id;
        c.mark_announced(&[id]);
        c.reply(ReplyKind::Instruct, "x").unwrap();
        assert!(
            c.bridge_pending("b", &b, LOCALHOST_PEER, 0)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            c.bridge_pending("a", &a, LOCALHOST_PEER, 0).unwrap().len(),
            1
        );
        assert_eq!(
            c.bridge_pending("a", &b, LOCALHOST_PEER, 0),
            Err(HookError::Unauthorized),
            "b's token is not a's"
        );
    }

    // REQ-CFG-028: the rate-limit state of an endpoint is not inherited either: a new endpoint under a deleted
    // endpoint's name starts with a full burst, not with whatever the old agent had used up.
    #[test]
    fn a_recreated_endpoint_does_not_inherit_the_old_ones_rate_limit() {
        let (c, _) = core();
        let (_, old) = c.create_endpoint("agent", "generic").unwrap();
        let mut n = 0;
        while c
            .ingest_hook(
                "agent",
                &old,
                LOCALHOST_PEER,
                format!(r#"{{"title":"t{n}","session_ref":"s{n}"}}"#).as_bytes(),
            )
            .is_ok()
        {
            n += 1;
            assert!(n < 1000, "never throttled");
        }
        c.revoke_endpoint("agent").unwrap();
        let (_, new) = c.create_endpoint("agent", "generic").unwrap();
        assert!(
            c.ingest_hook("agent", &new, LOCALHOST_PEER, br#"{"title":"fresh"}"#)
                .is_ok(),
            "the new endpoint was throttled by the old one's use"
        );
    }

    // REQ-CFG-028: deleting an endpoint drops what was waiting for it, so an endpoint made later under the
    // same name starts empty and its bridge never receives the old agent's replies.
    #[test]
    fn a_recreated_endpoint_inherits_no_replies_and_no_events() {
        let (c, _) = core();
        let (_, old) = c.create_endpoint("agent", "generic").unwrap();
        c.ingest_hook(
            "agent",
            &old,
            LOCALHOST_PEER,
            br#"{"title":"first","session_ref":"s1"}"#,
        )
        .unwrap();
        let first = c.next_announcement(true, 0).unwrap().event.id;
        c.mark_announced(&[first]);
        c.reply(ReplyKind::Instruct, "private to the old agent")
            .unwrap();
        c.ingest_hook(
            "agent",
            &old,
            LOCALHOST_PEER,
            br#"{"title":"second","session_ref":"s2"}"#,
        )
        .unwrap();
        assert_eq!(
            c.bridge_pending("agent", &old, LOCALHOST_PEER, 0)
                .unwrap()
                .len(),
            1
        );

        c.revoke_endpoint("agent").unwrap();
        let (_, new) = c.create_endpoint("agent", "generic").unwrap();

        assert!(
            c.bridge_pending("agent", &new, LOCALHOST_PEER, 0)
                .unwrap()
                .is_empty(),
            "the new endpoint got the old one's replies"
        );
        assert!(c.events().is_empty(), "queued events outlived the endpoint");
        assert!(
            c.next_announcement(true, 0).is_none(),
            "a revoked sender's queued event was still announced"
        );
        assert_eq!(
            c.reply(ReplyKind::Instruct, "x"),
            Err(ReplyError::NothingToReplyTo)
        );
    }

    #[test]
    fn dismissing_and_snoozing_the_current_event() {
        let (c, clock) = core();
        let (_, token) = c.create_endpoint("x", "generic").unwrap();
        c.ingest_hook(
            "x",
            &token,
            LOCALHOST_PEER,
            br#"{"title":"one","session_ref":"1"}"#,
        )
        .unwrap();
        let id = c.next_announcement(true, 0).unwrap().event.id;
        c.mark_announced(&[id]);
        assert!(c.snooze_current(600));
        assert!(c.next_announcement(true, 0).is_none());
        clock.0.fetch_add(601, Ordering::Relaxed);
        let again = c.next_announcement(true, 0).unwrap();
        c.mark_announced(&[again.event.id]);
        assert!(c.dismiss_current());
        assert!(c.next_announcement(true, 0).is_none());
        assert!(!c.dismiss_current(), "nothing left to dismiss");
    }

    #[test]
    fn a_huge_snooze_postpones_for_good_instead_of_overflowing() {
        let (c, _) = core();
        let (_, token) = c.create_endpoint("x", "generic").unwrap();
        c.ingest_hook(
            "x",
            &token,
            LOCALHOST_PEER,
            br#"{"title":"one","session_ref":"1"}"#,
        )
        .unwrap();
        let id = c.next_announcement(true, 0).unwrap().event.id;
        c.mark_announced(&[id]);
        assert!(c.snooze_current(i64::MAX));
        assert!(c.next_announcement(true, 0).is_none());
    }

    fn open_in(dir: &TempDir) -> Core {
        let clock = Arc::new(TestClock(AtomicI64::new(1_800_000_000)));
        Core::open(
            clock,
            Adapters::builtin(),
            Limits::default(),
            Stores::in_dir(&dir.0),
        )
        .unwrap()
    }

    /// Which of the three state files exist after the core has written everything it owes.
    fn written(c: &Core, dir: &TempDir) -> Vec<&'static str> {
        c.flush_blocking();
        let found = ["endpoints.json", "inbox.json", "replies.json"]
            .into_iter()
            .filter(|f| dir.0.join(f).exists())
            .collect();
        for f in ["endpoints.json", "inbox.json", "replies.json"] {
            let _ = std::fs::remove_file(dir.0.join(f));
        }
        found
    }

    #[test]
    fn a_change_rewrites_only_the_files_it_changed() {
        let dir = TempDir::new("dirty");
        let c = open_in(&dir);
        let (_, token) = c.create_endpoint("claude-code", "claude-code").unwrap();
        assert_eq!(written(&c, &dir), ["endpoints.json"], "a new endpoint");
        c.ingest_hook("claude-code", &token, LOCALHOST_PEER, STOP.as_bytes())
            .unwrap();
        assert_eq!(written(&c, &dir), ["inbox.json"], "a posted event");
        let id = c.next_announcement(true, 0).unwrap().event.id;
        c.mark_announced(&[id]);
        assert_eq!(written(&c, &dir), ["inbox.json"], "an announcement");
        c.reply(ReplyKind::Instruct, "go on").unwrap();
        assert_eq!(written(&c, &dir), ["replies.json"], "an instruction");
        let reply = c
            .bridge_pending("claude-code", &token, LOCALHOST_PEER, 0)
            .unwrap()[0]
            .id;
        assert!(
            c.bridge_ack("claude-code", &token, LOCALHOST_PEER, reply)
                .unwrap()
        );
        assert_eq!(written(&c, &dir), ["replies.json"], "a bridge ack");
        c.reply(ReplyKind::Ack, "").unwrap();
        assert_eq!(
            written(&c, &dir),
            ["inbox.json", "replies.json"],
            "a spoken ack"
        );
    }

    // REQ-INB-004: housekeeping that only prunes old handled events still saves the inbox.
    #[test]
    fn pruning_alone_saves_the_inbox() {
        let dir = TempDir::new("prune");
        let clock = Arc::new(TestClock(AtomicI64::new(1_800_000_000)));
        let c = Core::open(
            clock.clone(),
            Adapters::builtin(),
            Limits::default(),
            Stores::in_dir(&dir.0),
        )
        .unwrap();
        let (_, token) = c.create_endpoint("claude-code", "claude-code").unwrap();
        c.ingest_hook("claude-code", &token, LOCALHOST_PEER, STOP.as_bytes())
            .unwrap();
        let id = c.next_announcement(true, 0).unwrap().event.id;
        c.mark_announced(&[id]);
        assert!(c.dismiss_current());
        written(&c, &dir);
        clock.0.fetch_add(25 * 3600, Ordering::Relaxed);
        c.maintain();
        assert!(c.events().is_empty(), "the old handled event was pruned");
        assert_eq!(written(&c, &dir), ["inbox.json"], "and the file follows");
    }

    #[test]
    fn state_files_are_compact_json() {
        let dir = TempDir::new("compact");
        let c = open_in(&dir);
        let (_, token) = c.create_endpoint("claude-code", "claude-code").unwrap();
        c.ingest_hook("claude-code", &token, LOCALHOST_PEER, STOP.as_bytes())
            .unwrap();
        c.flush_blocking();
        for f in ["endpoints.json", "inbox.json"] {
            let text = std::fs::read_to_string(dir.0.join(f)).unwrap();
            assert!(!text.contains('\n'), "{f} is one line");
            assert!(!text.contains("  "), "{f} has no indentation");
            serde_json::from_str::<serde_json::Value>(&text).unwrap();
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn durable_resolves_once_an_accepted_event_is_on_disk() {
        let dir = TempDir::new("durable");
        let c = open_in(&dir);
        let (_, token) = c.create_endpoint("claude-code", "claude-code").unwrap();
        let mark = c.save_mark();
        c.ingest_hook("claude-code", &token, LOCALHOST_PEER, STOP.as_bytes())
            .unwrap();
        c.saved_since(mark).await.unwrap();
        // No flush_blocking: awaiting durable() alone is what the HTTP layer does before it answers.
        let text = std::fs::read_to_string(dir.0.join("inbox.json")).unwrap();
        assert!(text.contains("clawdot finished"));
    }

    #[test]
    fn state_survives_a_restart_including_pending_replies() {
        let dir = TempDir::new("core");
        let clock = Arc::new(TestClock(AtomicI64::new(1_800_000_000)));
        let token;
        {
            let c = Core::open(
                clock.clone(),
                Adapters::builtin(),
                Limits::default(),
                Stores::in_dir(&dir.0),
            )
            .unwrap();
            token = c.create_endpoint("claude-code", "claude-code").unwrap().1;
            c.ingest_hook("claude-code", &token, LOCALHOST_PEER, STOP.as_bytes())
                .unwrap();
            let id = c.next_announcement(true, 0).unwrap().event.id;
            c.mark_announced(&[id]);
            c.reply(ReplyKind::Instruct, "carry on").unwrap();
        }
        let c = Core::open(
            clock,
            Adapters::builtin(),
            Limits::default(),
            Stores::in_dir(&dir.0),
        )
        .unwrap();
        assert_eq!(c.events().len(), 1);
        assert_eq!(
            c.bridge_pending("claude-code", &token, LOCALHOST_PEER, 0)
                .unwrap()[0]
                .text,
            "carry on"
        );
        assert!(
            c.ingest_hook("claude-code", &token, LOCALHOST_PEER, STOP.as_bytes())
                .is_ok(),
            "the old token still works"
        );
    }

    fn peer(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    // REQ-SEC-004: failed attempts are counted per source address. One host that guesses until it is refused does
    // not stop another host presenting the right token, nor the administrator signing in with the right password.
    #[test]
    fn one_hosts_bad_guesses_do_not_lock_out_another_host() {
        let (c, _) = core();
        let (_, token) = c.create_endpoint("x", "generic").unwrap();
        let hash = crate::admin::hash_password("correct horse").unwrap();
        c.admin_setup_password(peer("192.168.1.2"), hash.clone())
            .unwrap();
        let (a, b) = (peer("192.168.1.10"), peer("192.168.1.11"));
        let mut n = 0;
        while c.ingest_hook("x", "cdt_guess", a, b"{}") != Err(HookError::RateLimited) {
            n += 1;
            assert!(n <= 20, "never throttled");
        }
        assert_eq!(n, 20, "the per-address burst");
        // A is throttled everywhere: hooks, bridges, announcements, sessions, sign-in.
        assert_eq!(
            c.bridge_check("x", "cdt_guess", a),
            Err(HookError::RateLimited)
        );
        assert_eq!(
            c.announce_check("cdt_guess", a),
            Err(HookError::RateLimited)
        );
        assert_eq!(
            c.check_admin(Some("wrong"), a),
            Err(AdminError::RateLimited)
        );
        assert_eq!(c.admin_login_begin(a), Err(AdminError::RateLimited));
        // B is not touched.
        assert_eq!(
            c.ingest_hook("x", "cdt_guess", b, b"{}"),
            Err(HookError::Unauthorized)
        );
        assert!(c.ingest_hook("x", &token, b, br#"{"title":"hi"}"#).is_ok());
        assert_eq!(c.admin_login_begin(b).unwrap(), hash);
        assert!(c.admin_login_finish(b).is_ok());
    }

    // A good sign-in is not charged, and one's own earlier failures cost only the usual throttle.
    #[test]
    fn a_successful_sign_in_gives_its_attempt_back() {
        let (c, _) = core();
        let hash = crate::admin::hash_password("correct horse").unwrap();
        c.admin_setup_password(peer("192.168.1.2"), hash).unwrap();
        let a = peer("192.168.1.10");
        for _ in 0..19 {
            assert_eq!(
                c.ingest_hook("x", "cdt_guess", a, b"{}"),
                Err(HookError::Unauthorized)
            );
        }
        // One attempt left: the right password signs in, and gets it back, many times over.
        for _ in 0..5 {
            c.admin_login_begin(a).unwrap();
            c.admin_login_finish(a).unwrap();
        }
        // Spent for real: the 20th bad guess, then refused, right password or not.
        assert_eq!(
            c.ingest_hook("x", "cdt_guess", a, b"{}"),
            Err(HookError::Unauthorized)
        );
        assert_eq!(c.admin_login_begin(a), Err(AdminError::RateLimited));
    }

    // Many addresses together hit the backstop; one address never reaches it.
    #[test]
    fn the_overall_backstop_exists_and_one_address_cannot_reach_it() {
        let (c, _) = core();
        let a = peer("203.0.113.1");
        for _ in 0..1000 {
            let _ = c.ingest_hook("x", "cdt_guess", a, b"{}");
        }
        assert_eq!(
            c.ingest_hook("x", "cdt_guess", peer("203.0.113.2"), b"{}"),
            Err(HookError::Unauthorized),
            "one noisy address left the others their share"
        );
        // Enough different addresses use up the shared ceiling (100); then even a new address is refused.
        let mut refused = false;
        for i in 0..64u8 {
            let p = IpAddr::from([198, 51, 100, i]);
            for _ in 0..20 {
                if c.ingest_hook("x", "cdt_guess", p, b"{}") == Err(HookError::RateLimited) {
                    refused = true;
                }
            }
        }
        assert!(refused, "the shared ceiling never applied");
        assert_eq!(
            c.ingest_hook("x", "cdt_guess", peer("198.51.100.200"), b"{}"),
            Err(HookError::RateLimited)
        );
    }

    #[test]
    fn the_table_of_sources_stays_bounded_under_ten_thousand_addresses() {
        let (c, clock) = core();
        for i in 0..10_000u32 {
            let p = IpAddr::from([10, (i >> 16) as u8, (i >> 8) as u8, i as u8]);
            let _ = c.ingest_hook("x", "cdt_guess", p, b"{}");
            if i % 50 == 0 {
                clock.0.fetch_add(10, Ordering::Relaxed);
            }
            assert!(c.lock().auth_failures.tracked() <= c.limits().auth_fail_sources);
        }
    }

    #[test]
    fn saved_since_reports_a_write_that_failed_and_then_one_that_did_not() {
        let dir = TempDir::new("save-outcome");
        let c = open_in(&dir);
        let (_, token) = c.create_endpoint("claude-code", "claude-code").unwrap();
        c.flush_blocking();
        let blocker = crate::store::temp_path(&dir.0.join("inbox.json"));
        std::fs::create_dir(&blocker).unwrap();
        let mark = c.save_mark();
        c.ingest_hook("claude-code", &token, LOCALHOST_PEER, STOP.as_bytes())
            .unwrap();
        let r = futures_lite_block(c.saved_since(mark));
        assert_eq!(r, Err(persist::SaveError { disk_full: false }));
        assert_eq!(c.events().len(), 1, "the event is still held in memory");
        // An unrelated file's failure is not ours, and once saving works the next change reports success.
        std::fs::remove_dir(&blocker).unwrap();
        let mark = c.save_mark();
        c.ingest_hook(
            "claude-code",
            &token,
            LOCALHOST_PEER,
            STOP.replace("s1", "s2").as_bytes(),
        )
        .unwrap();
        assert_eq!(futures_lite_block(c.saved_since(mark)), Ok(()));
    }

    /// Run a future to its end on a throwaway runtime (these tests have none of their own).
    fn futures_lite_block<T>(f: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(f)
    }

    #[test]
    fn status_reports_what_is_open() {
        let (c, _) = core();
        let (_, token) = c.create_endpoint("claude-code", "claude-code").unwrap();
        c.ingest_hook("claude-code", &token, LOCALHOST_PEER, STOP.as_bytes())
            .unwrap();
        let st = c.status();
        // The roll-up names the endpoint, not the label the payload or adapter carries.
        assert_eq!((st[0].origin.as_str(), st[0].finished), ("claude-code", 1));
        assert_eq!(c.events()[0].source, "Claude Code");
        let said = crate::inbox::summary::status_sentence(&st);
        assert!(said.starts_with("claude-code: "), "{said}");
        c.set_endpoint_spoken_name("claude-code", "the laptop")
            .unwrap();
        let said = crate::inbox::summary::status_sentence(&c.status());
        assert!(said.starts_with("the laptop: "), "{said}");
    }

    #[test]
    fn a_hook_payload_cannot_choose_who_the_speaker_names() {
        let (c, _) = core();
        let (_, token) = c.create_endpoint("claude-code", "claude-code").unwrap();
        // Every field a payload could carry that looks like an origin.
        let payload = r#"{"hook_event_name":"Stop","session_id":"s1","cwd":"/ClawDot",
            "source":"ClawDot","from":"Mum","name":"the bank","title":"ClawDot"}"#;
        c.ingest_hook("claude-code", &token, LOCALHOST_PEER, payload.as_bytes())
            .unwrap();
        let ev = c.next_announcement(true, 0).unwrap().event;
        let line = crate::inbox::summary::announcement(&ev, "", 0);
        assert!(line.starts_with("From claude-code: "), "{line}");
        assert!(!line.contains("Mum") && !line.contains("bank"), "{line}");
    }

    // REQ-RTV-011
    #[test]
    fn one_field_of_the_status_can_be_replaced_between_snapshots() {
        let (c, _) = core();
        c.patch_ui_status("realtime", serde_json::json!({"active": true}));
        assert!(c.ui_status().is_null(), "nothing before the first snapshot");
        c.set_ui_status(serde_json::json!({"version": "9", "realtime": {"active": false}}));
        c.patch_ui_status("realtime", serde_json::json!({"active": true}));
        assert_eq!(
            c.ui_status(),
            serde_json::json!({"version": "9", "realtime": {"active": true}})
        );
    }

    // ----- durations run on the monotonic clock (REQ-CFG-019, REQ-CFG-069, REQ-SEC-004) -----

    fn stepping_core() -> (Core, Arc<crate::sched::testclock::StepClock>) {
        let clock = Arc::new(crate::sched::testclock::StepClock::at(1_800_000_000));
        let core = Core::in_memory(clock.clone(), Adapters::builtin(), Limits::default());
        (core, clock)
    }

    // REQ-CFG-019: a sign-in lasts 12 hours of real time, whatever the wall clock does.
    #[test]
    fn an_administrator_session_does_not_get_longer_or_shorter_when_the_wall_clock_steps() {
        for (step, later, alive) in [
            (-24 * 3600, crate::admin::SESSION_SECS + 1, false),
            (24 * 3600, crate::admin::SESSION_SECS + 1, false),
            (-24 * 3600, crate::admin::SESSION_SECS - 1, true),
            (24 * 3600, 60, true),
        ] {
            let (c, clock) = stepping_core();
            let hash = crate::admin::hash_password("correct horse").unwrap();
            let token = c.admin_setup_password(peer("192.168.1.2"), hash).unwrap();
            let who = peer("192.168.1.2");
            assert_eq!(c.check_admin(Some(&token), who), Ok(()));
            clock.advance(later);
            clock.step_wall(step);
            assert_eq!(
                c.check_admin(Some(&token), who).is_ok(),
                alive,
                "step {step} s, {later} s on the monotonic clock"
            );
        }
    }

    // REQ-CFG-069: 300 s window, 60 s cooldown; both durations.
    #[test]
    fn the_reset_window_and_its_cooldown_do_not_get_longer_when_the_wall_clock_steps_back() {
        let who = peer("192.168.1.9");
        // Stepped back: the window still closes after 300 s.
        let (c, clock) = stepping_core();
        c.admin_setup_password(who, crate::admin::hash_password("correct horse").unwrap())
            .unwrap();
        assert_eq!(c.admin_open_reset_window(), Ok(300));
        clock.advance(299);
        clock.step_wall(-3600);
        assert_eq!(c.admin_window_begin(who), Ok(()), "299 s: still open");
        clock.advance(1);
        assert_eq!(
            c.admin_window_begin(who),
            Err(AdminError::WindowClosed),
            "300 s: closed, though the wall clock reads 59 minutes earlier"
        );
        // The cooldown after it also counts real time: refused at 59 s, allowed at 60 s.
        clock.advance(59);
        assert_eq!(c.admin_open_reset_window(), Err(AdminError::RateLimited));
        clock.advance(1);
        clock.step_wall(-3600);
        assert_eq!(c.admin_open_reset_window(), Ok(300));
        // Stepped forward: it does not close early.
        let (c, clock) = stepping_core();
        c.admin_setup_password(who, crate::admin::hash_password("correct horse").unwrap())
            .unwrap();
        c.admin_open_reset_window().unwrap();
        clock.advance(100);
        clock.step_wall(6 * 3600);
        assert_eq!(c.admin_window_begin(who), Ok(()));
    }

    // REQ-SEC-004: a step of the wall clock must not hand a guesser a fresh budget.
    #[test]
    fn a_forward_step_of_the_wall_clock_does_not_refill_the_failed_attempt_budget() {
        let (c, clock) = stepping_core();
        let a = peer("192.168.1.10");
        let mut n = 0;
        while c.ingest_hook("x", "cdt_guess", a, b"{}") != Err(HookError::RateLimited) {
            n += 1;
            assert!(n <= 20, "never throttled");
        }
        clock.step_wall(24 * 3600);
        assert_eq!(
            c.ingest_hook("x", "cdt_guess", a, b"{}"),
            Err(HookError::RateLimited),
            "no real time has passed"
        );
        clock.advance(2);
        assert_eq!(
            c.ingest_hook("x", "cdt_guess", a, b"{}"),
            Err(HookError::Unauthorized),
            "real time refills it"
        );
    }
}
