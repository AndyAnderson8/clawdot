//! The background writer for the endpoint, inbox and reply files, and for any other store that hands it
//! "this path, these bytes" (the timers and the usage counters).
//!
//! The core serialises a changed store while it holds its lock (a memory copy, no I/O) and hands the bytes
//! here; this thread does the slow part, the write and the two `fsync`s, so the lock and the async runtime are
//! never held up by flash. The runtime is current-thread, so a blocking write on it would stall every request,
//! the voice loop and the timers. A dedicated thread is used rather than `spawn_blocking` because the core's
//! methods are synchronous and are called from tests and the command line with no runtime at all.
//!
//! Writes coalesce: a file that changes again before the writer reaches it is written once, with the latest
//! bytes. A burst of acknowledgements costs one write per file, which is what saves flash wear.
//!
//! Durability (REQ-INB-004): the HTTP layer awaits [`Writer::flushed_since`] before it answers, so an event that
//! was acknowledged to a caller is on disk, and a caller whose change could not be written is told so. A ticket
//! resolves to success or failure ([`SaveError`]); a failed write is never reported as done. Dropping the writer
//! writes everything still queued.

use crate::store::{JsonStore, StoreError};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use tokio::sync::watch;

/// Which stores a change touched.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Dirty {
    pub endpoints: bool,
    pub inbox: bool,
    pub replies: bool,
}

impl Dirty {
    pub const ENDPOINTS: Self = Self {
        endpoints: true,
        inbox: false,
        replies: false,
    };
    pub const INBOX: Self = Self {
        endpoints: false,
        inbox: true,
        replies: false,
    };
    pub const REPLIES: Self = Self {
        endpoints: false,
        inbox: false,
        replies: true,
    };
    pub const INBOX_AND_REPLIES: Self = Self {
        endpoints: false,
        inbox: true,
        replies: true,
    };
}

/// A change that could not be written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SaveError {
    /// The disk (or the quota) is full, so retrying will not help until space is freed.
    pub disk_full: bool,
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.disk_full {
            "the change could not be saved because the storage is full"
        } else {
            "the change could not be saved to storage"
        })
    }
}

impl std::error::Error for SaveError {}

impl SaveError {
    /// What a failed write says about the cause.
    pub(crate) fn from_store(e: &StoreError) -> Self {
        Self {
            disk_full: matches!(e, StoreError::Io(io) if io.kind() == std::io::ErrorKind::StorageFull),
        }
    }
}

/// One file waiting to be written.
#[derive(Debug)]
struct Pending {
    store: JsonStore,
    bytes: Vec<u8>,
    /// The first ticket this entry carries (later ones replaced its bytes), and the newest.
    first: u64,
    newest: u64,
}

/// A write that failed: the file and the tickets (`first..=newest`) whose bytes it carried.
#[derive(Debug)]
struct Failure {
    path: PathBuf,
    first: u64,
    newest: u64,
    error: SaveError,
}

/// Names the files a caller cares about.
pub type Ours<'a> = &'a (dyn Fn(&Path) -> bool + Sync);

/// Failures remembered; an older one is forgotten (see `State::forgotten`).
const KEEP_FAILURES: usize = 16;

#[derive(Debug, Default)]
struct State {
    /// At most one entry per file: a newer write replaces an older one that has not started.
    pending: Vec<Pending>,
    /// Writes handed in so far.
    submitted: u64,
    /// Writes handed in before the last batch started that have now finished (well or badly).
    written: u64,
    /// The most recent writes that failed.
    failures: VecDeque<Failure>,
    /// Every ticket up to this one may have failed without a record: a failure was dropped to make room. Asked
    /// about, such a ticket is reported as failed, so a failure is never lost, only reported too eagerly.
    forgotten: u64,
    closing: bool,
    /// Tests hold the writer to look at the state before a write lands.
    #[cfg(test)]
    paused: bool,
}

