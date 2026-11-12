//! The core's side of the satellite protocol (spec 0003, REQ-SAT-001..010).
//!
//! A satellite is a microphone and speaker that knows nothing about assistants. It connects, says `hello`,
//! and then streams microphone audio when asked and plays what it is sent. Everything else happens here: the
//! [`Dialog`] decides who may listen, the endpointer finds the end of the utterance, the speech providers turn
//! audio into text and back, and a *responder* (reached through a channel, so the assistant keeps one owner)
//! answers.
//!
//! A session is generic over its transport, so the tests run it over in-memory pipes and the binary runs it
//! over TCP. Limits that protect a small device: connections are capped, the `hello` must arrive quickly, a
//! silent peer is dropped after the keepalive timeout, and frames are size-limited by the protocol decoder.

use crate::assistant::{Effect, MediaCmd};
use crate::cancel::CancelToken;
use crate::inbox::endpoint::verify_token;
use crate::rtvoice::{Claim, Done, Job, Realtime, Speaker, resample};
use crate::session::is_cancel;
use crate::speech::decode_pcm16;
use crate::voice::{
    Config as DialogConfig, Cue, Dialog, Phase, Step, Synthesizer, Transcriber, VoiceOutcome,
    speak_text, voice_turn,
};
use clawdot_protocol::frame::{Decoder, Limits, encode};
use clawdot_protocol::message::{
    AudioCodec, AudioFormat, Indicator, MediaAction, Message, PROTOCOL_VERSION, Reject, Welcome,
};
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{Semaphore, mpsc, oneshot};

/// A question for the assistant, with a place to send the answer.
#[derive(Debug)]
pub struct Ask {
    /// Which speaker said it: one connection, one conversation (REQ-AST-026).
    pub speaker: String,
    /// What the user said.
    pub text: String,
    /// Where the answer goes.
    pub reply: oneshot::Sender<Answer>,
    /// Where plan and progress phrases go while the turn runs (REQ-AST-020).
    pub progress: Option<mpsc::UnboundedSender<String>>,
    /// Set to stop the turn (REQ-AST-022).
    pub cancel: CancelToken,
}

/// The assistant's answer.
#[derive(Debug, Clone, Default)]
pub struct Answer {
    /// What to say.
    pub say: String,
    /// Side effects for the satellite.
    pub effects: Vec<Effect>,
    /// The answer ends with a question, so listen longer for the reply (REQ-AST-021).
    pub expects_reply: bool,
}

/// What a satellite's administrator-reset request does (REQ-SAT-020): open the local reset window and return the
/// seconds it stays open, or a short reason in plain words.
#[derive(Clone)]
pub struct AdminResetHook(Arc<dyn Fn() -> Result<u32, String> + Send + Sync>);

impl AdminResetHook {
    /// Wrap the function the core provides.
    pub fn new(f: impl Fn() -> Result<u32, String> + Send + Sync + 'static) -> Self {
        Self(Arc::new(f))
    }
}

impl std::fmt::Debug for AdminResetHook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AdminResetHook")
    }
}

/// A satellite may ask for the reset window at most this often.
const RESET_ASK_EVERY: Duration = Duration::from_secs(5);

/// Server settings.
#[derive(Debug, Clone)]
pub struct Settings {
    /// Hex SHA-256 hashes of accepted satellite tokens. Empty means no token is required, which is only
    /// reasonable on a trusted loopback or home network (REQ-SAT-010).
    pub token_hashes: Vec<String>,
    /// Dialog behaviour.
    pub dialog: DialogConfig,
    /// How often to ping.
    pub keepalive: Duration,
    /// Drop a peer that has sent nothing for this long.
    pub dead_after: Duration,
    /// How long a new connection has to say hello.
    pub hello_timeout: Duration,
    /// Connections one address may hold at once, so one host cannot fill every session slot (T12).
    pub max_per_ip: usize,
    /// Simultaneous satellites.
    pub max_sessions: usize,
    /// Lets the rest of the program speak to connected satellites.
    pub announcer: Announcer,
    /// Opens the local administrator reset window when a satellite asks (REQ-SAT-020). `None` refuses.
    pub admin_reset: Option<AdminResetHook>,
    /// Set by [`serve`] for a connection from a loopback address; such a peer is on the device itself.
    pub peer_loopback: bool,
    /// The optional realtime voice (REQ-RTV-009). `None`, or not configured, keeps the normal path.
    pub realtime: Option<Realtime>,
    /// A realtime session ends after this long without speech from either side.
    pub rt_idle: Duration,
    /// After a realtime failure the normal path is used for this long (REQ-RTV-010).
    pub rt_cooldown: Duration,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            token_hashes: Vec::new(),
            dialog: DialogConfig::default(),
            keepalive: Duration::from_secs(5),
            dead_after: Duration::from_secs(20),
            hello_timeout: Duration::from_secs(5),
            max_per_ip: 2,
            max_sessions: 4,
            announcer: Announcer::default(),
            admin_reset: None,
            peer_loopback: false,
            realtime: None,
            rt_idle: Duration::from_secs(10),
            rt_cooldown: Duration::from_secs(60),
        }
    }
}

enum Request {
    /// Would a spoken announcement be accepted right now?
    Probe,
    /// Speak this text.
    Speak(String),
}

struct Announce {
    request: Request,
    answer: oneshot::Sender<bool>,
}

