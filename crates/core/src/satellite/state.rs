//! The values a session passes between its phases: the turn being run, speech being made, the per-pass `Pending`,
//! the `Env` that does not change after the handshake, and the `SessionState`. Also the small futures the event loop
//! polls beside its other events.

use super::announcer::{Announce, Registration};
use super::playback::{Audio, Output};
use super::realtime::RtLive;
use super::reply::Reply;
use super::settings::{Ask, Settings};
use crate::assistant::Effect;
use crate::cancel::CancelToken;
use crate::voice::{Dialog, Phase, Step, VoiceOutcome};
use clawdot_protocol::message::Message;
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};

/// What a turn produced: what was heard and said, and the satellite effects.
pub(super) struct Finished {
    pub(super) outcome: VoiceOutcome,
    pub(super) effects: Vec<Effect>,
    /// The answer ends with a question, so the follow-up window is longer (REQ-AST-021).
    pub(super) asked: bool,
    /// The sentences of the reply, still to be synthesised and played one at a time (see `reply`). Empty when the
    /// audio is already in `outcome`, or there is nothing to say.
    pub(super) speak: Vec<String>,
}

/// A running turn.
pub(super) type Work = Pin<Box<dyn Future<Output = Finished> + Send>>;

/// What speech made outside a turn is for.
pub(super) enum Purpose {
    /// An announcement from elsewhere in the program; the sender is told whether it was spoken.
    Announcement(oneshot::Sender<bool>),
    /// A phrase spoken while a turn works.
    Progress,
    /// The closing line of a realtime session. The session's end waits in `SessionState::finished` without
    /// audio; the speech fills it in when it is made.
    Closing,
}

/// Speech being made for the speaker. The event loop polls it beside its other events, so a slow speech
/// provider never stops the session from reading the satellite or answering its keepalives.
pub(super) struct Synth {
    pub(super) purpose: Purpose,
    pub(super) work: Pin<Box<dyn Future<Output = Vec<Vec<i16>>> + Send>>,
}

/// A short check of audio heard while a turn runs: was it a stop phrase (REQ-VOI-020)?
pub(super) type StopCheck = Pin<Box<dyn Future<Output = bool> + Send>>;

/// Everything about a session that does not change once the handshake is done.
pub(super) struct Env<S, T> {
    pub(super) settings: Settings,
    pub(super) stt: Arc<S>,
    pub(super) tts: Arc<T>,
    pub(super) asks: mpsc::Sender<Ask>,
    /// The connection's own identifier for the conversation and any held confirmation in the core.
    pub(super) speaker_id: String,
    /// The satellite declared it can play URLs.
    pub(super) media_url: bool,
    pub(super) started: Instant,
}

/// What one pass of the event loop asks the phases after it to do.
#[derive(Default)]
pub(super) struct Pending {
    /// What the dialog asked for during this pass.
    pub(super) steps: Vec<Step>,
    /// The wake word was heard, or the action button was pressed with no realtime session or turn running. The
    /// wake word sets it during a turn too; the dialog then turns it into an interrupt.
    pub(super) woke: bool,
}

/// The state one session shares across its phases.
pub(super) struct SessionState {
    pub(super) _registration: Registration,
    pub(super) announce_rx: mpsc::Receiver<Announce>,
    pub(super) dialog: Dialog,
    pub(super) tick: tokio::time::Interval,
    pub(super) last_heard: Instant,
    pub(super) last_ping: Instant,
    pub(super) volume: u8,
    /// What is on the speaker and what the music is doing.
    pub(super) out: Output,
    /// Stream commands that wait for the speech to finish, so music never covers a reply.
    pub(super) media_after_speech: Vec<Message>,
    pub(super) work: Option<Work>,
    /// The running turn's cancel signal and the phrases it speaks while it works (REQ-AST-020, REQ-AST-022).
    pub(super) turn_cancel: Option<CancelToken>,
    pub(super) progress_rx: Option<mpsc::UnboundedReceiver<String>>,
    pub(super) progress_q: VecDeque<String>,
    /// A finished turn (or the end of a realtime session) waits here until nothing is playing and the progress
    /// speech is spoken.
    pub(super) finished: Option<Finished>,
    pub(super) stop_check: Option<StopCheck>,
    /// Speech for an announcement or a progress phrase that is being made.
    pub(super) synth: Option<Synth>,
    /// The reply of a finished turn while it is being synthesised and played, a sentence at a time.
    pub(super) reply: Option<Reply>,
    /// The optional realtime session and what goes with it (REQ-RTV-009, REQ-RTV-010).
    pub(super) rt: Option<RtLive>,
    pub(super) rt_blocked_until: Option<Instant>,
    pub(super) warned_format: bool,
    /// A reset request is honoured from the device itself (loopback) or from a satellite that proved a token.
    pub(super) reset_trusted: bool,
    pub(super) last_reset_ask: Option<Instant>,
}