#[derive(Debug)]
struct Shared {
    state: Mutex<State>,
    /// Signalled when work arrives or the writer is closing.
    work: Condvar,
    /// Signalled when a batch has been written.
    done: Condvar,
    /// The same `written` count for async waiters.
    progress: watch::Sender<u64>,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Owns the writer thread.
#[derive(Debug)]
pub struct Writer {
    shared: Arc<Shared>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl Writer {
    /// Start the thread, or `None` if the system will not give one.
    pub fn start() -> Option<Self> {
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            work: Condvar::new(),
            done: Condvar::new(),
            progress: watch::channel(0).0,
        });
        let worker = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("clawdot-store".into())
            // The loop holds a few locals and calls into the file system; the default 2 MiB is wasted here.
            .stack_size(128 * 1024)
            .spawn(move || run(&worker))
            .ok()?;
        Some(Self {
            shared,
            thread: Mutex::new(Some(thread)),
        })
    }

    /// Queue `bytes` to replace the file behind `store`. Returns a ticket for [`Writer::is_durable`] and
    /// [`Writer::wait`].
    pub fn submit(&self, store: &JsonStore, bytes: Vec<u8>) -> u64 {
        let mut st = self.shared.lock();
        let ticket = Self::enqueue(&mut st, store, bytes);
        drop(st);
        self.shared.work.notify_one();
        ticket
    }

    /// Like [`Writer::submit`], but the bytes are made while the writer's lock is held. Two callers racing to
    /// save one piece of state are then ordered by the lock: whichever is queued last also read the state last,
    /// so an older state can never replace a newer one. `encode` must be quick and must not call the writer.
    /// Returns `None` (and logs) when the bytes could not be made.
    pub fn submit_with(
        &self,
        store: &JsonStore,
        encode: impl FnOnce() -> Result<Vec<u8>, crate::store::StoreError>,
    ) -> Option<u64> {
        let mut st = self.shared.lock();
        let bytes = match encode() {
            Ok(b) => b,
            Err(e) => {
                crate::diag::error(&format!("could not save {}: {e}", store.path().display()));
                return None;
            }
        };
        let ticket = Self::enqueue(&mut st, store, bytes);
        drop(st);
        self.shared.work.notify_one();
        Some(ticket)
    }

    fn enqueue(st: &mut State, store: &JsonStore, bytes: Vec<u8>) -> u64 {
        st.submitted += 1;
        let ticket = st.submitted;
        if let Some(slot) = st
            .pending
            .iter_mut()
            .find(|p| p.store.path() == store.path())
        {
            slot.bytes = bytes;
            slot.newest = ticket;
        } else {
            st.pending.push(Pending {
                store: store.clone(),
                bytes,
                first: ticket,
                newest: ticket,
            });
        }
        ticket
    }

    /// Whether the write behind `ticket` (and everything before it) has finished.
    pub fn is_durable(&self, ticket: u64) -> bool {
        self.shared.lock().written >= ticket
    }

    /// The tickets handed out so far. Take it before a change; [`Writer::flushed_since`] then answers for every
    /// write queued after it.
    pub fn mark(&self) -> u64 {
        self.shared.lock().submitted
    }

    /// Whether any write of a file `ours` accepts, carrying a ticket in `(after, upto]`, failed.
    fn outcome(&self, after: u64, upto: u64, ours: Ours<'_>) -> Result<(), SaveError> {
        let st = self.shared.lock();
        if st.forgotten > after {
            return Err(SaveError { disk_full: false });
        }
        match st
            .failures
            .iter()
            .find(|f| f.newest > after && f.first <= upto && ours(&f.path))
        {
            Some(f) => Err(f.error),
            None => Ok(()),
        }
    }

    /// Resolves once the write behind `ticket` (and everything before it) has finished: `Ok` if it was written,
    /// `Err` if it failed. `ours` names the files the caller cares about.
    ///
    /// # Errors
    /// [`SaveError`] when the write failed.
    pub async fn wait(&self, ticket: u64, ours: Ours<'_>) -> Result<(), SaveError> {
        let mut rx = self.shared.progress.subscribe();
        // The sender lives as long as `self`, so this cannot fail while we hold it.
        let _ = rx.wait_for(|written| *written >= ticket).await;
        self.outcome(ticket.saturating_sub(1), ticket, ours)
    }

