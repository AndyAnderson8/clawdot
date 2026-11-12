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
//! Durability (REQ-INB-004): the HTTP layer awaits [`Writer::flushed`] before it answers, so an event that was
//! acknowledged to a caller is on disk. Dropping the writer writes everything still queued.

use crate::store::JsonStore;
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

#[derive(Debug, Default)]
struct State {
    /// At most one entry per file: a newer write replaces an older one that has not started.
    pending: Vec<(JsonStore, Vec<u8>)>,
    /// Writes handed in so far.
    submitted: u64,
    /// Writes handed in before the last batch started that have now finished.
    written: u64,
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
        if let Some(slot) = st
            .pending
            .iter_mut()
            .find(|(s, _)| s.path() == store.path())
        {
            slot.1 = bytes;
        } else {
            st.pending.push((store.clone(), bytes));
        }
        st.submitted
    }

    /// Whether the write behind `ticket` (and everything before it) has finished.
    pub fn is_durable(&self, ticket: u64) -> bool {
        self.shared.lock().written >= ticket
    }

    /// Resolves once the write behind `ticket` (and everything before it) has finished, or failed and been
    /// logged.
    pub async fn wait(&self, ticket: u64) {
        let mut rx = self.shared.progress.subscribe();
        // The sender lives as long as `self`, so this cannot fail while we hold it.
        let _ = rx.wait_for(|written| *written >= ticket).await;
    }

    /// Resolves once everything submitted before the call is on disk (or has failed and been logged).
    pub async fn flushed(&self) {
        let target = self.shared.lock().submitted;
        let mut rx = self.shared.progress.subscribe();
        // The sender lives as long as `self`, so this cannot fail while we hold it.
        let _ = rx.wait_for(|written| *written >= target).await;
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
        for (store, bytes) in batch {
            // A failed write must not lose the in-memory event, but it must not be silent either.
            if let Err(e) = store.write(&bytes) {
                crate::diag::error(&format!("could not save {}: {e}", store.path().display()));
            }
        }
        shared.lock().written = upto;
        shared.progress.send_replace(upto);
        shared.done.notify_all();
    }
}

/// A write that was queued and may not be on disk yet.
#[derive(Debug, Clone)]
pub struct Durability {
    writer: Arc<Writer>,
    ticket: u64,
}

impl Durability {
    /// Name the write behind `ticket`.
    #[must_use]
    pub fn new(writer: Arc<Writer>, ticket: u64) -> Self {
        Self { writer, ticket }
    }

    /// The write has finished (or failed and been logged).
    #[must_use]
    pub fn is_done(&self) -> bool {
        self.writer.is_durable(self.ticket)
    }

    /// Resolves once the write has finished.
    pub async fn wait(self) {
        self.writer.wait(self.ticket).await;
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
