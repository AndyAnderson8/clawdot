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

/// A running turn: what was heard and said, the satellite effects, and whether it ended with a question.
type Work = Pin<Box<dyn Future<Output = (VoiceOutcome, Vec<Effect>, bool)> + Send>>;

/// A short check of audio heard while a turn runs: was it a stop phrase (REQ-VOI-020)?
type StopCheck = Pin<Box<dyn Future<Output = bool> + Send>>;

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
    SpeakerClosed,
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
    tokio::select! {
        sp = r.speaker.recv(), if r.speaker_open => match sp {
            Some(s) => RtEv::Out(s),
            None => {
                r.speaker_open = false;
                RtEv::SpeakerClosed
            }
        },
        d = &mut r.done => RtEv::Done(d.ok()),
        () = gap => RtEv::Gap,
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
    playing: &mut bool,
    rt_audio: &mut bool,
    music: &mut bool,
    ducked: &mut bool,
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
        duck(w, music, ducked).await?;
        send(w, &Message::SetIndicator(Indicator::Speaking)).await?;
        send(w, &Message::AudioStart(AudioFormat::PCM_16K_MONO)).await?;
        r.streaming = true;
        *playing = true;
        *rt_audio = true;
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
    playing: &mut bool,
    rt_audio: &mut bool,
) -> Result<(), String> {
    if *rt_audio {
        send(w, &Message::StopPlayback).await?;
        *playing = false;
        *rt_audio = false;
    }
    if let Some(r) = r {
        r.streaming = false;
        r.carry = None;
    }
    Ok(())
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
    let (mut rd, mut wr) = tokio::io::split(io);
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
            (outcome, effects, asked) = poll_work(&mut st.work), if st.work.is_some() => {
                st.on_turn_finished(outcome, effects, asked);
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
            Some(a) = st.announce_rx.recv() => {
                env.on_announce(&mut st, &mut wr, a).await?;
            }
            _ = st.tick.tick() => {
                env.on_tick(&mut st, &mut wr, &mut pending).await?;
            }
        }

        if pending.woke {
            env.on_wake(&mut st, &mut wr, &mut pending).await?;
        }
        // Muting must really stop the microphone, so it ends a session.
        if st.dialog.is_muted()
            && let Some(r) = st.rt.as_mut()
        {
            r.mic = None;
        }
        env.play_progress(&mut st, &mut wr).await?;
        env.deliver_ready(&mut st, &mut wr, &mut pending).await?;
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
    /// The wake word or the action button was pressed while no turn was running.
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
    /// Audio is on the speaker (a reply, progress speech, an announcement or a realtime reply).
    playing: bool,
    /// Music is playing on the satellite.
    music: bool,
    /// Music was paused for speech and is to be resumed afterwards.
    ducked: bool,
    /// Stream commands that wait for the speech to finish, so music never covers a reply.
    after: Vec<Message>,
    work: Option<Work>,
    /// The running turn's cancel signal and the phrases it speaks while it works (REQ-AST-020, REQ-AST-022).
    turn_cancel: Option<CancelToken>,
    progress_rx: Option<mpsc::UnboundedReceiver<String>>,
    progress_q: VecDeque<String>,
    progress_playing: bool,
    /// A finished turn waits here while its progress speech is still playing.
    ready: Option<(VoiceOutcome, Vec<Effect>, bool)>,
    stop_check: Option<StopCheck>,
    /// The optional realtime session and what goes with it (REQ-RTV-009, REQ-RTV-010).
    rt: Option<RtLive>,
    /// Audio from a session is on the speaker, so its playback-done must not open the follow-up window.
    rt_audio: bool,
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
            playing: false,
            music: false,
            ducked: false,
            after: Vec::new(),
            work: None,
            turn_cancel: None,
            progress_rx: None,
            progress_q: VecDeque::new(),
            progress_playing: false,
            ready: None,
            stop_check: None,
            rt: None,
            rt_audio: false,
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
            && self.ready.is_none()
            && !self.playing
    }

    /// A turn finished: what it said while it worked comes before the answer.
    fn on_turn_finished(&mut self, outcome: VoiceOutcome, effects: Vec<Effect>, asked: bool) {
        self.work = None;
        self.turn_cancel = None;
        if let Some(mut rx) = self.progress_rx.take() {
            while let Ok(p) = rx.try_recv() {
                self.progress_q.push_back(p);
            }
        }
        self.ready = Some((outcome, effects, asked));
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
        if st.progress_playing {
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
    if st.ducked {
        st.ducked = false;
        if st.after.is_empty() {
            send(wr, &Message::MediaControl(MediaAction::Resume)).await?;
            st.music = true;
        }
    }
    flush_media(wr, &mut st.after, &mut st.music).await
}

/// The satellite finished playing what it was sent. Which kind of audio it was decides what happens next.
async fn on_playback_done<W: AsyncWrite + Unpin>(
    st: &mut SessionState,
    wr: &mut W,
    pending: &mut Pending,
) -> Result<(), String> {
    if !st.playing {
        return Ok(());
    }
    if st.rt_audio {
        // A reply from the session ended. The session itself goes on listening.
        st.playing = false;
        st.rt_audio = false;
        if st.rt.is_some() {
            send(wr, &Message::SetIndicator(Indicator::Listening)).await?;
        }
    } else if st.progress_playing {
        // Progress speech ended; the turn is still running, so the dialog does not move on.
        st.playing = false;
        st.progress_playing = false;
        st.dialog.set_output_playing(false);
    } else {
        st.playing = false;
        finish_speech(st, wr).await?;
        pending.steps.push(st.dialog.on_playback_done());
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
                st.music = false;
                st.after.clear();
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
                st.after.push(Message::PlayUrl {
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
                st.after.push(Message::MediaControl(MediaAction::Pause));
            }
            Effect::Media(MediaCmd::Resume) if media_url => {
                st.after.push(Message::MediaControl(MediaAction::Resume));
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
                    rt_flush(wr, st.rt.as_mut(), &mut st.playing, &mut st.rt_audio).await?;
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
                    rt_play(
                        wr,
                        r,
                        &bytes,
                        &mut st.playing,
                        &mut st.rt_audio,
                        &mut st.music,
                        &mut st.ducked,
                    )
                    .await?;
                }
            }
            RtEv::Out(Speaker::Flush) => {
                rt_flush(wr, st.rt.as_mut(), &mut st.playing, &mut st.rt_audio).await?;
            }
            RtEv::SpeakerClosed => {}
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
                rt_play(
                    wr,
                    &mut r,
                    &bytes,
                    &mut st.playing,
                    &mut st.rt_audio,
                    &mut st.music,
                    &mut st.ducked,
                )
                .await?;
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
        if let Some(text) = done.speak {
            let mut problems = Vec::new();
            outcome.audio = speak_text(&*self.tts, &text, &mut problems).await;
            outcome.transcript = None;
            outcome.reply = text;
        }
        st.ready = Some((outcome, done.effects, false));
        Ok(())
    }

    /// Phase: someone elsewhere in the program wants this speaker to say something.
    async fn on_announce<W: AsyncWrite + Unpin>(
        &self,
        st: &mut SessionState,
        wr: &mut W,
        a: Announce,
    ) -> Result<(), String> {
        let idle = st.is_quiet() && st.rt.is_none();
        let accepted = match a.request {
            Request::Probe => idle,
            Request::Speak(text) if idle => {
                let mut problems = Vec::new();
                let audio = speak_text(&*self.tts, &text, &mut problems).await;
                if audio.is_empty() {
                    false
                } else {
                    st.dialog.on_reply_started();
                    duck(wr, &mut st.music, &mut st.ducked).await?;
                    play_audio(wr, &audio).await?;
                    st.playing = true;
                    true
                }
            }
            Request::Speak(_) => false,
        };
        let _ = a.answer.send(accepted);
        Ok(())
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
            && !st.playing
            && !r.streaming
            && r.last_activity.elapsed() >= self.settings.rt_idle
        {
            r.mic = None;
        }
        Ok(())
    }

    /// Phase: the wake word (or the action button) while no turn runs. Starts a realtime session when one is
    /// available, else the normal dialog.
    async fn on_wake<W: AsyncWrite + Unpin>(
        &self,
        st: &mut SessionState,
        wr: &mut W,
        pending: &mut Pending,
    ) -> Result<(), String> {
        if st.rt.is_some() {
            // The wake word during a session is the user cutting in.
            rt_flush(wr, st.rt.as_mut(), &mut st.playing, &mut st.rt_audio).await?;
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
    async fn play_progress<W: AsyncWrite + Unpin>(
        &self,
        st: &mut SessionState,
        wr: &mut W,
    ) -> Result<(), String> {
        if !st.playing && (st.work.is_some() || st.ready.is_some()) && !st.dialog.is_muted() {
            if let Some(text) = st.progress_q.pop_front() {
                let mut problems = Vec::new();
                let audio = speak_text(&*self.tts, &text, &mut problems).await;
                if !audio.is_empty() {
                    st.dialog.set_output_playing(true);
                    duck(wr, &mut st.music, &mut st.ducked).await?;
                    play_audio(wr, &audio).await?;
                    st.playing = true;
                    st.progress_playing = true;
                }
            }
        }
        Ok(())
    }

    /// Phase: the finished turn is delivered once nothing it said earlier is still playing.
    async fn deliver_ready<W: AsyncWrite + Unpin>(
        &self,
        st: &mut SessionState,
        wr: &mut W,
        pending: &mut Pending,
    ) -> Result<(), String> {
        let (playing, progress_empty) = (st.playing, st.progress_q.is_empty());
        let Some((outcome, effects, asked)) = st.ready.take_if(|_| !playing && progress_empty)
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
            duck(wr, &mut st.music, &mut st.ducked).await?;
            play_audio(wr, &outcome.audio).await?;
            st.playing = true;
        } else if outcome.transcript.is_some() && outcome.reply.is_empty() {
            // A command with nothing to say ("pause", "volume up"): do it now and go quiet. Music that a
            // realtime session paused comes back (nothing is playing now, or this would not run).
            finish_speech(st, wr).await?;
            if !st.dialog.is_muted() {
                send(wr, &Message::SetIndicator(Indicator::Idle)).await?;
            }
            pending.steps.push(st.dialog.on_reply_silent());
        } else {
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
            (outcome, effects, asked.load(Ordering::Relaxed))
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
async fn duck<W: AsyncWrite + Unpin>(
    w: &mut W,
    music: &mut bool,
    ducked: &mut bool,
) -> Result<(), String> {
    if *music {
        send(w, &Message::MediaControl(MediaAction::Pause)).await?;
        *music = false;
        *ducked = true;
    }
    Ok(())
}

/// Send the stream commands that were waiting for speech to finish, tracking whether music is now playing.
async fn flush_media<W: AsyncWrite + Unpin>(
    w: &mut W,
    after: &mut Vec<Message>,
    music: &mut bool,
) -> Result<(), String> {
    for m in after.drain(..) {
        match &m {
            Message::PlayUrl { .. } | Message::MediaControl(MediaAction::Resume) => *music = true,
            Message::MediaControl(MediaAction::Pause) => *music = false,
            _ => {}
        }
        send(w, &m).await?;
    }
    Ok(())
}

async fn poll_work(work: &mut Option<Work>) -> (VoiceOutcome, Vec<Effect>, bool) {
    match work {
        Some(w) => w.await,
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
mod tests {
    use super::*;
    use crate::inbox::endpoint::hash_token;
    use crate::vad::tests::{silence, tone};
    use clawdot_protocol::message::{Capabilities, Hello, Version, Wake};
    use tokio::io::DuplexStream;
    use tokio::time::timeout;

    struct Ears(&'static str);
    impl Transcriber for Ears {
        async fn transcribe(&self, _: &[i16]) -> Result<String, String> {
            Ok(self.0.to_owned())
        }
    }
    struct Mouth;
    impl Synthesizer for Mouth {
        async fn synthesize(&self, text: &str) -> Result<Vec<i16>, String> {
            Ok(vec![7; text.len() * 100])
        }
    }

    /// The satellite end of the pipe.
    struct Peer {
        io: DuplexStream,
        dec: Decoder,
    }

    impl Peer {
        async fn send(&mut self, m: &Message) {
            self.io
                .write_all(&encode(&m.to_frame()).unwrap())
                .await
                .unwrap();
        }
        async fn recv(&mut self) -> Message {
            timeout(
                Duration::from_secs(5),
                next_message(&mut self.io, &mut self.dec),
            )
            .await
            .expect("timed out waiting for the core")
            .unwrap()
            .expect("the core closed the connection")
        }
        /// True if nothing but keepalives arrives for `ms` milliseconds.
        async fn quiet_for(&mut self, ms: u64) -> bool {
            let deadline = Instant::now() + Duration::from_millis(ms);
            while Instant::now() < deadline {
                let left = deadline.saturating_duration_since(Instant::now());
                match timeout(left, next_message(&mut self.io, &mut self.dec)).await {
                    Err(_) => return true,
                    Ok(Ok(Some(Message::Ping))) => self.send(&Message::Pong).await,
                    Ok(_) => return false,
                }
            }
            true
        }
        async fn hello(&mut self, token: Option<&str>, protocol: Version) {
            self.send(&Message::Hello(Hello {
                protocol,
                device_id: "test-dot".into(),
                name: None,
                token: token.map(str::to_owned),
                capabilities: Capabilities {
                    wake_word: true,
                    ..Capabilities::default()
                },
            }))
            .await;
        }
        async fn say(&mut self, ms: usize) {
            let mut audio = silence(200);
            audio.extend(tone(3000.0, ms));
            audio.extend(silence(1000));
            for chunk in audio.chunks(320) {
                let payload: Vec<u8> = chunk.iter().flat_map(|s| s.to_le_bytes()).collect();
                self.send(&Message::AudioChunk {
                    format: AudioFormat::PCM_16K_MONO,
                    timestamp_ms: None,
                    payload,
                })
                .await;
            }
        }
    }

    /// Start a session; the "assistant" answers every question with `answer`.
    fn start(
        settings: Settings,
        heard: &'static str,
        answer: &'static str,
    ) -> (Peer, tokio::task::JoinHandle<Result<(), String>>) {
        let (near, far) = tokio::io::duplex(1 << 20);
        let (tx, mut rx) = mpsc::channel::<Ask>(4);
        tokio::spawn(async move {
            while let Some(ask) = rx.recv().await {
                let _ = ask.reply.send(Answer {
                    say: format!("{answer} ({})", ask.text),
                    effects: vec![Effect::VolumeSet(30)],
                    ..Answer::default()
                });
            }
        });
        let h = tokio::spawn(run_session(
            far,
            Arc::new(Ears(heard)),
            Arc::new(Mouth),
            tx,
            settings,
        ));
        (
            Peer {
                io: near,
                dec: Decoder::new(Limits::default()),
            },
            h,
        )
    }

    fn fast() -> Settings {
        Settings {
            keepalive: Duration::from_millis(50),
            dead_after: Duration::from_millis(400),
            hello_timeout: Duration::from_millis(300),
            ..Settings::default()
        }
    }

    #[tokio::test]
    async fn a_good_hello_is_welcomed() {
        let (mut p, _h) = start(fast(), "x", "y");
        p.hello(None, PROTOCOL_VERSION).await;
        let Message::Welcome(w) = p.recv().await else {
            panic!("expected welcome")
        };
        assert_eq!(w.protocol, PROTOCOL_VERSION);
        assert_eq!(w.audio_out, AudioCodec::Pcm);
        assert_eq!(w.session_id.len(), 16);
    }

    #[tokio::test]
    async fn tokens_versions_and_first_messages_are_checked() {
        let mut s = fast();
        s.token_hashes = vec![hash_token("sat-secret")];
        for (token, version, needle) in [
            (None, PROTOCOL_VERSION, "unauthorized"),
            (Some("wrong"), PROTOCOL_VERSION, "unauthorized"),
            (
                Some("sat-secret"),
                Version { major: 0, minor: 9 },
                "not supported",
            ),
        ] {
            let (mut p, h) = start(s.clone(), "x", "y");
            p.hello(token, version).await;
            let Message::Reject(r) = p.recv().await else {
                panic!("expected reject")
            };
            assert!(r.reason.contains(needle), "{}", r.reason);
            assert!(h.await.unwrap().is_ok(), "a refusal is not a session error");
        }
        let (mut p, _h) = start(s.clone(), "x", "y");
        p.hello(Some("sat-secret"), PROTOCOL_VERSION).await;
        assert!(matches!(p.recv().await, Message::Welcome(_)));
        let (mut p, _h) = start(s, "x", "y");
        p.send(&Message::Ping).await;
        let Message::Reject(r) = p.recv().await else {
            panic!("expected reject")
        };
        assert!(r.reason.contains("hello"));
    }

    #[tokio::test]
    async fn a_silent_connection_is_dropped() {
        let (_p, h) = start(fast(), "x", "y");
        let e = timeout(Duration::from_secs(5), h)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert!(e.contains("hello"), "{e}");
    }

    #[tokio::test]
    async fn a_full_spoken_turn() {
        let (mut p, _h) = start(fast(), "what is up", "Not much.");
        p.hello(None, PROTOCOL_VERSION).await;
        assert!(matches!(p.recv().await, Message::Welcome(_)));

        p.send(&Message::Wake(Wake {
            name: "hey".into(),
            timestamp_ms: None,
        }))
        .await;
        assert_eq!(p.recv().await, Message::SetIndicator(Indicator::Listening));
        assert_eq!(p.recv().await, Message::StreamStart);

        p.say(600).await;
        // Pings may be interleaved; skip them.
        let mut seen = Vec::new();
        loop {
            match p.recv().await {
                Message::Ping => p.send(&Message::Pong).await,
                m => {
                    let last = matches!(m, Message::AudioStop);
                    seen.push(m);
                    if last {
                        break;
                    }
                }
            }
        }
        assert_eq!(seen[0], Message::StreamStop);
        assert_eq!(seen[1], Message::SetIndicator(Indicator::Thinking));
        assert!(seen.contains(&Message::SetIndicator(Indicator::Speaking)));
        assert!(
            seen.contains(&Message::SetVolume { level: 30 }),
            "effects reach the satellite: {seen:?}"
        );
        let audio: usize = seen
            .iter()
            .filter_map(|m| match m {
                Message::AudioChunk { payload, .. } => Some(payload.len() / 2),
                _ => None,
            })
            .sum();
        // Two sentences, "Not much." and "(what is up)", each synthesised separately (100 samples per character).
        assert_eq!(audio, ("Not much.".len() + "(what is up)".len()) * 100);

        // The satellite finished playing: the follow-up window opens.
        p.send(&Message::PlaybackDone { id: None }).await;
        loop {
            match p.recv().await {
                Message::Ping => {}
                m => {
                    assert_eq!(m, Message::SetIndicator(Indicator::Listening));
                    break;
                }
            }
        }
        assert_eq!(p.recv().await, Message::StreamStart);
    }

    #[tokio::test]
    async fn mute_means_the_wake_word_is_ignored_and_the_state_is_shown() {
        let (mut p, _h) = start(fast(), "x", "y");
        p.hello(None, PROTOCOL_VERSION).await;
        p.recv().await;
        p.send(&Message::MuteChanged { muted: true }).await;
        assert_eq!(p.recv().await, Message::StreamStop);
        assert_eq!(p.recv().await, Message::SetIndicator(Indicator::Muted));
        p.send(&Message::Wake(Wake {
            name: "hey".into(),
            timestamp_ms: None,
        }))
        .await;
        assert!(
            p.quiet_for(250).await,
            "the wake word must do nothing while muted"
        );
        p.send(&Message::MuteChanged { muted: false }).await;
        loop {
            match p.recv().await {
                Message::Ping => p.send(&Message::Pong).await,
                m => {
                    assert_eq!(m, Message::SetIndicator(Indicator::Idle));
                    break;
                }
            }
        }
    }

    #[tokio::test]
    async fn the_mute_button_toggles_and_commands_the_satellite() {
        let (mut p, _h) = start(fast(), "x", "y");
        p.hello(None, PROTOCOL_VERSION).await;
        p.recv().await;
        p.send(&Message::Button(clawdot_protocol::message::Button {
            name: "mute".into(),
            long: false,
        }))
        .await;
        let mut got = Vec::new();
        while got.len() < 3 {
            match p.recv().await {
                Message::Ping => p.send(&Message::Pong).await,
                m => got.push(m),
            }
        }
        assert!(got.contains(&Message::SetMute { muted: true }), "{got:?}");
        assert!(
            got.contains(&Message::SetIndicator(Indicator::Muted)),
            "{got:?}"
        );
    }

    #[tokio::test]
    async fn nothing_understood_shows_the_error_state() {
        let (mut p, _h) = start(fast(), "   ", "unused");
        p.hello(None, PROTOCOL_VERSION).await;
        p.recv().await;
        p.send(&Message::Wake(Wake {
            name: "hey".into(),
            timestamp_ms: None,
        }))
        .await;
        p.recv().await;
        p.recv().await;
        p.say(600).await;
        let mut seen = Vec::new();
        while !seen.contains(&Message::SetIndicator(Indicator::Error)) {
            match p.recv().await {
                Message::Ping => p.send(&Message::Pong).await,
                m => seen.push(m),
            }
        }
        assert!(!seen.iter().any(|m| matches!(m, Message::AudioChunk { .. })));
    }

    #[tokio::test]
    async fn wrong_audio_formats_are_reported_once_and_unknown_events_are_ignored() {
        let (mut p, h) = start(fast(), "x", "y");
        p.hello(None, PROTOCOL_VERSION).await;
        p.recv().await;
        let bad = AudioFormat {
            rate: 44_100,
            width: 2,
            channels: 2,
        };
        for _ in 0..3 {
            p.send(&Message::AudioChunk {
                format: bad,
                timestamp_ms: None,
                payload: vec![0; 64],
            })
            .await;
        }
        p.send(&Message::Unknown(clawdot_protocol::frame::Frame::new(
            "future.thing",
        )))
        .await;
        let mut errors = 0;
        let deadline = Instant::now() + Duration::from_millis(300);
        while Instant::now() < deadline {
            let left = deadline.saturating_duration_since(Instant::now());
            match timeout(left, next_message(&mut p.io, &mut p.dec)).await {
                Ok(Ok(Some(Message::Error(e)))) => {
                    assert_eq!(e.code.as_deref(), Some("bad-audio-format"));
                    errors += 1;
                }
                Ok(Ok(Some(Message::Ping))) => p.send(&Message::Pong).await,
                Ok(other) => panic!("unexpected: {other:?}"),
                Err(_) => break,
            }
        }
        assert_eq!(errors, 1, "reported once, not per chunk");
        assert!(
            !h.is_finished(),
            "unknown events and bad audio do not end the session"
        );
    }

    #[tokio::test]
    async fn a_peer_that_goes_quiet_is_dropped_after_the_keepalive_timeout() {
        let (mut p, h) = start(fast(), "x", "y");
        p.hello(None, PROTOCOL_VERSION).await;
        p.recv().await;
        // Never answer pings.
        let r = timeout(Duration::from_secs(5), h).await.unwrap().unwrap();
        assert!(r.unwrap_err().contains("stopped responding"));
    }

    #[tokio::test]
    async fn garbage_on_the_wire_ends_the_session_without_a_panic() {
        let (mut p, h) = start(fast(), "x", "y");
        p.io.write_all(&vec![b'x'; 40_000]).await.unwrap();
        let r = timeout(Duration::from_secs(5), h).await.unwrap().unwrap();
        assert!(r.is_err());
    }

    #[tokio::test]
    async fn the_tcp_server_caps_sessions_and_stops_on_request() {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let (tx, _rx) = mpsc::channel::<Ask>(1);
        let (stop_tx, stop_rx) = oneshot::channel::<()>();
        let settings = Settings {
            max_sessions: 1,
            ..fast()
        };
        let server = tokio::spawn(serve(
            l,
            Arc::new(Ears("x")),
            Arc::new(Mouth),
            tx,
            settings,
            async {
                let _ = stop_rx.await;
            },
        ));
        let mut first = tokio::net::TcpStream::connect(addr).await.unwrap();
        first
            .write_all(&encode(&Message::Ping.to_frame()).unwrap())
            .await
            .unwrap();
        // The first connection holds the only slot, so a second is closed straight away.
        tokio::time::sleep(Duration::from_millis(50)).await;
        let mut second = tokio::net::TcpStream::connect(addr).await.unwrap();
        let mut buf = [0u8; 16];
        let n = timeout(Duration::from_secs(2), second.read(&mut buf))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            n, 0,
            "the second satellite is refused by closing the connection"
        );
        stop_tx.send(()).unwrap();
        assert!(
            timeout(Duration::from_secs(2), server)
                .await
                .unwrap()
                .unwrap()
                .is_ok()
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn one_address_cannot_fill_every_session_slot_with_silent_sockets() {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let (tx, _rx) = mpsc::channel::<Ask>(1);
        let (stop_tx, stop_rx) = oneshot::channel::<()>();
        let settings = Settings {
            max_sessions: 4,
            max_per_ip: 1,
            hello_timeout: Duration::from_millis(400),
            ..fast()
        };
        let server = tokio::spawn(serve(
            l,
            Arc::new(Ears("x")),
            Arc::new(Mouth),
            tx,
            settings,
            async {
                let _ = stop_rx.await;
            },
        ));
        // A silent socket holds this address's only share; the next one is closed at once.
        let mut silent = tokio::net::TcpStream::connect(addr).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        let mut extra = tokio::net::TcpStream::connect(addr).await.unwrap();
        let mut buf = [0u8; 16];
        let n = timeout(Duration::from_millis(300), extra.read(&mut buf))
            .await
            .expect("the extra connection was not refused")
            .unwrap_or(0);
        assert_eq!(n, 0);
        // The silent one is dropped when the hello deadline passes, and the address can connect again.
        let n = timeout(Duration::from_secs(3), silent.read(&mut buf))
            .await
            .expect("the silent connection was not dropped")
            .unwrap_or(0);
        assert_eq!(n, 0);
        tokio::time::sleep(Duration::from_millis(100)).await;
        let mut again = tokio::net::TcpStream::connect(addr).await.unwrap();
        assert!(
            timeout(Duration::from_millis(150), again.read(&mut buf))
                .await
                .is_err(),
            "a fresh connection is accepted once the silent one is gone"
        );
        stop_tx.send(()).unwrap();
        assert!(timeout(Duration::from_secs(2), server).await.is_ok());
    }

    async fn connect_and_register(s: Settings) -> (Peer, Announcer) {
        let ann = s.announcer.clone();
        let (mut p, h) = start(s, "x", "y");
        std::mem::forget(h);
        p.hello(None, PROTOCOL_VERSION).await;
        assert!(matches!(p.recv().await, Message::Welcome(_)));
        for _ in 0..100 {
            if ann.connected() == 1 {
                return (p, ann);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("the session never registered");
    }

    #[tokio::test]
    async fn announcements_are_spoken_when_idle_and_open_the_follow_up_window() {
        let s = fast();
        let ann = s.announcer.clone();
        assert_eq!(ann.connected(), 0);
        assert!(
            !ann.announce("nobody is listening").await,
            "no satellite, no announcement"
        );

        let (mut p, ann) = connect_and_register(s).await;
        assert!(ann.is_available().await);
        assert!(ann.announce("Claude finished. Want to reply?").await);

        let mut seen = Vec::new();
        loop {
            match p.recv().await {
                Message::Ping => p.send(&Message::Pong).await,
                m => {
                    let last = matches!(m, Message::AudioStop);
                    seen.push(m);
                    if last {
                        break;
                    }
                }
            }
        }
        assert_eq!(seen[0], Message::SetIndicator(Indicator::Speaking));
        let samples: usize = seen
            .iter()
            .filter_map(|m| match m {
                Message::AudioChunk { payload, .. } => Some(payload.len() / 2),
                _ => None,
            })
            .sum();
        assert_eq!(
            samples,
            ("Claude finished.".len() + "Want to reply?".len()) * 100
        );
        assert!(
            !ann.is_available().await,
            "busy while the satellite is playing"
        );

        // Playback ends: the user can answer straight away, without the wake word.
        p.send(&Message::PlaybackDone { id: None }).await;
        loop {
            match p.recv().await {
                Message::Ping => p.send(&Message::Pong).await,
                m => {
                    assert_eq!(m, Message::SetIndicator(Indicator::Listening));
                    break;
                }
            }
        }
        assert_eq!(p.recv().await, Message::StreamStart);
        assert!(!ann.is_available().await, "listening for the answer");
    }

    #[tokio::test]
    async fn announcements_never_play_into_a_muted_room() {
        let (mut p, ann) = connect_and_register(fast()).await;
        p.send(&Message::MuteChanged { muted: true }).await;
        // Wait until the satellite has processed the mute.
        for _ in 0..100 {
            if !ann.is_available().await {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(!ann.announce("hello?").await);
        p.send(&Message::MuteChanged { muted: false }).await;
        for _ in 0..100 {
            if ann.is_available().await {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("unmuting should make the satellite available again");
    }

    #[tokio::test]
    async fn a_closed_session_leaves_the_announcer() {
        let (p, ann) = connect_and_register(fast()).await;
        drop(p);
        for _ in 0..100 {
            if ann.connected() == 0 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("the session should deregister when the peer goes away");
    }

    // ----- music on a satellite (REQ-MUS-001, REQ-MUS-004) -----

    /// Like `start`, but the assistant answers with whatever `respond` says, the satellite can play URLs when
    /// `media_url` is true, and the speech-to-text always hears `heard`.
    async fn start_music(
        media_url: bool,
        heard: &'static str,
        respond: fn(&str) -> Answer,
    ) -> Peer {
        let (near, far) = tokio::io::duplex(1 << 20);
        let (tx, mut rx) = mpsc::channel::<Ask>(4);
        tokio::spawn(async move {
            while let Some(ask) = rx.recv().await {
                let _ = ask.reply.send(respond(&ask.text));
            }
        });
        tokio::spawn(run_session(
            far,
            Arc::new(Ears(heard)),
            Arc::new(Mouth),
            tx,
            fast(),
        ));
        let mut p = Peer {
            io: near,
            dec: Decoder::new(Limits::default()),
        };
        p.send(&Message::Hello(Hello {
            protocol: PROTOCOL_VERSION,
            device_id: "radio".into(),
            name: None,
            token: None,
            capabilities: Capabilities {
                media_url,
                ..Capabilities::default()
            },
        }))
        .await;
        assert!(matches!(p.recv().await, Message::Welcome(_)));
        p
    }

    /// Wake the satellite and speak one utterance.
    async fn speak(p: &mut Peer) {
        p.send(&Message::Wake(Wake {
            name: "hey".into(),
            timestamp_ms: None,
        }))
        .await;
        assert_eq!(p.recv().await, Message::SetIndicator(Indicator::Listening));
        assert_eq!(p.recv().await, Message::StreamStart);
        p.say(600).await;
    }

    /// Messages (without keepalives) up to and including the first one `done` accepts.
    async fn until(p: &mut Peer, done: impl Fn(&Message) -> bool) -> Vec<Message> {
        let mut seen = Vec::new();
        loop {
            match p.recv().await {
                Message::Ping => p.send(&Message::Pong).await,
                m => {
                    let stop = done(&m);
                    seen.push(m);
                    if stop {
                        return seen;
                    }
                }
            }
        }
    }

    fn jazz_url() -> Message {
        Message::PlayUrl {
            url: "http://r.example/jazz".into(),
            title: "jazz".into(),
        }
    }

    fn play_jazz(_: &str) -> Answer {
        Answer {
            say: "Playing jazz.".into(),
            effects: vec![Effect::PlayStream {
                url: "http://r.example/jazz".into(),
                title: "jazz".into(),
            }],
            ..Answer::default()
        }
    }

    /// Answers "play jazz" with the stream, anything else with a plain reply.
    fn jazz_or_chat(heard: &str) -> Answer {
        if heard.contains("jazz") {
            play_jazz(heard)
        } else {
            Answer {
                say: "Fine.".into(),
                effects: vec![],
                ..Answer::default()
            }
        }
    }

    #[tokio::test]
    async fn a_radio_stream_starts_only_after_the_spoken_confirmation() {
        let mut p = start_music(true, "play jazz", play_jazz).await;
        speak(&mut p).await;
        let seen = until(&mut p, |m| matches!(m, Message::AudioStop)).await;
        assert!(
            !seen.iter().any(|m| matches!(m, Message::PlayUrl { .. })),
            "music must not cover the reply: {seen:?}"
        );
        p.send(&Message::PlaybackDone { id: None }).await;
        let seen = until(&mut p, |m| matches!(m, Message::PlayUrl { .. })).await;
        assert_eq!(seen.last(), Some(&jazz_url()));
    }

    #[tokio::test]
    async fn music_is_paused_while_the_assistant_talks_and_resumed_afterwards() {
        // Hears "play jazz" first (starts the radio), then something else.
        let (near, far) = tokio::io::duplex(1 << 20);
        let (tx, mut rx) = mpsc::channel::<Ask>(4);
        tokio::spawn(async move {
            while let Some(ask) = rx.recv().await {
                let _ = ask.reply.send(jazz_or_chat(&ask.text));
            }
        });
        struct Sequence(std::sync::atomic::AtomicUsize);
        impl Transcriber for Sequence {
            async fn transcribe(&self, _: &[i16]) -> Result<String, String> {
                let n = self.0.fetch_add(1, Ordering::Relaxed);
                Ok(if n == 0 { "play jazz" } else { "how are you" }.to_owned())
            }
        }
        let ears = Arc::new(Sequence(std::sync::atomic::AtomicUsize::new(0)));
        tokio::spawn(run_session(far, ears, Arc::new(Mouth), tx, fast()));
        let mut p = Peer {
            io: near,
            dec: Decoder::new(Limits::default()),
        };
        p.send(&Message::Hello(Hello {
            protocol: PROTOCOL_VERSION,
            device_id: "radio".into(),
            name: None,
            token: None,
            capabilities: Capabilities {
                media_url: true,
                ..Capabilities::default()
            },
        }))
        .await;
        assert!(matches!(p.recv().await, Message::Welcome(_)));

        speak(&mut p).await;
        until(&mut p, |m| matches!(m, Message::AudioStop)).await;
        p.send(&Message::PlaybackDone { id: None }).await;
        until(&mut p, |m| matches!(m, Message::StreamStart)).await;

        // The radio is playing and the follow-up window is open: ask something else.
        p.say(600).await;
        let seen = until(&mut p, |m| matches!(m, Message::AudioStop)).await;
        let pause = seen
            .iter()
            .position(|m| *m == Message::MediaControl(MediaAction::Pause));
        let start = seen
            .iter()
            .position(|m| matches!(m, Message::AudioStart(_)));
        assert!(
            pause.is_some() && pause < start,
            "the music is paused before speech: {seen:?}"
        );

        p.send(&Message::PlaybackDone { id: None }).await;
        let seen = until(&mut p, |m| {
            matches!(m, Message::MediaControl(MediaAction::Resume))
        })
        .await;
        assert!(
            !seen.is_empty(),
            "the music resumes once the reply has been played"
        );
    }

    #[tokio::test]
    async fn a_silent_command_is_carried_out_without_speech_or_an_error_cue() {
        fn pause_only(_: &str) -> Answer {
            Answer {
                say: String::new(),
                effects: vec![Effect::Media(MediaCmd::Pause)],
                ..Answer::default()
            }
        }
        let mut p = start_music(true, "pause", pause_only).await;
        speak(&mut p).await;
        let seen = until(&mut p, |m| {
            matches!(m, Message::SetIndicator(Indicator::Idle))
        })
        .await;
        assert!(
            seen.contains(&Message::MediaControl(MediaAction::Pause)),
            "{seen:?}"
        );
        assert!(
            !seen.iter().any(|m| matches!(
                m,
                Message::AudioStart(_) | Message::SetIndicator(Indicator::Error)
            )),
            "{seen:?}"
        );
    }

    #[tokio::test]
    async fn a_satellite_that_cannot_play_urls_is_told_so_and_sent_no_stream_commands() {
        let mut p = start_music(false, "play jazz", play_jazz).await;
        speak(&mut p).await;
        let seen = until(&mut p, |m| matches!(m, Message::AudioStop)).await;
        let err = seen.iter().find_map(|m| match m {
            Message::Error(e) => Some(e),
            _ => None,
        });
        assert_eq!(
            err.and_then(|e| e.code.as_deref()),
            Some("media-unsupported"),
            "{seen:?}"
        );
        p.send(&Message::PlaybackDone { id: None }).await;
        // Whatever follows (the follow-up window), it is never a stream command.
        let end = Instant::now() + Duration::from_millis(300);
        while Instant::now() < end {
            let left = end.saturating_duration_since(Instant::now());
            match timeout(left, next_message(&mut p.io, &mut p.dec)).await {
                Err(_) => break,
                Ok(Ok(Some(Message::Ping))) => p.send(&Message::Pong).await,
                Ok(Ok(Some(Message::PlayUrl { .. } | Message::MediaControl(_)))) => {
                    panic!("a stream command was sent to a satellite that cannot play streams")
                }
                Ok(_) => {}
            }
        }
    }

    // ----- a turn that takes a while: speech while working and stopping it (REQ-AST-020, 022, VOI-020) -----

    /// Answers the questions it is asked with the given texts, one per call.
    struct Cycle(Mutex<VecDeque<&'static str>>);
    impl Cycle {
        fn new(texts: &[&'static str]) -> Self {
            Self(Mutex::new(texts.iter().copied().collect()))
        }
    }
    impl Transcriber for Cycle {
        async fn transcribe(&self, _: &[i16]) -> Result<String, String> {
            Ok(self
                .0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .pop_front()
                .unwrap_or("")
                .to_owned())
        }
    }

    /// A session whose assistant runs `handler` for every question.
    async fn start_with<T>(ears: T, handler: impl Fn(Ask) + Send + 'static) -> Peer
    where
        T: Transcriber + Send + Sync + 'static,
    {
        let (near, far) = tokio::io::duplex(1 << 20);
        let (tx, mut rx) = mpsc::channel::<Ask>(4);
        tokio::spawn(async move {
            while let Some(ask) = rx.recv().await {
                handler(ask);
            }
        });
        tokio::spawn(run_session(
            far,
            Arc::new(ears),
            Arc::new(Mouth),
            tx,
            fast(),
        ));
        let mut p = Peer {
            io: near,
            dec: Decoder::new(Limits::default()),
        };
        p.hello(None, PROTOCOL_VERSION).await;
        assert!(matches!(p.recv().await, Message::Welcome(_)));
        p
    }

    fn samples(seen: &[Message]) -> usize {
        seen.iter()
            .filter_map(|m| match m {
                Message::AudioChunk { payload, .. } => Some(payload.len() / 2),
                _ => None,
            })
            .sum()
    }

    fn wake() -> Message {
        Message::Wake(Wake {
            name: "hey".into(),
            timestamp_ms: None,
        })
    }

    /// Say something and wait until the core is thinking about it.
    async fn ask_and_wait_for_thinking(p: &mut Peer) {
        speak(p).await;
        until(p, |m| *m == Message::SetIndicator(Indicator::Thinking)).await;
    }

    const STOPPED: &str = "Okay, I stopped.";

    /// Waits for the user to cancel (up to `wait`), then says what happened.
    fn wait_for_stop(
        wait: Duration,
        cancelled: Arc<std::sync::atomic::AtomicBool>,
    ) -> impl Fn(Ask) + Send {
        move |ask: Ask| {
            let cancelled = Arc::clone(&cancelled);
            tokio::spawn(async move {
                let stopped = timeout(wait, ask.cancel.cancelled()).await.is_ok();
                cancelled.store(stopped, Ordering::SeqCst);
                let say = if stopped { STOPPED } else { "Finished." };
                let _ = ask.reply.send(Answer {
                    say: say.into(),
                    ..Answer::default()
                });
            });
        }
    }

    #[tokio::test]
    async fn progress_speech_plays_while_the_turn_works_and_the_follow_up_waits_for_the_answer() {
        let mut p = start_with(Ears("check the runs"), |ask: Ask| {
            tokio::spawn(async move {
                if let Some(tx) = &ask.progress {
                    let _ = tx.send("Checking the runs.".into());
                }
                tokio::time::sleep(Duration::from_millis(80)).await;
                let _ = ask.reply.send(Answer {
                    say: "All done.".into(),
                    ..Answer::default()
                });
            });
        })
        .await;
        speak(&mut p).await;
        let first = until(&mut p, |m| matches!(m, Message::AudioStop)).await;
        assert_eq!(
            samples(&first),
            "Checking the runs.".len() * 100,
            "{first:?}"
        );
        // The progress phrase ended; the turn is still going, so no follow-up window opens.
        p.send(&Message::PlaybackDone { id: None }).await;
        let second = until(&mut p, |m| matches!(m, Message::AudioStop)).await;
        assert!(
            !second.contains(&Message::SetIndicator(Indicator::Listening)),
            "listening opened between the phrase and the answer: {second:?}"
        );
        assert_eq!(samples(&second), "All done.".len() * 100, "{second:?}");
        // After the answer the follow-up window opens as before.
        p.send(&Message::PlaybackDone { id: None }).await;
        let after = until(&mut p, |m| *m == Message::StreamStart).await;
        assert!(after.contains(&Message::SetIndicator(Indicator::Listening)));
    }

    #[tokio::test]
    async fn the_action_button_stops_a_running_turn() {
        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut p = start_with(
            Ears("check the runs"),
            wait_for_stop(Duration::from_secs(3), Arc::clone(&cancelled)),
        )
        .await;
        ask_and_wait_for_thinking(&mut p).await;
        p.send(&Message::Button(clawdot_protocol::message::Button {
            name: "action".into(),
            long: false,
        }))
        .await;
        let seen = until(&mut p, |m| matches!(m, Message::AudioStop)).await;
        assert!(cancelled.load(Ordering::SeqCst));
        assert_eq!(samples(&seen), STOPPED.len() * 100, "{seen:?}");
        assert!(
            !seen.contains(&Message::StreamStart),
            "the button stops the turn; it does not start another"
        );
    }

    #[tokio::test]
    async fn a_wake_word_and_stop_while_thinking_cancels_the_turn() {
        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut p = start_with(
            Cycle::new(&["check the runs", "stop"]),
            wait_for_stop(Duration::from_secs(3), Arc::clone(&cancelled)),
        )
        .await;
        ask_and_wait_for_thinking(&mut p).await;
        // The wake word opens a short capture while the turn goes on.
        p.send(&wake()).await;
        let opened = until(&mut p, |m| *m == Message::StreamStart).await;
        assert!(
            opened.contains(&Message::SetIndicator(Indicator::Listening)),
            "{opened:?}"
        );
        p.say(500).await;
        let seen = until(&mut p, |m| matches!(m, Message::AudioStop)).await;
        assert!(
            seen.contains(&Message::StreamStop),
            "the capture was closed: {seen:?}"
        );
        assert!(cancelled.load(Ordering::SeqCst), "the turn was cancelled");
        assert_eq!(samples(&seen), STOPPED.len() * 100, "{seen:?}");
    }

    #[tokio::test]
    async fn anything_else_said_after_the_wake_word_while_thinking_is_dropped() {
        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut p = start_with(
            Cycle::new(&["check the runs", "what is the weather"]),
            wait_for_stop(Duration::from_millis(600), Arc::clone(&cancelled)),
        )
        .await;
        ask_and_wait_for_thinking(&mut p).await;
        p.send(&wake()).await;
        until(&mut p, |m| *m == Message::StreamStart).await;
        p.say(500).await;
        let seen = until(&mut p, |m| matches!(m, Message::AudioStop)).await;
        assert!(
            !cancelled.load(Ordering::SeqCst),
            "a different request is not a stop"
        );
        assert_eq!(samples(&seen), "Finished.".len() * 100, "{seen:?}");
    }

    // ----- the administrator reset request (REQ-SAT-020, REQ-DEV-031) -----

    async fn next_reset_answer(p: &mut Peer) -> (u32, Option<String>) {
        loop {
            match p.recv().await {
                Message::AdminResetWindow { open_secs, reason } => return (open_secs, reason),
                Message::Ping => p.send(&Message::Pong).await,
                _ => {}
            }
        }
    }

    fn counting_hook(calls: &Arc<AtomicU64>, answer: Result<u32, String>) -> AdminResetHook {
        let calls = Arc::clone(calls);
        AdminResetHook::new(move || {
            calls.fetch_add(1, Ordering::SeqCst);
            answer.clone()
        })
    }

    async fn welcomed(s: Settings, token: Option<&str>) -> Peer {
        let (mut p, h) = start(s, "x", "y");
        std::mem::forget(h);
        p.hello(token, PROTOCOL_VERSION).await;
        assert!(matches!(p.recv().await, Message::Welcome(_)));
        p
    }

    #[tokio::test]
    async fn a_loopback_satellite_can_open_the_reset_window() {
        let calls = Arc::new(AtomicU64::new(0));
        let mut s = fast();
        s.peer_loopback = true;
        s.admin_reset = Some(counting_hook(&calls, Ok(300)));
        let mut p = welcomed(s, None).await;
        p.send(&Message::AdminReset).await;
        assert_eq!(next_reset_answer(&mut p).await, (300, None));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_network_satellite_without_a_token_is_refused_and_the_core_is_not_asked() {
        let calls = Arc::new(AtomicU64::new(0));
        let mut s = fast();
        s.admin_reset = Some(counting_hook(&calls, Ok(300)));
        let mut p = welcomed(s, None).await;
        p.send(&Message::AdminReset).await;
        let (secs, reason) = next_reset_answer(&mut p).await;
        assert_eq!(secs, 0);
        assert!(reason.unwrap().contains("not trusted"));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "a guest on the LAN gets nothing"
        );
    }

    #[tokio::test]
    async fn a_network_satellite_that_proved_a_token_is_trusted() {
        let calls = Arc::new(AtomicU64::new(0));
        let mut s = fast();
        s.token_hashes = vec![hash_token("sat-secret")];
        s.admin_reset = Some(counting_hook(&calls, Ok(300)));
        let mut p = welcomed(s, Some("sat-secret")).await;
        p.send(&Message::AdminReset).await;
        assert_eq!(next_reset_answer(&mut p).await.0, 300);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_refusal_from_the_core_and_a_missing_hook_are_both_explained() {
        let calls = Arc::new(AtomicU64::new(0));
        let mut s = fast();
        s.peer_loopback = true;
        s.admin_reset = Some(counting_hook(
            &calls,
            Err("Wait a minute and try again.".into()),
        ));
        let mut p = welcomed(s, None).await;
        p.send(&Message::AdminReset).await;
        assert_eq!(
            next_reset_answer(&mut p).await,
            (0, Some("Wait a minute and try again.".into()))
        );
        let mut s = fast();
        s.peer_loopback = true;
        let mut p = welcomed(s, None).await;
        p.send(&Message::AdminReset).await;
        assert_eq!(next_reset_answer(&mut p).await.0, 0);
    }

    #[tokio::test]
    async fn a_satellite_cannot_hammer_the_reset_request() {
        let calls = Arc::new(AtomicU64::new(0));
        let mut s = fast();
        s.peer_loopback = true;
        s.admin_reset = Some(counting_hook(&calls, Ok(300)));
        let mut p = welcomed(s, None).await;
        for _ in 0..5 {
            p.send(&Message::AdminReset).await;
        }
        let mut answers = Vec::new();
        for _ in 0..5 {
            answers.push(next_reset_answer(&mut p).await.0);
        }
        assert_eq!(answers, [300, 0, 0, 0, 0]);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}

/// The session phases, each exercised alone against an in-memory writer (REQ-SAT-002, REQ-MUS-004, REQ-SAT-020).
#[cfg(test)]
mod phase_tests {
    use super::*;
    use crate::inbox::endpoint::hash_token;
    use clawdot_protocol::message::{Capabilities, Hello, Version};

    /// Everything written to `bytes`, decoded.
    fn decode_all(bytes: &[u8]) -> Vec<Message> {
        let mut dec = Decoder::new(Limits::default());
        dec.push(bytes);
        let mut out = Vec::new();
        while let Some(f) = dec.next_frame().unwrap() {
            out.push(Message::from_frame(f).unwrap());
        }
        out
    }

    fn hello_bytes(token: Option<&str>, protocol: Version, media_url: bool) -> Vec<u8> {
        encode(
            &Message::Hello(Hello {
                protocol,
                device_id: "d".into(),
                name: None,
                token: token.map(str::to_owned),
                capabilities: Capabilities {
                    media_url,
                    ..Capabilities::default()
                },
            })
            .to_frame(),
        )
        .unwrap()
    }

    async fn shake(
        input: &[u8],
        settings: &Settings,
    ) -> (Result<Option<Accepted>, String>, Vec<Message>) {
        let mut rd = input;
        let mut out = Vec::new();
        let mut dec = Decoder::new(Limits::default());
        let r = handshake(&mut rd, &mut out, &mut dec, settings).await;
        (r, decode_all(&out))
    }

    #[tokio::test]
    async fn the_handshake_welcomes_a_good_hello_and_records_its_capabilities() {
        let (r, sent) = shake(
            &hello_bytes(None, PROTOCOL_VERSION, true),
            &Settings::default(),
        )
        .await;
        let accepted = r.unwrap().expect("accepted");
        assert!(accepted.media_url);
        assert!(accepted.speaker_id.starts_with("sat:"));
        assert!(matches!(sent.as_slice(), [Message::Welcome(_)]));
    }

    #[tokio::test]
    async fn the_handshake_refuses_without_a_welcome() {
        let s = Settings {
            token_hashes: vec![hash_token("secret")],
            ..Settings::default()
        };
        let cases = [
            (
                hello_bytes(Some("nope"), PROTOCOL_VERSION, false),
                "unauthorized",
            ),
            (hello_bytes(None, PROTOCOL_VERSION, false), "unauthorized"),
            (
                hello_bytes(Some("secret"), Version { major: 0, minor: 9 }, false),
                "not supported",
            ),
            (encode(&Message::Ping.to_frame()).unwrap(), "must be hello"),
        ];
        for (input, needle) in cases {
            let (r, sent) = shake(&input, &s).await;
            assert!(r.unwrap().is_none(), "{needle}");
            let [Message::Reject(rej)] = sent.as_slice() else {
                panic!("expected one reject, got {sent:?}")
            };
            assert!(rej.reason.contains(needle), "{}", rej.reason);
        }
        // A peer that hangs up before saying anything is simply done.
        let (r, sent) = shake(&[], &s).await;
        assert!(r.unwrap().is_none() && sent.is_empty());
    }

    #[tokio::test]
    async fn effects_set_the_volume_within_bounds_and_hold_stream_commands_until_speech_ends() {
        let mut st = SessionState::new(&Settings::default());
        st.music = true;
        let mut out = Vec::new();
        let play = Effect::PlayStream {
            url: "http://x/y".into(),
            title: "Y".into(),
        };
        let effects = [
            Effect::VolumeUp,
            Effect::VolumeSet(250),
            Effect::VolumeDown,
            play.clone(),
            Effect::Media(MediaCmd::Pause),
        ];
        apply_effects(&mut st, &mut out, true, &effects)
            .await
            .unwrap();
        let levels: Vec<u8> = decode_all(&out)
            .into_iter()
            .map(|m| match m {
                Message::SetVolume { level } => level,
                other => panic!("unexpected {other:?}"),
            })
            .collect();
        assert_eq!(levels, [60, 100, 90]);
        assert_eq!(st.after.len(), 2, "stream commands wait");
        assert!(st.music, "nothing was started or stopped yet");

        // Without the media_url capability the stream is refused once, in words, and nothing is queued.
        let mut st = SessionState::new(&Settings::default());
        let mut out = Vec::new();
        apply_effects(&mut st, &mut out, false, &[play])
            .await
            .unwrap();
        assert!(st.after.is_empty());
        let sent = decode_all(&out);
        assert!(matches!(
            sent.as_slice(),
            [Message::Error(e)] if e.code.as_deref() == Some("media-unsupported")
        ));

        // A stop clears what was waiting and marks the music as stopped.
        st.after.push(Message::MediaControl(MediaAction::Resume));
        st.music = true;
        let mut out = Vec::new();
        apply_effects(&mut st, &mut out, true, &[Effect::StopPlayback])
            .await
            .unwrap();
        assert!(st.after.is_empty() && !st.music);
        assert_eq!(decode_all(&out), [Message::StopPlayback]);
    }

    #[tokio::test]
    async fn playback_done_resumes_ducked_music_and_progress_speech_does_not_move_the_dialog_on() {
        let mut st = SessionState::new(&Settings::default());
        let mut pending = Pending::default();
        let mut out = Vec::new();

        // Nothing playing: the message is ignored.
        on_playback_done(&mut st, &mut out, &mut pending)
            .await
            .unwrap();
        assert!(out.is_empty() && pending.steps.is_empty());

        // Progress speech ended while the turn runs: no step for the dialog, music stays ducked.
        st.playing = true;
        st.progress_playing = true;
        st.ducked = true;
        on_playback_done(&mut st, &mut out, &mut pending)
            .await
            .unwrap();
        assert!(!st.playing && !st.progress_playing && st.ducked);
        assert!(out.is_empty() && pending.steps.is_empty());

        // A reply ended: ducked music resumes and the dialog is told.
        st.playing = true;
        on_playback_done(&mut st, &mut out, &mut pending)
            .await
            .unwrap();
        assert!(!st.playing && !st.ducked && st.music);
        assert_eq!(
            decode_all(&out),
            [Message::MediaControl(MediaAction::Resume)]
        );
        assert_eq!(pending.steps.len(), 1);
    }

    #[test]
    fn a_reset_request_is_rate_limited_trusted_and_needs_a_hook() {
        let mut s = Settings::default();
        assert_eq!(
            reset_answer(&s, true, false).1.as_deref(),
            Some("Not available.")
        );
        s.admin_reset = Some(AdminResetHook::new(|| Ok(30)));
        assert_eq!(reset_answer(&s, true, false), (30, None));
        assert!(
            reset_answer(&s, false, false)
                .1
                .is_some_and(|r| r.contains("not trusted"))
        );
        assert!(
            reset_answer(&s, true, true)
                .1
                .is_some_and(|r| r.contains("Too many"))
        );
        s.admin_reset = Some(AdminResetHook::new(|| Err("busy".into())));
        assert_eq!(reset_answer(&s, true, false), (0, Some("busy".into())));
    }
}