impl SessionState {
    pub(super) fn new(settings: &Settings) -> Self {
        let (registration, announce_rx) = settings.announcer.register();
        Self {
            _registration: registration,
            announce_rx,
            dialog: Dialog::new(settings.dialog),
            tick: tokio::time::interval(
                settings
                    .keepalive
                    .min(Duration::from_secs(1))
                    .max(Duration::from_millis(10)),
            ),
            last_heard: Instant::now(),
            last_ping: Instant::now(),
            volume: 50,
            out: Output::default(),
            media_after_speech: Vec::new(),
            work: None,
            turn_cancel: None,
            progress_rx: None,
            progress_q: VecDeque::new(),
            finished: None,
            stop_check: None,
            synth: None,
            reply: None,
            rt: None,
            rt_blocked_until: None,
            warned_format: false,
            reset_trusted: settings.peer_loopback || !settings.token_hashes.is_empty(),
            last_reset_ask: None,
        }
    }

    /// Nothing is happening that a new wake word or announcement would interrupt.
    pub(super) fn is_quiet(&self) -> bool {
        self.dialog.phase() == Phase::Idle
            && !self.dialog.is_muted()
            && self.work.is_none()
            && self.finished.is_none()
            && self.reply.is_none()
            && self.out.audio == Audio::Idle
    }

    /// A progress phrase may be spoken now: the turn is working (or its answer waits), nothing is playing and
    /// the room is not muted.
    pub(super) fn wants_progress_speech(&self) -> bool {
        self.out.audio == Audio::Idle
            && (self.work.is_some() || self.finished.is_some())
            && !self.dialog.is_muted()
    }

    /// Phase: speech that is being made for something that no longer applies is thrown away. An announcement
    /// needs a quiet speaker, so a wake word, a mute or a turn cancels it (its sender is told no); a progress
    /// phrase needs the turn.
    pub(super) fn drop_stale_synthesis(&mut self) {
        let stale = match self.synth.as_ref().map(|s| &s.purpose) {
            Some(Purpose::Announcement(_)) => !(self.is_quiet() && self.rt.is_none()),
            Some(Purpose::Progress) => {
                self.dialog.is_muted() || (self.work.is_none() && self.finished.is_none())
            }
            // Its text belongs to the waiting session end; only that being gone makes it stale.
            Some(Purpose::Closing) => self.finished.is_none(),
            None => false,
        };
        if stale {
            self.synth = None;
        }
    }

    /// Phase: muting must really stop the microphone, so it ends a realtime session (the session ends when its
    /// microphone channel closes).
    pub(super) fn end_realtime_when_muted(&mut self) {
        if self.dialog.is_muted()
            && let Some(r) = self.rt.as_mut()
        {
            r.mic = None;
        }
    }

    /// A turn finished: what it said while it worked comes before the answer.
    pub(super) fn on_turn_finished(&mut self, done: Finished) {
        self.work = None;
        self.turn_cancel = None;
        if let Some(mut rx) = self.progress_rx.take() {
            while let Ok(p) = rx.try_recv() {
                self.progress_q.push_back(p);
            }
        }
        self.finished = Some(done);
    }
}

pub(super) async fn poll_work(work: &mut Option<Work>) -> Finished {
    match work {
        Some(w) => w.await,
        None => std::future::pending().await,
    }
}

pub(super) async fn poll_synth(synth: &mut Option<Synth>) -> (Purpose, Vec<Vec<i16>>) {
    let Some(s) = synth.as_mut() else {
        return std::future::pending().await;
    };
    let audio = (&mut s.work).await;
    match synth.take() {
        Some(s) => (s.purpose, audio),
        None => std::future::pending().await,
    }
}

pub(super) async fn poll_stop(check: &mut Option<StopCheck>) -> bool {
    match check {
        Some(c) => c.await,
        None => std::future::pending().await,
    }
}

pub(super) async fn recv_progress(
    rx: &mut Option<mpsc::UnboundedReceiver<String>>,
) -> Option<String> {
    match rx {
        Some(r) => r.recv().await,
        None => std::future::pending().await,
    }
}
