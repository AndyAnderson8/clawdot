//! How the rest of the program speaks to connected satellites: the `Announcer` handle and the registration each
//! session holds while it is connected.

#[cfg(test)]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::{mpsc, oneshot};

pub(super) enum Request {
    /// Would a spoken announcement be accepted right now?
    Probe,
    /// Speak this text, in `voice` when one is given (a portal preview) and in the configured voice otherwise.
    Speak { text: String, voice: Option<String> },
}

pub(super) struct Announce {
    pub(super) request: Request,
    pub(super) answer: oneshot::Sender<bool>,
}

#[derive(Debug, Default)]
pub(super) struct AnnouncerInner {
    sessions: Mutex<Vec<(u64, mpsc::Sender<Announce>)>>,
    next: AtomicU64,
}

/// How the rest of the program speaks to connected satellites (inbox announcements, alarms).
///
/// A satellite accepts an announcement only when it is idle and unmuted, so an announcement never interrupts a
/// conversation or plays into a muted room. After it plays, the follow-up window opens, so the user can answer
/// ("keep going and add tests") without the wake word.
#[derive(Debug, Clone, Default)]
pub struct Announcer {
    inner: Arc<AnnouncerInner>,
}

impl std::fmt::Debug for Announce {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Announce")
    }
}

/// Removes a session from the announcer when it ends.
pub(super) struct Registration {
    announcer: Announcer,
    id: u64,
}

impl Drop for Registration {
    fn drop(&mut self) {
        let mut g = self
            .announcer
            .inner
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        g.retain(|(id, _)| *id != self.id);
    }
}

#[cfg(test)]
impl Announcer {
    /// Connect a stand-in satellite for a test. It takes a line when `free` is true and refuses it (busy or
    /// muted) when false; every line it takes is sent on the returned channel. It disconnects when the channel
    /// is dropped and the task ends with the runtime.
    pub(crate) fn connect_fake(&self, free: Arc<AtomicBool>) -> mpsc::UnboundedReceiver<String> {
        let (registration, mut rx) = self.register();
        let (spoken, heard) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let _registration = registration;
            while let Some(Announce { request, answer }) = rx.recv().await {
                let idle = free.load(Ordering::Relaxed);
                let _ = match request {
                    Request::Probe => answer.send(idle),
                    Request::Speak { text, .. } if idle => {
                        let _ = spoken.send(text);
                        answer.send(true)
                    }
                    Request::Speak { .. } => answer.send(false),
                };
            }
        });
        heard
    }
}

impl Announcer {
    pub(super) fn register(&self) -> (Registration, mpsc::Receiver<Announce>) {
        let (tx, rx) = mpsc::channel(4);
        let id = self.inner.next.fetch_add(1, Ordering::Relaxed);
        self.inner
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((id, tx));
        (
            Registration {
                announcer: self.clone(),
                id,
            },
            rx,
        )
    }

    /// How many satellites are connected.
    #[must_use]
    pub fn connected(&self) -> usize {
        self.inner
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    async fn ask(&self, make: impl Fn() -> Request) -> bool {
        let senders: Vec<_> = self
            .inner
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|(_, s)| s.clone())
            .collect();
        for s in senders {
            let (tx, rx) = oneshot::channel();
            if s.send(Announce {
                request: make(),
                answer: tx,
            })
            .await
            .is_ok()
                && rx.await == Ok(true)
            {
                return true;
            }
        }
        false
    }

    /// Whether some satellite could speak an announcement right now.
    pub async fn is_available(&self) -> bool {
        self.ask(|| Request::Probe).await
    }

    /// Speak `text` on the first satellite that is free. Returns whether anyone did.
    pub async fn announce(&self, text: &str) -> bool {
        self.announce_in_voice(text, None).await
    }

    /// Like [`Announcer::announce`], in `voice` for this one line when given. The voice travels with the
    /// request, so the voice every other line uses is never touched.
    pub async fn announce_in_voice(&self, text: &str, voice: Option<&str>) -> bool {
        self.ask(|| Request::Speak {
            text: text.to_owned(),
            voice: voice.map(str::to_owned),
        })
        .await
    }
}