#[derive(Debug, Default)]
struct AnnouncerInner {
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
struct Registration {
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

impl Announcer {
    fn register(&self) -> (Registration, mpsc::Receiver<Announce>) {
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
        self.ask(|| Request::Speak(text.to_owned())).await
    }
}

/// Samples per downlink audio frame (about 0.26 s at 16 kHz).
const DOWNLINK_CHUNK: usize = 4096;
/// Volume change for "louder" and "quieter", in percent.
const VOLUME_STEP: u8 = 10;

/// What a turn produced: what was heard and said, and the satellite effects.
struct Finished {
    outcome: VoiceOutcome,
    effects: Vec<Effect>,
    /// The answer ends with a question, so the follow-up window is longer (REQ-AST-021).
    asked: bool,
}

/// A running turn.
type Work = Pin<Box<dyn Future<Output = Finished> + Send>>;

/// What speech made outside a turn is for.
enum Purpose {
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
struct Synth {
    purpose: Purpose,
    work: Pin<Box<dyn Future<Output = Vec<Vec<i16>>> + Send>>,
}

/// A short check of audio heard while a turn runs: was it a stop phrase (REQ-VOI-020)?
type StopCheck = Pin<Box<dyn Future<Output = bool> + Send>>;

/// What the satellite's speaker is doing for this session. One value, because the kinds exclude each other: a
/// realtime session starts only when the speaker is quiet, and progress speech and announcements start only
/// when it is idle, so no two are ever on at once.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Audio {
    /// Nothing is playing.
    #[default]
    Idle,
    /// A reply or an announcement.
    Speech,
    /// A phrase spoken while the turn works; the dialog does not move on when it ends.
    Progress,
    /// A reply from the realtime session; its end must not open the follow-up window.
    Realtime,
}

/// What the music on the satellite is doing. Paused-for-speech is a state of its own, so music cannot be both
/// playing and waiting to be resumed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Music {
    /// No music, or it was stopped.
    #[default]
    Off,
    /// Music is playing.
    Playing,
    /// Music was paused for speech and is to be resumed afterwards.
    Ducked,
}

/// What the satellite is playing: speech and music.
#[derive(Debug, Default)]
struct Output {
    audio: Audio,
    music: Music,
}

/// Model audio that stops arriving for this long ends the reply on the speaker.
const RT_GAP: Duration = Duration::from_millis(300);
/// Microphone samples above this peak count as someone speaking, for the idle timeout.
const RT_LOUD: u16 = 1500;

/// One open realtime session, from this speaker's side (REQ-RTV-009).
struct RtLive {
    /// Microphone audio to the session. Dropping it ends the session.
    mic: Option<mpsc::Sender<Vec<u8>>>,
    speaker: mpsc::Receiver<Speaker>,
    speaker_open: bool,
    done: oneshot::Receiver<Done>,
    /// Frees the single session slot when this is dropped.
    _claim: Claim,
    /// An odd byte of model audio waiting for its other half.
    carry: Option<u8>,
    /// A reply is being streamed to the speaker (started, not yet stopped).
    streaming: bool,
    last_audio: Instant,
    last_activity: Instant,
}

enum RtEv {
    Out(Speaker),
    Gap,
    Done(Option<Done>),
}

/// Start a session for a speaker that is idle, unmuted and not in its cool-down, or `None` to use the normal
/// path. The claim makes sure only one session runs at a time.
fn rt_start(
    settings: &Settings,
    speaker_id: &str,
    quiet: bool,
    blocked_until: Option<Instant>,
) -> Option<RtLive> {
    if !quiet || blocked_until.is_some_and(|t| Instant::now() < t) {
        return None;
    }
    let claim = settings.realtime.as_ref()?.claim()?;
    let (mic_tx, mic) = mpsc::channel(64);
    let (speaker_tx, speaker) = mpsc::channel(256);
    let (done_tx, done) = oneshot::channel();
    if !claim.submit(Job {
        speaker_id: speaker_id.to_owned(),
        mic,
        speaker: speaker_tx,
        done: done_tx,
    }) {
        return None;
    }
    let now = Instant::now();
    Some(RtLive {
        mic: Some(mic_tx),
        speaker,
        speaker_open: true,
        done,
        _claim: claim,
        carry: None,
        streaming: false,
        last_audio: now,
        last_activity: now,
    })
}

async fn rt_next(rt: &mut Option<RtLive>) -> RtEv {
    let Some(r) = rt else {
        return std::future::pending().await;
    };
    let (streaming, deadline) = (r.streaming, r.last_audio + RT_GAP);
    let gap = async move {
        if streaming {
            tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)).await;
        } else {
            std::future::pending::<()>().await;
        }
    };
    tokio::pin!(gap);
    loop {
        tokio::select! {
            sp = r.speaker.recv(), if r.speaker_open => match sp {
                Some(s) => return RtEv::Out(s),
                // The session dropped its audio channel; its end is reported through `done`.
                None => r.speaker_open = false,
            },
            d = &mut r.done => return RtEv::Done(d.ok()),
            () = &mut gap => return RtEv::Gap,
        }
    }
}

/// Microphone audio (16 kHz) goes to the session at 24 kHz. A full queue drops audio rather than stalling the
/// connection.
fn rt_mic(r: &mut RtLive, payload: &[u8]) {
    let pcm = decode_pcm16(payload);
    if pcm.iter().any(|s| s.unsigned_abs() > RT_LOUD) {
        r.last_activity = Instant::now();
    }
    let bytes: Vec<u8> = resample(&pcm, 16_000, 24_000)
        .iter()
        .flat_map(|s| s.to_le_bytes())
        .collect();
    if let Some(tx) = &r.mic {
        let _ = tx.try_send(bytes);
    }
}

/// Model audio (24 kHz) goes to the speaker at 16 kHz through the same start, chunks and stop as any reply, with
/// music ducked first (REQ-RTV-009).
async fn rt_play<W: AsyncWrite + Unpin>(
    w: &mut W,
    r: &mut RtLive,
    bytes: &[u8],
    out: &mut Output,
) -> Result<(), String> {
    r.last_audio = Instant::now();
    r.last_activity = r.last_audio;
    let mut b: Vec<u8> = r.carry.take().into_iter().collect();
    b.extend_from_slice(bytes);
    if b.len() % 2 == 1 {
        r.carry = b.pop();
    }
    let pcm = resample(&decode_pcm16(&b), 24_000, 16_000);
    if pcm.is_empty() {
        return Ok(());
    }
    if !r.streaming {
        duck(w, &mut out.music).await?;
        send(w, &Message::SetIndicator(Indicator::Speaking)).await?;
        send(w, &Message::AudioStart(AudioFormat::PCM_16K_MONO)).await?;
        r.streaming = true;
        out.audio = Audio::Realtime;
    }
    for chunk in pcm.chunks(DOWNLINK_CHUNK) {
        let payload: Vec<u8> = chunk.iter().flat_map(|s| s.to_le_bytes()).collect();
        send(
            w,
            &Message::AudioChunk {
                format: AudioFormat::PCM_16K_MONO,
                timestamp_ms: None,
                payload,
            },
        )
        .await?;
    }
    Ok(())
}

