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
use crate::inbox::endpoint::verify_token;
use crate::speech::decode_pcm16;
use crate::voice::{
    Config as DialogConfig, Cue, Dialog, Phase, Step, Synthesizer, Transcriber, VoiceOutcome,
    speak_text, voice_turn,
};
use clawdot_protocol::frame::{Decoder, Limits, encode};
use clawdot_protocol::message::{
    AudioCodec, AudioFormat, Indicator, MediaAction, Message, PROTOCOL_VERSION, Reject, Welcome,
};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{Semaphore, mpsc, oneshot};

/// A question for the assistant, with a place to send the answer.
#[derive(Debug)]
pub struct Ask {
    /// What the user said.
    pub text: String,
    /// Where the answer goes.
    pub reply: oneshot::Sender<Answer>,
}

/// The assistant's answer.
#[derive(Debug, Clone, Default)]
pub struct Answer {
    /// What to say.
    pub say: String,
    /// Side effects for the satellite.
    pub effects: Vec<Effect>,
}

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

type Work = Pin<Box<dyn Future<Output = (VoiceOutcome, Vec<Effect>)> + Send>>;

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
async fn next_message<R: AsyncRead + Unpin>(
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

    // ---- handshake (REQ-SAT-002) ----
    let first = tokio::time::timeout(settings.hello_timeout, next_message(&mut rd, &mut dec))
        .await
        .map_err(|_| "no hello within the allowed time".to_owned())??;
    let hello = match first {
        Some(Message::Hello(h)) => h,
        Some(_) => {
            send(
                &mut wr,
                &Message::Reject(Reject {
                    reason: "the first message must be hello".into(),
                }),
            )
            .await?;
            return Ok(());
        }
        None => return Ok(()),
    };
    if !PROTOCOL_VERSION.compatible_with(hello.protocol) {
        let reason = format!(
            "protocol {}.{} is not supported; this core speaks {}.{}",
            hello.protocol.major,
            hello.protocol.minor,
            PROTOCOL_VERSION.major,
            PROTOCOL_VERSION.minor
        );
        send(&mut wr, &Message::Reject(Reject { reason })).await?;
        return Ok(());
    }
    if !settings.token_hashes.is_empty() {
        let ok = hello
            .token
            .as_deref()
            .is_some_and(|t| settings.token_hashes.iter().any(|h| verify_token(t, h)));
        if !ok {
            send(
                &mut wr,
                &Message::Reject(Reject {
                    reason: "unauthorized".into(),
                }),
            )
            .await?;
            return Ok(());
        }
    }
    send(
        &mut wr,
        &Message::Welcome(Welcome {
            protocol: PROTOCOL_VERSION,
            session_id: new_session_id(),
            audio_out: AudioCodec::Pcm,
        }),
    )
    .await?;

    // ---- conversation ----
    let (_registration, mut announce_rx) = settings.announcer.register();
    let started = Instant::now();
    let now_ms = || u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let mut dialog = Dialog::new(settings.dialog);
    let mut last_heard = Instant::now();
    let mut volume: u8 = 50;
    let mut playing = false;
    let media_url = hello.capabilities.media_url;
    let mut music = false;
    let mut ducked = false;
    let mut after: Vec<Message> = Vec::new();
    let mut work: Option<Work> = None;
    let mut warned_format = false;
    let mut tick = tokio::time::interval(
        settings
            .keepalive
            .min(Duration::from_secs(1))
            .max(Duration::from_millis(10)),
    );
    let mut last_ping = Instant::now();

    loop {
        // What the previous step asked us to do.
        let mut steps: Vec<Step> = Vec::new();
        tokio::select! {
            msg = next_message(&mut rd, &mut dec) => {
                last_heard = Instant::now();
                let Some(msg) = msg? else { return Ok(()) };
                match msg {
                    Message::Ping => send(&mut wr, &Message::Pong).await?,
                    Message::Wake(_) => steps.push(dialog.on_wake()),
                    Message::Button(b) if b.name == "action" => steps.push(dialog.on_wake()),
                    Message::Button(b) if b.name == "mute" => {
                        let muted = !dialog.is_muted();
                        steps.push(dialog.set_muted(muted));
                        send(&mut wr, &Message::SetMute { muted }).await?;
                    }
                    Message::MuteChanged { muted } => steps.push(dialog.set_muted(muted)),
                    Message::AudioChunk { format, payload, .. } => {
                        if format != AudioFormat::PCM_16K_MONO {
                            if !warned_format {
                                warned_format = true;
                                send(&mut wr, &Message::Error(clawdot_protocol::message::ErrorReport {
                                    text: "uplink audio must be 16 kHz 16-bit mono".into(),
                                    code: Some("bad-audio-format".into()),
                                })).await?;
                            }
                        } else {
                            steps.push(dialog.on_audio(now_ms(), &decode_pcm16(&payload)));
                        }
                    }
                    Message::PlaybackDone { .. } if playing => {
                        playing = false;
                        if ducked {
                            ducked = false;
                            if after.is_empty() {
                                send(&mut wr, &Message::MediaControl(MediaAction::Resume)).await?;
                                music = true;
                            }
                        }
                        flush_media(&mut wr, &mut after, &mut music).await?;
                        steps.push(dialog.on_playback_done());
                    }
                    // Pong, button names we do not use, errors, unknown events, and anything that only
                    // the core should send are all ignored, so newer satellites keep working (REQ-SAT-009).
                    _ => {}
                }
            }
            (outcome, effects) = poll_work(&mut work), if work.is_some() => {
                work = None;
                // Volume and stop take effect now; stream commands wait until the satellite has finished
                // speaking, so music never covers the reply.
                for e in &effects {
                    match e {
                        Effect::StopPlayback => {
                            send(&mut wr, &Message::StopPlayback).await?;
                            music = false;
                            after.clear();
                        }
                        Effect::VolumeSet(l) => {
                            volume = (*l).min(100);
                            send(&mut wr, &Message::SetVolume { level: volume }).await?;
                        }
                        Effect::VolumeUp => {
                            volume = volume.saturating_add(VOLUME_STEP).min(100);
                            send(&mut wr, &Message::SetVolume { level: volume }).await?;
                        }
                        Effect::VolumeDown => {
                            volume = volume.saturating_sub(VOLUME_STEP);
                            send(&mut wr, &Message::SetVolume { level: volume }).await?;
                        }
                        Effect::PlayStream { url, title } if media_url => {
                            after.push(Message::PlayUrl { url: url.clone(), title: title.clone() });
                        }
                        Effect::PlayStream { .. } => {
                            send(&mut wr, &Message::Error(clawdot_protocol::message::ErrorReport {
                                text: "this satellite did not declare media_url, so it cannot play streams".into(),
                                code: Some("media-unsupported".into()),
                            })).await?;
                        }
                        Effect::Media(MediaCmd::Pause) if media_url => after.push(Message::MediaControl(MediaAction::Pause)),
                        Effect::Media(MediaCmd::Resume) if media_url => after.push(Message::MediaControl(MediaAction::Resume)),
                        Effect::Media(_) => {}
                    }
                }
                if !outcome.audio.is_empty() {
                    dialog.on_reply_started();
                    duck(&mut wr, &mut music, &mut ducked).await?;
                    play_audio(&mut wr, &outcome.audio).await?;
                    playing = true;
                } else if outcome.transcript.is_some() && outcome.reply.is_empty() {
                    // A command with nothing to say ("pause", "volume up"): do it now and go quiet.
                    flush_media(&mut wr, &mut after, &mut music).await?;
                    send(&mut wr, &Message::SetIndicator(Indicator::Idle)).await?;
                    steps.push(dialog.on_reply_silent());
                } else {
                    steps.push(dialog.on_reply_failed());
                }
            }
            Some(a) = announce_rx.recv() => {
                let idle = dialog.phase() == Phase::Idle && !dialog.is_muted() && work.is_none() && !playing;
                let accepted = match a.request {
                    Request::Probe => idle,
                    Request::Speak(text) if idle => {
                        let mut problems = Vec::new();
                        let audio = speak_text(&*tts, &text, &mut problems).await;
                        if audio.is_empty() {
                            false
                        } else {
                            dialog.on_reply_started();
                            duck(&mut wr, &mut music, &mut ducked).await?;
                            play_audio(&mut wr, &audio).await?;
                            playing = true;
                            true
                        }
                    }
                    Request::Speak(_) => false,
                };
                let _ = a.answer.send(accepted);
            }
            _ = tick.tick() => {
                if last_heard.elapsed() >= settings.dead_after {
                    return Err("the satellite stopped responding".into());
                }
                if last_ping.elapsed() >= settings.keepalive {
                    last_ping = Instant::now();
                    send(&mut wr, &Message::Ping).await?;
                }
                steps.push(dialog.tick(now_ms()));
            }
        }

        for step in steps {
            match step {
                Step::Nothing => {}
                Step::Cue(cue) => cue_to_wire(cue, &mut wr).await?,
                Step::Utterance(pcm) => {
                    send(&mut wr, &Message::StreamStop).await?;
                    send(&mut wr, &Message::SetIndicator(Indicator::Thinking)).await?;
                    let (stt, tts, asks) = (Arc::clone(&stt), Arc::clone(&tts), asks.clone());
                    work = Some(Box::pin(async move {
                        let effects = Arc::new(Mutex::new(Vec::new()));
                        let sink = Arc::clone(&effects);
                        let outcome = voice_turn(&*stt, &*tts, &pcm, move |text| async move {
                            let (tx, rx) = oneshot::channel();
                            if asks.send(Ask { text, reply: tx }).await.is_err() {
                                return "Sorry, I can't answer right now.".to_owned();
                            }
                            match rx.await {
                                Ok(a) => {
                                    sink.lock()
                                        .unwrap_or_else(PoisonError::into_inner)
                                        .extend(a.effects);
                                    a.say
                                }
                                Err(_) => "Sorry, I can't answer right now.".to_owned(),
                            }
                        })
                        .await;
                        let effects = std::mem::take(
                            &mut *effects.lock().unwrap_or_else(PoisonError::into_inner),
                        );
                        (outcome, effects)
                    }));
                }
            }
        }
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

async fn poll_work(work: &mut Option<Work>) -> (VoiceOutcome, Vec<Effect>) {
    match work {
        Some(w) => w.await,
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
        let (stt, tts, asks, settings) = (
            Arc::clone(&stt),
            Arc::clone(&tts),
            asks.clone(),
            settings.clone(),
        );
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
}