    /// Resolves once everything submitted before the call has been written or has failed. `Ok` when none of the
    /// writes queued since `mark` (see [`Writer::mark`]) to a file `ours` accepts failed.
    ///
    /// # Errors
    /// [`SaveError`] when one of them failed.
    pub async fn flushed_since(&self, mark: u64, ours: Ours<'_>) -> Result<(), SaveError> {
        let target = self.shared.lock().submitted;
        let mut rx = self.shared.progress.subscribe();
        // The sender lives as long as `self`, so this cannot fail while we hold it.
        let _ = rx.wait_for(|written| *written >= target).await;
        self.outcome(mark, target, ours)
    }

    /// Whether any write queued since `mark` to a file `ours` accepts has failed. For tests: call it after
    /// [`Writer::flush_blocking`].
    #[cfg(test)]
    pub(crate) fn failed_since(&self, mark: u64, ours: Ours<'_>) -> bool {
        let target = self.shared.lock().submitted;
        self.outcome(mark, target, ours).is_err()
    }

    /// Stop writing until [`Writer::resume`], so a test can look at the state before a write lands.
    #[cfg(test)]
    pub(crate) fn pause(&self) {
        self.shared.lock().paused = true;
    }

    /// Let a paused writer go on.
    #[cfg(test)]
    pub(crate) fn resume(&self) {
        self.shared.lock().paused = false;
        self.shared.work.notify_one();
    }