/// Barge-in: stop what the speaker is playing from the session and forget what was queued (REQ-RTV-009).
async fn rt_flush<W: AsyncWrite + Unpin>(
    w: &mut W,
    r: Option<&mut RtLive>,
    audio: &mut Audio,
) -> Result<(), String> {
    if *audio == Audio::Realtime {
        send(w, &Message::StopPlayback).await?;
        *audio = Audio::Idle;
    }
    if let Some(r) = r {
        r.streaming = false;
        r.carry = None;
    }
    Ok(())
}

/// The write half of a session, with a bound on how long a write may make no progress. A satellite that stops
/// reading fills the pipe, and a plain write then waits for room forever; the loop is stuck inside it, so the
/// keepalive tick that would notice a dead peer never runs. With this wrapper a write that moves no byte for the
/// dead-peer time fails like a write to a closed connection, and the session ends. Progress of any size restarts
/// the clock, so a slow but live peer is not cut off.
struct TimedWrite<W> {
    inner: W,
    limit: Duration,
    stall: Pin<Box<tokio::time::Sleep>>,
    /// Whether `stall` is running for the write now waiting.
    waiting: bool,
}

impl<W> TimedWrite<W> {
    fn new(inner: W, limit: Duration) -> Self {
        Self {
            inner,
            limit,
            stall: Box::pin(tokio::time::sleep(limit)),
            waiting: false,
        }
    }

    /// Turn the result of one poll of the inner writer into the result for the caller: any answer ends the wait,
    /// and a wait that has gone on for the limit is an error.
    fn bounded<T>(
        &mut self,
        cx: &mut Context<'_>,
        polled: Poll<std::io::Result<T>>,
    ) -> Poll<std::io::Result<T>> {
        if polled.is_ready() {
            self.waiting = false;
            return polled;
        }
        if !self.waiting {
            self.waiting = true;
            self.stall
                .as_mut()
                .reset(tokio::time::Instant::now() + self.limit);
        }
        if self.stall.as_mut().poll(cx).is_ready() {
            self.waiting = false;
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "the peer is not reading",
            )));
        }
        Poll::Pending
    }
}

impl<W: AsyncWrite + Unpin> AsyncWrite for TimedWrite<W> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let polled = Pin::new(&mut self.inner).poll_write(cx, buf);
        self.bounded(cx, polled)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let polled = Pin::new(&mut self.inner).poll_flush(cx);
        self.bounded(cx, polled)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let polled = Pin::new(&mut self.inner).poll_shutdown(cx);
        self.bounded(cx, polled)
    }
}

async fn send<W: AsyncWrite + Unpin>(w: &mut W, m: &Message) -> Result<(), String> {
    let bytes = encode(&m.to_frame()).map_err(|e| e.to_string())?;
    w.write_all(&bytes).await.map_err(|e| format!("write: {e}"))
}