    /// Like [`Writer::flush_blocking`], but gives up after `limit`. Returns whether everything submitted before the
    /// call is on disk. For shutdown, where a disk that does not answer must not keep the program from stopping.
    pub fn flush_within(&self, limit: std::time::Duration) -> bool {
        let end = std::time::Instant::now() + limit;
        let mut st = self.shared.lock();
        let target = st.submitted;
        while st.written < target {
            let left = end.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return false;
            }
            st = self
                .shared
                .done
                .wait_timeout(st, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        true
    }

    /// Block until everything submitted before the call is on disk. For tests and shutdown, never the runtime.
    pub fn flush_blocking(&self) {
        let mut st = self.shared.lock();
        let target = st.submitted;
        while st.written < target {
            st = self
                .shared
                .done
                .wait(st)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        self.shared.lock().closing = true;
        self.shared.work.notify_one();
        let handle = self
            .thread
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(handle) = handle {
            let _ = handle.join();
        }
    }
}

/// Whether a test has paused the writer.
fn held(st: &State) -> bool {
    #[cfg(test)]
    {
        st.paused
    }
    #[cfg(not(test))]
    {
        let _ = st;
        false
    }
}

fn run(shared: &Shared) {
    loop {
        let (batch, upto) = {
            let mut st = shared.lock();
            while (st.pending.is_empty() || held(&st)) && !st.closing {
                st = shared.work.wait(st).unwrap_or_else(PoisonError::into_inner);
            }
            if st.pending.is_empty() {
                return;
            }
            (std::mem::take(&mut st.pending), st.submitted)
        };
        let mut failed = Vec::new();
        for p in batch {
            // A failed write must not lose the in-memory event, and it must not be silent: it is logged and
            // remembered, so whoever waits for this write is told that it did not happen.
            if let Err(e) = p.store.write(&p.bytes) {
                crate::diag::error(&format!("could not save {}: {e}", p.store.path().display()));
                failed.push(Failure {
                    path: p.store.path().to_path_buf(),
                    first: p.first,
                    newest: p.newest,
                    error: SaveError::from_store(&e),
                });
            }
        }
        {
            // The failures are recorded in the same step that moves `written` on, so a waiter that wakes sees them.
            let mut st = shared.lock();
            for f in failed {
                if st.failures.len() == KEEP_FAILURES
                    && let Some(old) = st.failures.pop_front()
                {
                    st.forgotten = st.forgotten.max(old.newest);
                }
                st.failures.push_back(f);
            }
            st.written = upto;
        }
        shared.progress.send_replace(upto);
        shared.done.notify_all();
    }
}

/// A write that was queued and may not be on disk yet, or one whose outcome is already known.
#[derive(Debug, Clone)]
pub struct Durability(Kind);

#[derive(Debug, Clone)]
enum Kind {
    Queued {
        writer: Arc<Writer>,
        ticket: u64,
        path: PathBuf,
    },
    Settled(Result<(), SaveError>),
}

impl Durability {
    /// Name the write behind `ticket`, which carried the file at `path`.
    #[must_use]
    pub fn new(writer: Arc<Writer>, ticket: u64, path: &Path) -> Self {
        Self(Kind::Queued {
            writer,
            ticket,
            path: path.to_path_buf(),
        })
    }

    /// A write whose outcome is known already (made inline, or never made).
    #[must_use]
    pub fn settled(outcome: Result<(), SaveError>) -> Self {
        Self(Kind::Settled(outcome))
    }

    /// The outcome, if the write has finished.
    #[must_use]
    pub fn outcome_now(&self) -> Option<Result<(), SaveError>> {
        match &self.0 {
            Kind::Queued {
                writer,
                ticket,
                path,
            } => writer
                .is_durable(*ticket)
                .then(|| writer.outcome(ticket.saturating_sub(1), *ticket, &|p| p == path)),
            Kind::Settled(r) => Some(*r),
        }
    }

    /// Resolves once the write has finished: `Ok` when it is on disk, `Err` when it failed.
    ///
    /// # Errors
    /// [`SaveError`] when the write failed.
    pub async fn wait(self) -> Result<(), SaveError> {
        match self.0 {
            Kind::Queued {
                writer,
                ticket,
                path,
            } => writer.wait(ticket, &|p| p == path).await,
            Kind::Settled(r) => r,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::testutil::TempDir;

    #[test]
    fn writes_to_one_file_coalesce_to_the_latest_and_all_land_on_flush() {
        let dir = TempDir::new("writer");
        let a = JsonStore::new(dir.0.join("a.json"), 1);
        let b = JsonStore::new(dir.0.join("b.json"), 1);
        let w = Writer::start().unwrap();
        for n in 0..50 {
            w.submit(&a, a.encode(&n).unwrap());
        }
        w.submit(&b, b.encode(&"b").unwrap());
        w.flush_blocking();
        assert_eq!(a.load::<u32>().unwrap(), 49, "the latest state wins");
        assert_eq!(b.load::<String>().unwrap(), "b");
    }

    // Threads racing to save one counter: the bytes are made under the writer's lock, so the file always ends
    // with the final value, never an older one that was encoded earlier but queued later.
    #[test]
    fn racing_saves_never_leave_an_older_state_on_disk() {
        use std::sync::atomic::{AtomicU32, Ordering};
        for round in 0..20 {
            let dir = TempDir::new(&format!("writer-race-{round}"));
            let a = JsonStore::new(dir.0.join("a.json"), 1);
            let w = Arc::new(Writer::start().unwrap());
            let counter = Arc::new(AtomicU32::new(0));
            let threads: Vec<_> = (0..4)
                .map(|_| {
                    let (w, a, counter) = (Arc::clone(&w), a.clone(), Arc::clone(&counter));
                    std::thread::spawn(move || {
                        for _ in 0..100 {
                            w.submit_with(&a, || {
                                let n = counter.fetch_add(1, Ordering::SeqCst) + 1;
                                a.encode(&n)
                            });
                        }
                    })
                })
                .collect();
            for t in threads {
                t.join().unwrap();
            }
            w.flush_blocking();
            assert_eq!(a.load::<u32>().unwrap(), 400, "round {round}");
        }
    }

    #[test]
    fn a_ticket_is_durable_only_once_its_write_has_landed() {
        let dir = TempDir::new("writer-ticket");
        let a = JsonStore::new(dir.0.join("a.json"), 1);
        let w = Writer::start().unwrap();
        w.pause();
        let first = w.submit(&a, a.encode(&1).unwrap());
        let second = w.submit(&a, a.encode(&2).unwrap());
        assert!(!w.is_durable(first) && !w.is_durable(second));
        assert!(!a.path().exists(), "nothing is written while held");
        w.resume();
        w.flush_blocking();
        assert!(w.is_durable(first) && w.is_durable(second));
        assert_eq!(a.load::<u32>().unwrap(), 2, "coalesced to the newer");
    }

    // REQ-CFG-085: the wait on a stop signal is bounded, so a disk that does not answer cannot hold the program.
    #[test]
    fn flush_within_gives_up_at_its_limit_and_reports_whether_all_is_written() {
        let dir = TempDir::new("writer-flush-within");
        let a = JsonStore::new(dir.0.join("a.json"), 1);
        let w = Writer::start().unwrap();
        assert!(
            w.flush_within(std::time::Duration::ZERO),
            "nothing to write"
        );
        w.pause();
        w.submit(&a, a.encode(&7).unwrap());
        let began = std::time::Instant::now();
        assert!(!w.flush_within(std::time::Duration::from_millis(60)));
        assert!(began.elapsed() < std::time::Duration::from_secs(5));
        assert!(!a.path().exists());
        w.resume();
        assert!(w.flush_within(std::time::Duration::from_secs(30)));
        assert_eq!(a.load::<u32>().unwrap(), 7);
    }

    fn block(path: &Path) -> PathBuf {
        let blocker = crate::store::temp_path(path);
        std::fs::create_dir(&blocker).unwrap();
        blocker
    }

    // REQ-INB-004: a ticket resolves to success or failure, per file, and a later write that works reports success.
    #[test]
    fn a_failed_write_is_reported_to_its_own_waiter_only_and_a_later_one_succeeds() {
        let dir = TempDir::new("writer-fail");
        let (bad, good) = (
            JsonStore::new(dir.0.join("bad.json"), 1),
            JsonStore::new(dir.0.join("good.json"), 1),
        );
        let blocker = block(bad.path());
        let w = Writer::start().unwrap();
        let mark = w.mark();
        w.pause();
        let t_bad = w.submit(&bad, bad.encode(&1).unwrap());
        let t_good = w.submit(&good, good.encode(&2).unwrap());
        w.resume();
        w.flush_blocking();
        let is = |p: &Path, name: &str| p.ends_with(name);
        assert!(w.outcome(t_bad - 1, t_bad, &|p| is(p, "bad.json")).is_err());
        assert!(
            w.outcome(t_good - 1, t_good, &|p| is(p, "good.json"))
                .is_ok(),
            "the file that was written is not blamed for the one that was not, though they shared a batch"
        );
        assert!(w.failed_since(mark, &|_| true));
        assert!(!w.failed_since(mark, &|p| is(p, "good.json")));
        assert_eq!(good.load::<u32>().unwrap(), 2);
        // Space comes back: the same file now saves, and a waiter after the mark sees success.
        std::fs::remove_dir(&blocker).unwrap();
        let mark = w.mark();
        w.submit(&bad, bad.encode(&3).unwrap());
        w.flush_blocking();
        assert!(!w.failed_since(mark, &|_| true));
        assert_eq!(bad.load::<u32>().unwrap(), 3);
    }

    #[test]
    fn a_failure_is_never_lost_when_the_record_of_it_is_dropped() {
        let dir = TempDir::new("writer-forget");
        let bad = JsonStore::new(dir.0.join("bad.json"), 1);
        block(bad.path());
        let w = Writer::start().unwrap();
        let first = w.mark();
        for n in 0..(KEEP_FAILURES + 5) {
            w.submit(&bad, bad.encode(&n).unwrap());
            w.flush_blocking();
        }
        // The oldest records were dropped; asking about them is answered "failed", not "fine".
        assert!(w.failed_since(first, &|_| true));
        assert!(w.outcome(0, 1, &|_| false).is_err());
    }

    #[test]
    fn a_full_disk_is_told_apart() {
        let full = StoreError::Io(std::io::Error::from(std::io::ErrorKind::StorageFull));
        assert!(SaveError::from_store(&full).disk_full);
        let other = StoreError::Io(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
        assert!(!SaveError::from_store(&other).disk_full);
    }

    #[test]
    fn dropping_the_writer_writes_what_is_still_queued() {
        let dir = TempDir::new("writer-drop");
        let a = JsonStore::new(dir.0.join("a.json"), 1);
        let w = Writer::start().unwrap();
        w.submit(&a, a.encode(&7).unwrap());
        drop(w);
        assert_eq!(a.load::<u32>().unwrap(), 7);
    }
}