fn new_session_id() -> String {
    let mut b = [0u8; 8];
    // Falling back to zeros only weakens an identifier that is not a secret.
    let _ = getrandom::fill(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Read the next message, or `None` when the peer closed the connection.
pub(crate) async fn next_message<R: AsyncRead + Unpin>(
    r: &mut R,
    dec: &mut Decoder,
) -> Result<Option<Message>, String> {
    let mut buf = [0u8; 4096];
    loop {
        if let Some(frame) = dec.next_frame().map_err(|e| e.to_string())? {
            return Message::from_frame(frame)
                .map(Some)
                .map_err(|e| e.to_string());
        }
        let n = r.read(&mut buf).await.map_err(|e| format!("read: {e}"))?;
        if n == 0 {
            return Ok(None);
        }
        dec.push(&buf[..n]);
    }
}

/// Run one satellite connection to completion.
///
/// The session is a handshake followed by one event loop. The loop waits for the next event (a message from the
/// satellite, a finished turn, a realtime event, a progress phrase, a stop check, an announcement or the
/// keepalive tick), hands it to the function for that phase, and then runs the phases that follow every event:
/// wake handling, progress speech, delivery of a finished turn, and the dialog's steps.
///
/// # Errors
/// A message when the session ended abnormally: a framing violation, a timeout, or a write failure. A satellite
/// that is simply refused (bad token or version) ends with `Ok`.
pub async fn run_session<IO, S, T>(
    io: IO,
    stt: Arc<S>,
    tts: Arc<T>,
    asks: mpsc::Sender<Ask>,
    settings: Settings,
) -> Result<(), String>
where
    IO: AsyncRead + AsyncWrite + Unpin + Send,
    S: Transcriber + Send + Sync + 'static,
    T: Synthesizer + Send + Sync + 'static,
{
    let (mut rd, wr) = tokio::io::split(io);
    let mut wr = TimedWrite::new(wr, settings.dead_after);
    let mut dec = Decoder::new(Limits::default());

    let Some(accepted) = handshake(&mut rd, &mut wr, &mut dec, &settings).await? else {
        return Ok(());
    };
    let mut st = SessionState::new(&settings);
    let env = Env {
        settings,
        stt,
        tts,
        asks,
        speaker_id: accepted.speaker_id,
        media_url: accepted.media_url,
        started: Instant::now(),
    };

    loop {
        let mut pending = Pending::default();
        tokio::select! {
            msg = next_message(&mut rd, &mut dec) => {
                st.last_heard = Instant::now();
                let Some(msg) = msg? else { return Ok(()) };
                env.on_message(&mut st, &mut wr, msg, &mut pending).await?;
            }
            done = poll_work(&mut st.work), if st.work.is_some() => {
                st.on_turn_finished(done);
            }
            ev = rt_next(&mut st.rt), if st.rt.is_some() => {
                env.on_rt_event(&mut st, &mut wr, ev).await?;
            }
            p = recv_progress(&mut st.progress_rx), if st.progress_rx.is_some() => {
                match p {
                    Some(p) => st.progress_q.push_back(p),
                    None => st.progress_rx = None,
                }
            }
            stop = poll_stop(&mut st.stop_check), if st.stop_check.is_some() => {
                st.stop_check = None;
                if stop {
                    stop_turn(&mut st, &mut wr).await?;
                }
            }
            (purpose, audio) = poll_synth(&mut st.synth), if st.synth.is_some() => {
                env.on_synthesized(&mut st, &mut wr, purpose, audio).await?;
            }
            Some(a) = st.announce_rx.recv() => {
                env.on_announce(&mut st, a);
            }
            _ = st.tick.tick() => {
                env.on_tick(&mut st, &mut wr, &mut pending).await?;
            }
        }

        if pending.woke {
            env.on_wake(&mut st, &mut wr, &mut pending).await?;
        }
        st.end_realtime_when_muted();
        st.drop_stale_synthesis();
        env.play_progress(&mut st);
        env.deliver_finished(&mut st, &mut wr, &mut pending).await?;
        env.run_steps(&mut st, &mut wr, pending.steps).await?;
    }
}

/// Everything about a session that does not change once the handshake is done.
struct Env<S, T> {
    settings: Settings,
    stt: Arc<S>,
    tts: Arc<T>,
    asks: mpsc::Sender<Ask>,
    /// The connection's own identifier for the conversation and any held confirmation in the core.
    speaker_id: String,
    /// The satellite declared it can play URLs.
    media_url: bool,
    started: Instant,
}

/// What the handshake learned about an accepted satellite.
struct Accepted {
    speaker_id: String,
    media_url: bool,
}

/// What one pass of the event loop asks the phases after it to do.
#[derive(Default)]
struct Pending {
    /// What the dialog asked for during this pass.
    steps: Vec<Step>,
    /// The wake word was heard, or the action button was pressed with no realtime session or turn running. The
    /// wake word sets it during a turn too; the dialog then turns it into an interrupt.
    woke: bool,
}

/// The state one session shares across its phases.
struct SessionState {
    _registration: Registration,
    announce_rx: mpsc::Receiver<Announce>,
    dialog: Dialog,
    tick: tokio::time::Interval,
    last_heard: Instant,
    last_ping: Instant,
    volume: u8,
    /// What is on the speaker and what the music is doing.
    out: Output,
    /// Stream commands that wait for the speech to finish, so music never covers a reply.
    media_after_speech: Vec<Message>,
    work: Option<Work>,
    /// The running turn's cancel signal and the phrases it speaks while it works (REQ-AST-020, REQ-AST-022).
    turn_cancel: Option<CancelToken>,
    progress_rx: Option<mpsc::UnboundedReceiver<String>>,
    progress_q: VecDeque<String>,
    /// A finished turn (or the end of a realtime session) waits here until nothing is playing and the progress
    /// speech is spoken.
    finished: Option<Finished>,
    stop_check: Option<StopCheck>,
    /// Speech for an announcement or a progress phrase that is being made.
    synth: Option<Synth>,
    /// The optional realtime session and what goes with it (REQ-RTV-009, REQ-RTV-010).
    rt: Option<RtLive>,
    rt_blocked_until: Option<Instant>,
    warned_format: bool,
    /// A reset request is honoured from the device itself (loopback) or from a satellite that proved a token.
    reset_trusted: bool,
    last_reset_ask: Option<Instant>,
}

impl SessionState {
    fn new(settings: &Settings) -> Self {
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
            rt: None,
            rt_blocked_until: None,
            warned_format: false,
            reset_trusted: settings.peer_loopback || !settings.token_hashes.is_empty(),
            last_reset_ask: None,
        }
    }

    /// Nothing is happening that a new wake word or announcement would interrupt.
    fn is_quiet(&self) -> bool {
        self.dialog.phase() == Phase::Idle
            && !self.dialog.is_muted()
            && self.work.is_none()
            && self.finished.is_none()
            && self.out.audio == Audio::Idle
    }

    /// A progress phrase may be spoken now: the turn is working (or its answer waits), nothing is playing and
    /// the room is not muted.
    fn wants_progress_speech(&self) -> bool {
        self.out.audio == Audio::Idle
            && (self.work.is_some() || self.finished.is_some())
            && !self.dialog.is_muted()
    }

    /// Phase: speech that is being made for something that no longer applies is thrown away. An announcement
    /// needs a quiet speaker, so a wake word, a mute or a turn cancels it (its sender is told no); a progress
    /// phrase needs the turn.
    fn drop_stale_synthesis(&mut self) {
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
    fn end_realtime_when_muted(&mut self) {
        if self.dialog.is_muted()
            && let Some(r) = self.rt.as_mut()
        {
            r.mic = None;
        }
    }

    /// A turn finished: what it said while it worked comes before the answer.
    fn on_turn_finished(&mut self, done: Finished) {
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

/// The hello exchange (REQ-SAT-002): the first message must be a compatible, authorised hello, answered with
/// the welcome. `None` means the satellite was refused (or hung up) and the session is over.
async fn handshake<R, W>(
    rd: &mut R,
    wr: &mut W,
    dec: &mut Decoder,
    settings: &Settings,
) -> Result<Option<Accepted>, String>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let first = tokio::time::timeout(settings.hello_timeout, next_message(rd, dec))
        .await
        .map_err(|_| "no hello within the allowed time".to_owned())??;
    let hello = match first {
        Some(Message::Hello(h)) => h,
        Some(_) => {
            reject(wr, "the first message must be hello".into()).await?;
            return Ok(None);
        }
        None => return Ok(None),
    };
    if !PROTOCOL_VERSION.compatible_with(hello.protocol) {
        let reason = format!(
            "protocol {}.{} is not supported; this core speaks {}.{}",
            hello.protocol.major,
            hello.protocol.minor,
            PROTOCOL_VERSION.major,
            PROTOCOL_VERSION.minor
        );
        reject(wr, reason).await?;
        return Ok(None);
    }
    if !settings.token_hashes.is_empty() {
        let ok = hello
            .token
            .as_deref()
            .is_some_and(|t| settings.token_hashes.iter().any(|h| verify_token(t, h)));
        if !ok {
            reject(wr, "unauthorized".into()).await?;
            return Ok(None);
        }
    }
    // The connection's own random identifier. It names this speaker's conversation and held confirmation in the
    // core, so one speaker's "yes" can never answer another's question; a new connection starts fresh.
    let speaker_id = format!("sat:{}", new_session_id());
    send(
        wr,
        &Message::Welcome(Welcome {
            protocol: PROTOCOL_VERSION,
            session_id: new_session_id(),
            audio_out: AudioCodec::Pcm,
        }),
    )
    .await?;
    Ok(Some(Accepted {
        speaker_id,
        media_url: hello.capabilities.media_url,
    }))
}

async fn reject<W: AsyncWrite + Unpin>(w: &mut W, reason: String) -> Result<(), String> {
    send(w, &Message::Reject(Reject { reason })).await
}

/// The answer to an administrator-reset request: the seconds the window stays open, or a reason it did not.
fn reset_answer(settings: &Settings, trusted: bool, slow: bool) -> (u32, Option<String>) {
    match (&settings.admin_reset, trusted, slow) {
        (_, _, true) => (0, Some("Too many requests. Wait a few seconds.".to_owned())),
        (Some(hook), true, false) => match (hook.0)() {
            Ok(s) => (s, None),
            Err(why) => (0, Some(why)),
        },
        (Some(_), false, false) => (
            0,
            Some("This speaker is not trusted to open the reset window.".to_owned()),
        ),
        (None, _, _) => (0, Some("Not available.".to_owned())),
    }
}

/// Stop the running turn and the progress speech that is playing for it (REQ-VOI-020).
async fn stop_turn<W: AsyncWrite + Unpin>(st: &mut SessionState, wr: &mut W) -> Result<(), String> {
    if let Some(t) = &st.turn_cancel {
        t.cancel();
        st.progress_q.clear();
        if matches!(&st.synth, Some(s) if matches!(s.purpose, Purpose::Progress)) {
            st.synth = None;
        }
        if st.out.audio == Audio::Progress {
            send(wr, &Message::StopPlayback).await?;
        }
    }
    Ok(())
}

/// Resume music that was paused for speech (unless a stream command is waiting), then send the waiting commands.
async fn finish_speech<W: AsyncWrite + Unpin>(
    st: &mut SessionState,
    wr: &mut W,
) -> Result<(), String> {
    if st.out.music == Music::Ducked {
        // Paused music stays paused when a stream command is waiting; that command decides what plays.
        st.out.music = Music::Off;
        if st.media_after_speech.is_empty() {
            send(wr, &Message::MediaControl(MediaAction::Resume)).await?;
            st.out.music = Music::Playing;
        }
    }
    flush_media(wr, &mut st.media_after_speech, &mut st.out.music).await
}

/// The satellite finished playing what it was sent. Which kind of audio it was decides what happens next.
async fn on_playback_done<W: AsyncWrite + Unpin>(
    st: &mut SessionState,
    wr: &mut W,
    pending: &mut Pending,
) -> Result<(), String> {
    match std::mem::take(&mut st.out.audio) {
        Audio::Idle => {}
        Audio::Realtime => {
            // A reply from the session ended. The session itself goes on listening.
            if st.rt.is_some() {
                send(wr, &Message::SetIndicator(Indicator::Listening)).await?;
            }
        }
        Audio::Progress => {
            // Progress speech ended; the turn is still running, so the dialog does not move on.
            st.dialog.set_output_playing(false);
        }
        Audio::Speech => {
            finish_speech(st, wr).await?;
            pending.steps.push(st.dialog.on_playback_done());
        }
    }
    Ok(())
}

/// Volume and stop take effect now; stream commands wait until the satellite has finished speaking, so music
/// never covers the reply.
async fn apply_effects<W: AsyncWrite + Unpin>(
    st: &mut SessionState,
    wr: &mut W,
    media_url: bool,
    effects: &[Effect],
) -> Result<(), String> {
    for e in effects {
        match e {
            Effect::StopPlayback => {
                send(wr, &Message::StopPlayback).await?;
                // Music paused for speech is stopped too, or the end of the reply would resume it.
                st.out.music = Music::Off;
                st.media_after_speech.clear();
            }
            Effect::VolumeSet(l) => {
                st.volume = (*l).min(100);
                send(wr, &Message::SetVolume { level: st.volume }).await?;
            }
            Effect::VolumeUp => {
                st.volume = st.volume.saturating_add(VOLUME_STEP).min(100);
                send(wr, &Message::SetVolume { level: st.volume }).await?;
            }
            Effect::VolumeDown => {
                st.volume = st.volume.saturating_sub(VOLUME_STEP);
                send(wr, &Message::SetVolume { level: st.volume }).await?;
            }
            Effect::PlayStream { url, title } if media_url => {
                st.media_after_speech.push(Message::PlayUrl {
                    url: url.clone(),
                    title: title.clone(),
                });
            }
            Effect::PlayStream { .. } => {
                send(
                    wr,
                    &Message::Error(clawdot_protocol::message::ErrorReport {
                        text: "this satellite did not declare media_url, so it cannot play streams"
                            .into(),
                        code: Some("media-unsupported".into()),
                    }),
                )
                .await?;
            }
            Effect::Media(MediaCmd::Pause) if media_url => {
                st.media_after_speech
                    .push(Message::MediaControl(MediaAction::Pause));
            }
            Effect::Media(MediaCmd::Resume) if media_url => {
                st.media_after_speech
                    .push(Message::MediaControl(MediaAction::Resume));
            }
            Effect::Media(_) => {}
        }
    }
    Ok(())
}

impl<S, T> Env<S, T>
where
    S: Transcriber + Send + Sync + 'static,
    T: Synthesizer + Send + Sync + 'static,
{
    fn now_ms(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /// Phase: a message from the satellite.
    async fn on_message<W: AsyncWrite + Unpin>(
        &self,
        st: &mut SessionState,
        wr: &mut W,
        msg: Message,
        pending: &mut Pending,
    ) -> Result<(), String> {
        match msg {
            Message::Ping => send(wr, &Message::Pong).await?,
            Message::Wake(_) => pending.woke = true,
            Message::Button(b) if b.name == "action" => {
                if st.rt.is_some() {
                    // The action button ends a realtime session.
                    rt_flush(wr, st.rt.as_mut(), &mut st.out.audio).await?;
                    if let Some(r) = st.rt.as_mut() {
                        r.mic = None;
                    }
                } else if st.turn_cancel.is_some() && st.dialog.phase() == Phase::Thinking {
                    // While a turn is running the action button stops it (REQ-VOI-020).
                    stop_turn(st, wr).await?;
                } else {
                    pending.woke = true;
                }
            }
            Message::Button(b) if b.name == "mute" => {
                let muted = !st.dialog.is_muted();
                pending.steps.push(st.dialog.set_muted(muted));
                send(wr, &Message::SetMute { muted }).await?;
            }
            Message::MuteChanged { muted } => pending.steps.push(st.dialog.set_muted(muted)),
            Message::AdminReset => {
                let slow = st
                    .last_reset_ask
                    .is_some_and(|t| t.elapsed() < RESET_ASK_EVERY);
                st.last_reset_ask = Some(Instant::now());
                let (open_secs, reason) = reset_answer(&self.settings, st.reset_trusted, slow);
                send(wr, &Message::AdminResetWindow { open_secs, reason }).await?;
            }
            Message::AudioChunk {
                format, payload, ..
            } => {
                self.on_uplink_audio(st, wr, format, &payload, pending)
                    .await?
            }
            Message::PlaybackDone { .. } => on_playback_done(st, wr, pending).await?,
            // Pong, button names we do not use, errors, unknown events, and anything that only
            // the core should send are all ignored, so newer satellites keep working (REQ-SAT-009).
            _ => {}
        }
        Ok(())
    }

    /// Microphone audio goes to the realtime session if there is one, else to the dialog's endpointer.
    async fn on_uplink_audio<W: AsyncWrite + Unpin>(
        &self,
        st: &mut SessionState,
        wr: &mut W,
        format: AudioFormat,
        payload: &[u8],
        pending: &mut Pending,
    ) -> Result<(), String> {
        if format != AudioFormat::PCM_16K_MONO {
            if !st.warned_format {
                st.warned_format = true;
                send(
                    wr,
                    &Message::Error(clawdot_protocol::message::ErrorReport {
                        text: "uplink audio must be 16 kHz 16-bit mono".into(),
                        code: Some("bad-audio-format".into()),
                    }),
                )
                .await?;
            }
        } else if let Some(r) = st.rt.as_mut() {
            rt_mic(r, payload);
        } else {
            pending
                .steps
                .push(st.dialog.on_audio(self.now_ms(), &decode_pcm16(payload)));
        }
        Ok(())
    }

    /// Phase: an event from the realtime session.
    async fn on_rt_event<W: AsyncWrite + Unpin>(
        &self,
        st: &mut SessionState,
        wr: &mut W,
        ev: RtEv,
    ) -> Result<(), String> {
        match ev {
            RtEv::Out(Speaker::Audio(bytes)) => {
                if let Some(r) = st.rt.as_mut() {
                    rt_play(wr, r, &bytes, &mut st.out).await?;
                }
            }
            RtEv::Out(Speaker::Flush) => {
                rt_flush(wr, st.rt.as_mut(), &mut st.out.audio).await?;
            }
            RtEv::Gap => {
                if let Some(r) = st.rt.as_mut() {
                    r.streaming = false;
                }
                send(wr, &Message::AudioStop).await?;
            }
            RtEv::Done(done) => self.on_rt_done(st, wr, done).await?,
        }
        Ok(())
    }

    /// The realtime session ended: play what is left, then deliver its end like a finished turn.
    async fn on_rt_done<W: AsyncWrite + Unpin>(
        &self,
        st: &mut SessionState,
        wr: &mut W,
        done: Option<Done>,
    ) -> Result<(), String> {
        if let Some(mut r) = st.rt.take() {
            // Whatever the model said before it ended still gets played.
            while let Ok(Speaker::Audio(bytes)) = r.speaker.try_recv() {
                rt_play(wr, &mut r, &bytes, &mut st.out).await?;
            }
            if r.streaming {
                send(wr, &Message::AudioStop).await?;
            }
        }
        send(wr, &Message::StreamStop).await?;
        let done = done.unwrap_or(Done {
            speak: Some("I couldn't run the realtime voice.".into()),
            failed: true,
            effects: Vec::new(),
        });
        if done.failed {
            // Use the normal path for a while, so a broken service is not tried on every wake.
            st.rt_blocked_until = Some(Instant::now() + self.settings.rt_cooldown);
        }
        // The end of a session is delivered like a finished turn: after any audio still playing,
        // with its effects, and then either silence or the explanation in the normal voice.
        let mut outcome = VoiceOutcome {
            transcript: Some(String::new()),
            ..VoiceOutcome::default()
        };
        let mut closing = None;
        if let Some(text) = done.speak {
            outcome.transcript = None;
            outcome.reply = text.clone();
            closing = Some(text);
        }
        st.finished = Some(Finished {
            outcome,
            effects: done.effects,
            asked: false,
        });
        if let Some(text) = closing {
            // The speech is made while the session goes on reading (network synthesis can take long).
            // `deliver_finished` waits for it, and `on_synthesized` hands it to the waiting session end.
            st.synth = Some(Synth {
                purpose: Purpose::Closing,
                work: self.synthesize(text),
            });
        }
        Ok(())
    }

    /// Phase: someone elsewhere in the program wants this speaker to say something.
    fn on_announce(&self, st: &mut SessionState, a: Announce) {
        let Announce { request, answer } = a;
        let idle = st.is_quiet() && st.rt.is_none() && st.synth.is_none();
        match request {
            Request::Probe => {
                let _ = answer.send(idle);
            }
            // The speech is made while the session goes on reading; `on_synthesized` plays it and answers.
            Request::Speak(text) if idle => {
                st.synth = Some(Synth {
                    purpose: Purpose::Announcement(answer),
                    work: self.synthesize(text),
                });
            }
            Request::Speak(_) => {
                let _ = answer.send(false);
            }
        }
    }

    /// Phase: speech made outside a turn is ready. It plays only if the speaker is still in the state it was
    /// wanted in, since the user may have spoken, muted or stopped the turn while it was being made.
    async fn on_synthesized<W: AsyncWrite + Unpin>(
        &self,
        st: &mut SessionState,
        wr: &mut W,
        purpose: Purpose,
        audio: Vec<Vec<i16>>,
    ) -> Result<(), String> {
        match purpose {
            Purpose::Announcement(answer) => {
                let accepted = !audio.is_empty() && st.is_quiet() && st.rt.is_none();
                if accepted {
                    st.dialog.on_reply_started();
                    duck(wr, &mut st.out.music).await?;
                    play_audio(wr, &audio).await?;
                    st.out.audio = Audio::Speech;
                }
                let _ = answer.send(accepted);
            }
            Purpose::Closing => {
                if let Some(f) = st.finished.as_mut() {
                    f.outcome.audio = audio;
                }
            }
            Purpose::Progress => {
                if !audio.is_empty() && st.wants_progress_speech() {
                    st.dialog.set_output_playing(true);
                    duck(wr, &mut st.out.music).await?;
                    play_audio(wr, &audio).await?;
                    st.out.audio = Audio::Progress;
                }
            }
        }
        Ok(())
    }

    /// Make speech for `text` as a future the event loop polls.
    fn synthesize(&self, text: String) -> Pin<Box<dyn Future<Output = Vec<Vec<i16>>> + Send>> {
        let tts = Arc::clone(&self.tts);
        Box::pin(async move {
            let mut problems = Vec::new();
            speak_text(&*tts, &text, &mut problems).await
        })
    }

    /// Phase: the keepalive tick (liveness, ping, dialog timers, idle realtime sessions).
    async fn on_tick<W: AsyncWrite + Unpin>(
        &self,
        st: &mut SessionState,
        wr: &mut W,
        pending: &mut Pending,
    ) -> Result<(), String> {
        if st.last_heard.elapsed() >= self.settings.dead_after {
            return Err("the satellite stopped responding".into());
        }
        if st.last_ping.elapsed() >= self.settings.keepalive {
            st.last_ping = Instant::now();
            send(wr, &Message::Ping).await?;
        }
        pending.steps.push(st.dialog.tick(self.now_ms()));
        // A session nobody is talking in ends, so the microphone does not stream (and cost) for nothing.
        if let Some(r) = st.rt.as_mut()
            && st.out.audio == Audio::Idle
            && !r.streaming
            && r.last_activity.elapsed() >= self.settings.rt_idle
        {
            r.mic = None;
        }
        Ok(())
    }

    /// Phase: the wake word (or the action button). In a realtime session it cuts in; otherwise a realtime
    /// session starts if one is available and the speaker is quiet, else the dialog handles it.
    async fn on_wake<W: AsyncWrite + Unpin>(
        &self,
        st: &mut SessionState,
        wr: &mut W,
        pending: &mut Pending,
    ) -> Result<(), String> {
        if st.rt.is_some() {
            // The wake word during a session is the user cutting in.
            rt_flush(wr, st.rt.as_mut(), &mut st.out.audio).await?;
        } else {
            let quiet = st.is_quiet();
            match rt_start(&self.settings, &self.speaker_id, quiet, st.rt_blocked_until) {
                Some(live) => {
                    send(wr, &Message::SetIndicator(Indicator::Listening)).await?;
                    send(wr, &Message::StreamStart).await?;
                    st.rt = Some(live);
                }
                None => pending.steps.push(st.dialog.on_wake()),
            }
        }
        Ok(())
    }

    /// Phase: progress speech is played one phrase at a time while the turn goes on (REQ-AST-020).
    fn play_progress(&self, st: &mut SessionState) {
        if st.synth.is_none()
            && st.wants_progress_speech()
            && let Some(text) = st.progress_q.pop_front()
        {
            st.synth = Some(Synth {
                purpose: Purpose::Progress,
                work: self.synthesize(text),
            });
        }
    }

    /// Phase: the finished turn is delivered once nothing it said earlier is still playing.
    async fn deliver_finished<W: AsyncWrite + Unpin>(
        &self,
        st: &mut SessionState,
        wr: &mut W,
        pending: &mut Pending,
    ) -> Result<(), String> {
        // Nothing is delivered while a progress phrase is still being made: it comes first.
        let (playing, progress_empty) = (
            st.out.audio != Audio::Idle,
            st.progress_q.is_empty() && st.synth.is_none(),
        );
        let Some(Finished {
            outcome,
            effects,
            asked,
        }) = st.finished.take_if(|_| !playing && progress_empty)
        else {
            return Ok(());
        };
        if st.dialog.is_interrupting() {
            send(wr, &Message::StreamStop).await?;
        }
        apply_effects(st, wr, self.media_url, &effects).await?;
        if !outcome.audio.is_empty() {
            if asked {
                st.dialog.expect_answer();
            }
            st.dialog.on_reply_started();
            duck(wr, &mut st.out.music).await?;
            play_audio(wr, &outcome.audio).await?;
            st.out.audio = Audio::Speech;
        } else if outcome.transcript.is_some() && outcome.reply.is_empty() {
            // A command with nothing to say ("pause", "volume up"): do it now and go quiet. Music that a
            // realtime session paused comes back (nothing is playing now, or this would not run).
            finish_speech(st, wr).await?;
            if !st.dialog.is_muted() {
                send(wr, &Message::SetIndicator(Indicator::Idle)).await?;
            }
            pending.steps.push(st.dialog.on_reply_silent());
        } else {
            // The reply could not be spoken. The stream commands were the assistant's answer to the request
            // and do not depend on the speech, so they run now; left queued they would start after some later,
            // unrelated speech (REQ-MUS-004 only asks that music never covers speech, and none is playing).
            finish_speech(st, wr).await?;
            pending.steps.push(st.dialog.on_reply_failed());
        }
        Ok(())
    }

    /// Phase: carry out what the dialog asked for (cues, the interrupt check, starting a turn).
    async fn run_steps<W: AsyncWrite + Unpin>(
        &self,
        st: &mut SessionState,
        wr: &mut W,
        steps: Vec<Step>,
    ) -> Result<(), String> {
        for step in steps {
            match step {
                Step::Nothing => {}
                Step::Cue(cue) => cue_to_wire(cue, wr).await?,
                Step::Interrupt(pcm) => {
                    // The capture opened by a wake word while the turn runs: close it, and check what was
                    // said against the stop phrases only (REQ-VOI-020).
                    send(wr, &Message::StreamStop).await?;
                    send(wr, &Message::SetIndicator(Indicator::Thinking)).await?;
                    if !pcm.is_empty() && st.stop_check.is_none() {
                        let stt = Arc::clone(&self.stt);
                        st.stop_check = Some(Box::pin(async move {
                            stt.transcribe(&pcm).await.is_ok_and(|t| is_cancel(&t))
                        }));
                    }
                }
                Step::Utterance(pcm) => {
                    send(wr, &Message::StreamStop).await?;
                    send(wr, &Message::SetIndicator(Indicator::Thinking)).await?;
                    self.start_turn(st, pcm);
                }
            }
        }
        Ok(())
    }

    /// Start a turn on a captured utterance: transcribe, ask the assistant, synthesize the reply.
    fn start_turn(&self, st: &mut SessionState, pcm: Vec<i16>) {
        let (stt, tts, asks) = (
            Arc::clone(&self.stt),
            Arc::clone(&self.tts),
            self.asks.clone(),
        );
        let speaker = self.speaker_id.clone();
        let cancel = CancelToken::new();
        st.turn_cancel = Some(cancel.clone());
        let (progress_tx, rx) = mpsc::unbounded_channel();
        st.progress_rx = Some(rx);
        st.work = Some(Box::pin(async move {
            let effects = Arc::new(Mutex::new(Vec::new()));
            let sink = Arc::clone(&effects);
            let asked = Arc::new(AtomicBool::new(false));
            let asked_sink = Arc::clone(&asked);
            let outcome = voice_turn(&*stt, &*tts, &pcm, move |text| async move {
                let (tx, rx) = oneshot::channel();
                if asks
                    .send(Ask {
                        speaker,
                        text,
                        reply: tx,
                        progress: Some(progress_tx),
                        cancel,
                    })
                    .await
                    .is_err()
                {
                    return "Sorry, I can't answer right now.".to_owned();
                }
                match rx.await {
                    Ok(a) => {
                        sink.lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .extend(a.effects);
                        asked_sink.store(a.expects_reply, Ordering::Relaxed);
                        a.say
                    }
                    Err(_) => "Sorry, I can't answer right now.".to_owned(),
                }
            })
            .await;
            let effects =
                std::mem::take(&mut *effects.lock().unwrap_or_else(PoisonError::into_inner));
            Finished {
                outcome,
                effects,
                asked: asked.load(Ordering::Relaxed),
            }
        }));
    }
}

/// Show the speaking state and send a spoken reply: start, audio in pieces, stop.
async fn play_audio<W: AsyncWrite + Unpin>(w: &mut W, audio: &[Vec<i16>]) -> Result<(), String> {
    send(w, &Message::SetIndicator(Indicator::Speaking)).await?;
    send(w, &Message::AudioStart(AudioFormat::PCM_16K_MONO)).await?;
    for sentence in audio {
        for chunk in sentence.chunks(DOWNLINK_CHUNK) {
            let payload: Vec<u8> = chunk.iter().flat_map(|s| s.to_le_bytes()).collect();
            send(
                w,
                &Message::AudioChunk {
                    format: AudioFormat::PCM_16K_MONO,
                    timestamp_ms: None,
                    payload,
                },
            )
            .await?;
        }
    }
    send(w, &Message::AudioStop).await
}

/// Pause music for the duration of speech (REQ-MUS-004); resumed on playback-done.
async fn duck<W: AsyncWrite + Unpin>(w: &mut W, music: &mut Music) -> Result<(), String> {
    if *music == Music::Playing {
        send(w, &Message::MediaControl(MediaAction::Pause)).await?;
        *music = Music::Ducked;
    }
    Ok(())
}

/// Send the stream commands that were waiting for speech to finish, tracking whether music is now playing.
async fn flush_media<W: AsyncWrite + Unpin>(
    w: &mut W,
    queued: &mut Vec<Message>,
    music: &mut Music,
) -> Result<(), String> {
    for m in queued.drain(..) {
        match &m {
            Message::PlayUrl { .. } | Message::MediaControl(MediaAction::Resume) => {
                *music = Music::Playing;
            }
            Message::MediaControl(MediaAction::Pause) => *music = Music::Off,
            _ => {}
        }
        send(w, &m).await?;
    }
    Ok(())
}

async fn poll_work(work: &mut Option<Work>) -> Finished {
    match work {
        Some(w) => w.await,
        None => std::future::pending().await,
    }
}

async fn poll_synth(synth: &mut Option<Synth>) -> (Purpose, Vec<Vec<i16>>) {
    let Some(s) = synth.as_mut() else {
        return std::future::pending().await;
    };
    let audio = (&mut s.work).await;
    match synth.take() {
        Some(s) => (s.purpose, audio),
        None => std::future::pending().await,
    }
}

async fn poll_stop(check: &mut Option<StopCheck>) -> bool {
    match check {
        Some(c) => c.await,
        None => std::future::pending().await,
    }
}

async fn recv_progress(rx: &mut Option<mpsc::UnboundedReceiver<String>>) -> Option<String> {
    match rx {
        Some(r) => r.recv().await,
        None => std::future::pending().await,
    }
}

async fn cue_to_wire<W: AsyncWrite + Unpin>(cue: Cue, w: &mut W) -> Result<(), String> {
    match cue {
        Cue::Wake | Cue::Listening => {
            send(w, &Message::SetIndicator(Indicator::Listening)).await?;
            send(w, &Message::StreamStart).await
        }
        Cue::Thinking => send(w, &Message::SetIndicator(Indicator::Thinking)).await,
        Cue::Error => {
            send(w, &Message::StreamStop).await?;
            send(w, &Message::SetIndicator(Indicator::Error)).await
        }
        Cue::Muted => {
            send(w, &Message::StreamStop).await?;
            send(w, &Message::SetIndicator(Indicator::Muted)).await
        }
        Cue::Unmuted => send(w, &Message::SetIndicator(Indicator::Idle)).await,
    }
}

/// Accept satellites on `listener` until `shutdown` completes. Each connection is a task, capped at
/// `settings.max_sessions`; extra connections are closed at once.
///
/// # Errors
/// Fails if accepting connections fails.
pub async fn serve<S, T>(
    listener: TcpListener,
    stt: Arc<S>,
    tts: Arc<T>,
    asks: mpsc::Sender<Ask>,
    settings: Settings,
    shutdown: impl Future<Output = ()> + Send,
) -> std::io::Result<()>
where
    S: Transcriber + Send + Sync + 'static,
    T: Synthesizer + Send + Sync + 'static,
{
    let slots = Arc::new(Semaphore::new(settings.max_sessions));
    let per_ip = crate::limit::IpLimiter::new(settings.max_per_ip);
    tokio::pin!(shutdown);
    loop {
        let (stream, peer) = tokio::select! {
            accepted = listener.accept() => accepted?,
            () = &mut shutdown => return Ok(()),
        };
        let Some(ip_guard) = per_ip.try_acquire(peer.ip()) else {
            drop(stream);
            continue;
        };
        let Ok(permit) = Arc::clone(&slots).try_acquire_owned() else {
            drop(stream);
            continue;
        };
        let (stt, tts, asks, mut settings) = (
            Arc::clone(&stt),
            Arc::clone(&tts),
            asks.clone(),
            settings.clone(),
        );
        settings.peer_loopback = peer.ip().is_loopback();
        tokio::spawn(async move {
            let _permit = permit;
            let _ip_guard = ip_guard;
            if let Err(e) = run_session(stream, stt, tts, asks, settings).await {
                eprintln!("clawdot: satellite {peer}: {e}");
            }
        });
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod phase_tests;
