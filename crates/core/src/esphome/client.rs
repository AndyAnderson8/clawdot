//! The ESPHome client: dials a device, sets up the native API, and plays the part of a satellite toward the
//! voice pipeline (spec 0004, REQ-ESP-005..013).
//!
//! The pipeline already serves satellites over the satellite protocol, so the client does not reimplement any
//! of it. For each connection it runs a normal [`satellite::run_session`] on one end of an in-memory pipe and
//! translates between that protocol and the ESPHome messages on the other end. Wake handling, endpointing,
//! speech-to-text, the assistant, text-to-speech, the follow-up window, and inbox announcements are therefore
//! exactly the ones every other satellite gets.

use super::frame::Link;
use super::media::{LiveWriter, MAX_BYTES, MediaStore, wav_from_pcm};
use super::proto::{Msg, event, flags, media as player};
use crate::esphome_cfg::ReplyMode;
use crate::satellite::{self, Ask, Settings};
use crate::voice::{Synthesizer, Transcriber};
use clawdot_protocol::frame::{Decoder, Limits, encode};
use clawdot_protocol::message::{
    AudioFormat, Capabilities, Hello, Indicator, MediaAction, Message, PROTOCOL_VERSION, Wake,
};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, DuplexStream, WriteHalf};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// Bytes of audio per `VoiceAssistantAudio` message toward the device (32 ms).
const AUDIO_CHUNK: usize = 1024;
/// Bytes per second of 16 kHz 16-bit mono audio.
const BYTES_PER_SEC: u64 = 32_000;
/// Messages sent back-to-back before audio is paced.
const AUDIO_BURST: u32 = 4;
/// The API version this client announces, and the major version it accepts.
const API_MAJOR: u32 = 1;
const API_MINOR: u32 = 10;
/// Media-player entities read during setup before the list is considered finished.
const MAX_MEDIA_PLAYERS: usize = 16;
/// A reply that is being relayed while it is made is ended when no audio has come for this long.
const STREAM_IDLE: Duration = Duration::from_secs(60);

/// Timeouts and intervals, in one place so tests can shorten them.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    /// Longest wait for the TCP connection.
    pub connect: Duration,
    /// Longest wait for each answer during setup.
    pub setup_step: Duration,
    /// How often we ping the device.
    pub ping_every: Duration,
    /// A device silent this long is gone.
    pub dead_after: Duration,
    /// How long the pipeline has to start listening after a device request.
    pub wake_watch: Duration,
    /// A run with no microphone audio for this long is abandoned.
    pub mic_idle: Duration,
    /// Added to the audio's duration before a streamed reply counts as played.
    pub api_margin: Duration,
    /// Added to the audio's duration before a fetched reply counts as played.
    pub url_margin: Duration,
    /// Added to the audio's duration while waiting for `VoiceAssistantAnnounceFinished`.
    pub announce_margin: Duration,
    /// Gap between paced audio messages.
    pub chunk_every: Duration,
    /// First reconnect delay.
    pub backoff_min: Duration,
    /// Longest reconnect delay.
    pub backoff_max: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(10),
            setup_step: Duration::from_secs(10),
            ping_every: Duration::from_secs(10),
            dead_after: Duration::from_secs(30),
            wake_watch: Duration::from_secs(2),
            mic_idle: Duration::from_secs(8),
            api_margin: Duration::from_millis(300),
            url_margin: Duration::from_millis(1500),
            announce_margin: Duration::from_secs(10),
            chunk_every: Duration::from_millis(20),
            backoff_min: Duration::from_secs(1),
            backoff_max: Duration::from_secs(30),
        }
    }
}

/// A device to dial.
#[derive(Clone)]
pub struct Device {
    /// Short name, the satellite's device id.
    pub name: String,
    /// `host:port`.
    pub addr: String,
    /// The Noise key, when the device is encrypted.
    pub psk: Option<[u8; 32]>,
}

#[cfg(feature = "esphome-noise")]
impl Device {
    /// Overwrite the Noise key with zeros.
    fn wipe_key(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.psk);
    }
}

/// Every clone of a device carries a copy of the key, so each one wipes its own when it is dropped.
#[cfg(feature = "esphome-noise")]
impl Drop for Device {
    fn drop(&mut self) {
        self.wipe_key();
    }
}

impl std::fmt::Debug for Device {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The key is a secret and never printed.
        f.debug_struct("Device")
            .field("name", &self.name)
            .field("addr", &self.addr)
            .field("encrypted", &self.psk.is_some())
            .finish()
    }
}

/// Keep device-supplied text safe for a log line.
fn clean(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .take(64)
        .collect()
}

/// What setup learned about the device.
#[derive(Debug, Clone)]
struct Info {
    friendly: String,
    flags: u32,
    media_key: Option<u32>,
}

/// Dial a device and keep it connected until `shutdown` completes, reconnecting with backoff
/// (REQ-ESP-012).
pub async fn run_device<S, T>(
    dev: Device,
    media: Option<MediaStore>,
    pipeline: Pipeline<S, T>,
    timing: Timing,
    shutdown: impl std::future::Future<Output = ()> + Send,
) where
    S: Transcriber + Send + Sync + 'static,
    T: Synthesizer + Send + Sync + 'static,
{
    run_device_with(dev, media, pipeline, timing, ReplyMode::Whole, shutdown).await;
}

/// Like [`run_device`], with the way spoken replies reach the device chosen (`esphome.devices[].reply`).
pub async fn run_device_with<S, T>(
    dev: Device,
    media: Option<MediaStore>,
    pipeline: Pipeline<S, T>,
    timing: Timing,
    reply: ReplyMode,
    shutdown: impl std::future::Future<Output = ()> + Send,
) where
    S: Transcriber + Send + Sync + 'static,
    T: Synthesizer + Send + Sync + 'static,
{
    tokio::pin!(shutdown);
    let mut backoff = timing.backoff_min;
    let mut last = String::new();
    loop {
        let started = Instant::now();
        let attempt = connect_once(&dev, &media, &pipeline, &timing, reply);
        let result = tokio::select! {
            r = attempt => r,
            () = &mut shutdown => return,
        };
        match result {
            Err(e) if e != last => {
                eprintln!("clawdot: esphome {}: {e}", clean(&dev.name));
                last = e;
            }
            Err(_) => {}
            Ok(()) => last.clear(),
        }
        // A connection that held for a while starts the backoff over.
        if started.elapsed() >= Duration::from_secs(10) {
            backoff = timing.backoff_min;
        }
        let wait = backoff;
        backoff = (backoff * 2).min(timing.backoff_max);
        tokio::select! {
            () = tokio::time::sleep(wait) => {}
            () = &mut shutdown => return,
        }
    }
}

/// Everything a pipeline session needs, shared by every connection of a device.
pub struct Pipeline<S, T> {
    /// Speech to text.
    pub stt: Arc<S>,
    /// Text to speech.
    pub tts: Arc<T>,
    /// Where questions for the assistant go.
    pub asks: mpsc::Sender<Ask>,
    /// Session settings (dialog, announcer).
    pub settings: Settings,
}

impl<S, T> std::fmt::Debug for Pipeline<S, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Pipeline")
    }
}

impl<S, T> Clone for Pipeline<S, T> {
    fn clone(&self) -> Self {
        Self {
            stt: Arc::clone(&self.stt),
            tts: Arc::clone(&self.tts),
            asks: self.asks.clone(),
            settings: self.settings.clone(),
        }
    }
}

async fn connect_once<S, T>(
    dev: &Device,
    media: &Option<MediaStore>,
    pipeline: &Pipeline<S, T>,
    t: &Timing,
    reply_mode: ReplyMode,
) -> Result<(), String>
where
    S: Transcriber + Send + Sync + 'static,
    T: Synthesizer + Send + Sync + 'static,
{
    let stream = tokio::time::timeout(t.connect, TcpStream::connect(&dev.addr))
        .await
        .map_err(|_| "connect timed out".to_owned())?
        .map_err(|e| format!("connect: {e}"))?;
    let _ = stream.set_nodelay(true);
    let mut link = open_link(stream, dev, t).await?;
    let info = setup(&mut link, t).await?;
    link.send(&Msg::SubscribeVoice {
        subscribe: true,
        flags: 1,
    })
    .await?;

    // The pipeline end of an in-memory pipe runs a normal satellite session.
    let (sat_io, core_io) = tokio::io::duplex(128 * 1024);
    let (mut sat_rd, sat_wr) = tokio::io::split(sat_io);
    // A voice run cannot start while the assistant is working (REQ-ESP-007), so a wake word is never turned
    // into a stop capture on this transport (REQ-VOI-020 applies to the native satellite protocol).
    let mut settings = pipeline.settings.clone();
    settings.dialog.interrupt_while_working = false;
    let session = Guard(tokio::spawn(satellite::run_session(
        core_io,
        Arc::clone(&pipeline.stt),
        Arc::clone(&pipeline.tts),
        pipeline.asks.clone(),
        settings,
    )));
    let mut bridge = Bridge::new(
        link,
        sat_wr,
        *t,
        pipeline.settings.dialog.follow_up_secs > 0,
        media.clone(),
        reply_mode,
        info,
    );
    let caps = Capabilities {
        wake_word: true,
        media_url: bridge.info.media_key.is_some(),
        ..Capabilities::default()
    };
    bridge
        .sat_send(&Message::Hello(Hello {
            protocol: PROTOCOL_VERSION,
            device_id: dev.name.clone(),
            name: Some(clean(&bridge.info.friendly)),
            token: None,
            capabilities: caps,
        }))
        .await?;
    bridge.run_loop(&mut sat_rd, session).await
}

/// Ends the pipeline session task when the connection ends.
struct Guard(JoinHandle<Result<(), String>>);

impl Drop for Guard {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[cfg(feature = "esphome-noise")]
async fn open_link(stream: TcpStream, dev: &Device, t: &Timing) -> Result<Link<TcpStream>, String> {
    match &dev.psk {
        Some(psk) => tokio::time::timeout(t.setup_step, super::noise::connect(stream, psk))
            .await
            .map_err(|_| "Noise handshake timed out".to_owned())?,
        None => Ok(Link::plain(stream)),
    }
}

#[cfg(not(feature = "esphome-noise"))]
async fn open_link(
    stream: TcpStream,
    dev: &Device,
    _t: &Timing,
) -> Result<Link<TcpStream>, String> {
    if dev.psk.is_some() {
        return Err("this build has no Noise support (feature `esphome-noise`)".into());
    }
    Ok(Link::plain(stream))
}

/// Wait for a message that satisfies `want`, answering pings meanwhile.
async fn expect<IO, F>(link: &mut Link<IO>, t: &Timing, want: F) -> Result<Msg, String>
where
    IO: AsyncRead + AsyncWrite + Unpin,
    F: Fn(&Msg) -> bool,
{
    tokio::time::timeout(t.setup_step, async {
        loop {
            match link.recv().await? {
                None => return Err("the device closed the connection during setup".to_owned()),
                Some(Msg::PingRequest) => link.send(&Msg::PingResponse).await?,
                Some(Msg::DisconnectRequest) => {
                    link.send(&Msg::DisconnectResponse).await?;
                    return Err("the device ended the connection during setup".to_owned());
                }
                Some(m) if want(&m) => return Ok(m),
                Some(_) => {}
            }
        }
    })
    .await
    .map_err(|_| "the device did not answer during setup".to_owned())?
}

/// Hello, device info, and the entity list (REQ-ESP-005).
async fn setup<IO>(link: &mut Link<IO>, t: &Timing) -> Result<Info, String>
where
    IO: AsyncRead + AsyncWrite + Unpin,
{
    hello(link, t).await?;
    let (friendly, voice_flags) = device_info(link, t).await?;
    let media_key = first_media_player(link, t).await?;
    Ok(Info {
        friendly,
        flags: voice_flags,
        media_key,
    })
}

/// Say hello and check the device speaks a compatible API version.
async fn hello<IO>(link: &mut Link<IO>, t: &Timing) -> Result<(), String>
where
    IO: AsyncRead + AsyncWrite + Unpin,
{
    link.send(&Msg::HelloRequest {
        client_info: "ClawDot".into(),
        major: API_MAJOR,
        minor: API_MINOR,
    })
    .await?;
    let Msg::HelloResponse { major, minor, .. } =
        expect(link, t, |m| matches!(m, Msg::HelloResponse { .. })).await?
    else {
        return Err("unexpected reply to hello".into());
    };
    if major != API_MAJOR {
        return Err(format!(
            "the device speaks ESPHome API {major}.{minor}; this client supports {API_MAJOR}.x"
        ));
    }
    Ok(())
}

/// Ask for the device info; returns the name to show and the voice feature flags, after checking the device can
/// be a voice assistant that carries audio over the API.
async fn device_info<IO>(link: &mut Link<IO>, t: &Timing) -> Result<(String, u32), String>
where
    IO: AsyncRead + AsyncWrite + Unpin,
{
    link.send(&Msg::DeviceInfoRequest).await?;
    let Msg::DeviceInfoResponse {
        name,
        friendly_name,
        voice_flags,
        ..
    } = expect(link, t, |m| matches!(m, Msg::DeviceInfoResponse { .. })).await?
    else {
        return Err("unexpected reply to device info".into());
    };
    if voice_flags & flags::VOICE_ASSISTANT == 0 {
        return Err("the device is not a voice assistant (no VOICE_ASSISTANT feature flag)".into());
    }
    if voice_flags & flags::API_AUDIO == 0 {
        return Err(
            "the device cannot carry audio over the API (no API_AUDIO feature flag)".into(),
        );
    }
    let friendly = if friendly_name.is_empty() {
        name
    } else {
        friendly_name
    };
    Ok((friendly, voice_flags))
}

/// List the entities and return the key of the first media player, if any.
async fn first_media_player<IO>(link: &mut Link<IO>, t: &Timing) -> Result<Option<u32>, String>
where
    IO: AsyncRead + AsyncWrite + Unpin,
{
    link.send(&Msg::ListEntitiesRequest).await?;
    let mut media_key = None;
    for _ in 0..MAX_MEDIA_PLAYERS {
        match expect(link, t, |m| {
            matches!(
                m,
                Msg::ListEntitiesDone | Msg::ListEntitiesMediaPlayer { .. }
            )
        })
        .await?
        {
            Msg::ListEntitiesMediaPlayer { key, .. } => {
                media_key.get_or_insert(key);
            }
            _ => break,
        }
    }
    Ok(media_key)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// The wake was sent; waiting for the pipeline to say it is listening.
    Waking(Instant),
    Listening,
    Thinking,
    Speaking,
}

#[derive(Debug)]
struct Run {
    stage: Stage,
}

/// A reply being streamed to the device over the API.
struct Out {
    pcm: Vec<u8>,
    pos: usize,
    next: Instant,
    burst: u32,
    /// When the audio sent so far will have played out, counting any gap where the device ran dry.
    plays_until: Instant,
}

/// Where a reply that is relayed while the pipeline makes it goes.
enum Sink {
    /// Into [`Out`], which paces it to the device in API messages.
    Api,
    /// Into a live file on the media server, which the device fetches.
    Url(LiveWriter),
}

/// A reply being relayed to the device while the pipeline is still making it (`reply: "streamed"`).
struct Live {
    sink: Sink,
    /// Bytes accepted so far (bounded by `MAX_BYTES`).
    bytes: usize,
    /// When audio last arrived.
    last: Instant,
    /// For a URL: when the audio written so far will have played out.
    plays_until: Instant,
}

/// The playing time of `len` bytes of 16 kHz 16-bit mono audio.
fn playing_time(len: usize) -> Duration {
    Duration::from_micros(len as u64 * 1_000_000 / BYTES_PER_SEC)
}

struct Bridge<IO> {
    link: Link<IO>,
    sat: WriteHalf<DuplexStream>,
    t: Timing,
    info: Info,
    follow_up: bool,
    media: Option<MediaStore>,
    reply_mode: ReplyMode,
    /// A voice run the device started.
    run: Option<Run>,
    /// The pipeline wants microphone audio.
    listening: bool,
    /// The pipeline is working on an answer.
    thinking: bool,
    /// The next reply belongs to an abandoned run and is thrown away.
    discard: bool,
    /// Reply audio arriving from the pipeline.
    rec: Option<Vec<u8>>,
    /// A reply relayed to the device as it arrives, from its first sentence to `audio-stop`.
    live: Option<Live>,
    /// Reply audio going to the device.
    out: Option<Out>,
    /// When a spoken reply counts as played.
    done_at: Option<Instant>,
    /// When to stop waiting for an announcement to finish.
    announce_by: Option<Instant>,
    ping_at: Instant,
    heard: Instant,
    last_audio: Instant,
    silence_at: Instant,
}

impl<IO: AsyncRead + AsyncWrite + Unpin> Bridge<IO> {
    fn new(
        link: Link<IO>,
        sat: WriteHalf<DuplexStream>,
        t: Timing,
        follow_up: bool,
        media: Option<MediaStore>,
        reply_mode: ReplyMode,
        info: Info,
    ) -> Self {
        let now = Instant::now();
        Self {
            link,
            sat,
            t,
            follow_up,
            media,
            reply_mode,
            run: None,
            listening: false,
            thinking: false,
            discard: false,
            rec: None,
            live: None,
            out: None,
            done_at: None,
            announce_by: None,
            ping_at: now + t.ping_every,
            heard: now,
            last_audio: now,
            silence_at: now,
            info,
        }
    }

    async fn sat_send(&mut self, m: &Message) -> Result<(), String> {
        let bytes = encode(&m.to_frame()).map_err(|e| e.to_string())?;
        self.sat
            .write_all(&bytes)
            .await
            .map_err(|e| format!("pipeline: {e}"))
    }

    async fn ev(&mut self, kind: u32, data: &[(&str, &str)]) -> Result<(), String> {
        self.link
            .send(&Msg::VoiceEvent {
                kind,
                data: data
                    .iter()
                    .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
                    .collect(),
            })
            .await
    }

    async fn run_loop(
        &mut self,
        sat_rd: &mut tokio::io::ReadHalf<DuplexStream>,
        mut session: Guard,
    ) -> Result<(), String> {
        let mut dec = Decoder::new(Limits::default());
        loop {
            let wake = tokio::time::Instant::from_std(self.next_wake());
            tokio::select! {
                m = self.link.recv() => {
                    let Some(m) = m? else {
                        return Err("the device closed the connection".into());
                    };
                    self.heard = Instant::now();
                    if self.on_device(m).await? {
                        return Ok(());
                    }
                }
                s = satellite::next_message(sat_rd, &mut dec) => {
                    let Some(m) = s? else {
                        return Err("the pipeline session ended".into());
                    };
                    self.on_session(m).await?;
                }
                () = tokio::time::sleep_until(wake) => self.on_tick().await?,
                r = &mut session.0 => {
                    return Err(match r {
                        Ok(Err(e)) => format!("the pipeline session ended: {e}"),
                        _ => "the pipeline session ended".to_owned(),
                    });
                }
            }
        }
    }

    /// The earliest moment something needs attention.
    fn next_wake(&self) -> Instant {
        let mut at = self.ping_at.min(self.heard + self.t.dead_after);
        let mut soon = |t: Option<Instant>| {
            if let Some(t) = t {
                at = at.min(t);
            }
        };
        // An open stream that has sent everything it has waits for the pipeline, not for the clock.
        let open = self.live.is_some();
        soon(
            self.out
                .as_ref()
                .filter(|o| o.pos < o.pcm.len() || !open)
                .map(|o| o.next),
        );
        soon(self.live.as_ref().map(|l| l.last + STREAM_IDLE));
        soon(self.done_at);
        soon(self.announce_by);
        if let Some(Run {
            stage: Stage::Waking(d),
        }) = &self.run
        {
            soon(Some(*d));
        }
        if matches!(
            self.run,
            Some(Run {
                stage: Stage::Listening
            })
        ) {
            soon(Some(self.last_audio + self.t.mic_idle));
        }
        if self.listening && self.run.is_none() {
            soon(Some(self.silence_at));
        }
        at
    }

    /// Returns true when the connection should end cleanly.
    async fn on_device(&mut self, m: Msg) -> Result<bool, String> {
        match m {
            Msg::PingRequest => self.link.send(&Msg::PingResponse).await?,
            Msg::DisconnectRequest => {
                self.link.send(&Msg::DisconnectResponse).await?;
                return Ok(true);
            }
            Msg::DisconnectResponse => return Ok(true),
            Msg::VoiceRequest {
                start: true,
                wake_word_phrase,
                ..
            } => self.start_run(&wake_word_phrase).await?,
            Msg::VoiceRequest { start: false, .. } => self.abort_run().await?,
            Msg::VoiceAudio { data, end } => self.mic(data, end).await?,
            Msg::AnnounceFinished { .. } => self.end_announcement().await?,
            _ => {}
        }
        Ok(false)
    }

    /// The announcement is over (the device said so, or it timed out): tell the pipeline.
    async fn end_announcement(&mut self) -> Result<(), String> {
        if self.announce_by.take().is_some() {
            self.sat_send(&Message::PlaybackDone { id: None }).await?;
        }
        Ok(())
    }

    async fn start_run(&mut self, phrase: &str) -> Result<(), String> {
        if self.run.is_some() {
            self.abort_run().await?;
        }
        // A device that starts a run has finished with the announcement before it.
        self.end_announcement().await?;
        self.link
            .send(&Msg::VoiceResponse {
                port: 0,
                error: false,
            })
            .await?;
        self.ev(event::RUN_START, &[]).await?;
        self.ev(event::STT_START, &[]).await?;
        self.last_audio = Instant::now();
        if self.listening {
            // The follow-up window after an announcement or a reply is already open.
            self.run = Some(Run {
                stage: Stage::Listening,
            });
        } else {
            let name = if phrase.is_empty() { "esphome" } else { phrase };
            self.run = Some(Run {
                stage: Stage::Waking(Instant::now() + self.t.wake_watch),
            });
            self.sat_send(&Message::Wake(Wake {
                name: name.to_owned(),
                timestamp_ms: None,
            }))
            .await?;
        }
        Ok(())
    }

    /// The device gave up on its run (REQ-ESP-009).
    async fn abort_run(&mut self) -> Result<(), String> {
        let Some(run) = self.run.take() else {
            return Ok(());
        };
        if self.listening {
            // Muting and unmuting is how the dialog is told to stop listening.
            self.sat_send(&Message::MuteChanged { muted: true }).await?;
            self.sat_send(&Message::MuteChanged { muted: false })
                .await?;
            self.listening = false;
        } else if run.stage == Stage::Thinking {
            self.discard = true;
        }
        // A reply being relayed ends with the run: dropping the live file closes the device's download.
        let relaying = self.live.take().is_some();
        if self.out.take().is_some() || self.done_at.take().is_some() || relaying {
            self.sat_send(&Message::PlaybackDone { id: None }).await?;
        }
        Ok(())
    }

    async fn mic(&mut self, data: Vec<u8>, end: bool) -> Result<(), String> {
        let open = matches!(
            self.run,
            Some(Run {
                stage: Stage::Waking(_) | Stage::Listening
            })
        );
        if !open {
            return Ok(());
        }
        let even = data.len() & !1;
        if even > 0 {
            self.last_audio = Instant::now();
            self.sat_send(&Message::AudioChunk {
                format: AudioFormat::PCM_16K_MONO,
                timestamp_ms: None,
                payload: data[..even].to_vec(),
            })
            .await?;
        }
        if end {
            // The device stopped sending; give the endpointer the silence it needs to close the utterance.
            self.silence(1500).await?;
        }
        Ok(())
    }

    /// Feed the pipeline `ms` milliseconds of silence.
    async fn silence(&mut self, ms: usize) -> Result<(), String> {
        let mut left = ms * 32;
        while left > 0 {
            let n = left.min(3200);
            self.sat_send(&Message::AudioChunk {
                format: AudioFormat::PCM_16K_MONO,
                timestamp_ms: None,
                payload: vec![0; n],
            })
            .await?;
            left -= n;
        }
        Ok(())
    }

    async fn on_session(&mut self, m: Message) -> Result<(), String> {
        match m {
            Message::Ping => self.sat_send(&Message::Pong).await?,
            Message::Reject(r) => {
                return Err(format!("the pipeline refused the satellite: {}", r.reason));
            }
            Message::StreamStart => {
                self.listening = true;
                self.silence_at = Instant::now() + Duration::from_millis(100);
                if let Some(run) = &mut self.run {
                    if matches!(run.stage, Stage::Waking(_)) {
                        run.stage = Stage::Listening;
                    }
                }
            }
            Message::StreamStop => self.listening = false,
            Message::SetIndicator(i) => self.indicator(i).await?,
            Message::AudioStart(_) => {
                self.thinking = false;
                self.live = None;
                self.rec = if self.begin_live().await? {
                    None
                } else {
                    Some(Vec::new())
                };
            }
            Message::AudioChunk { payload, .. } => {
                if self.live.is_some() {
                    self.live_chunk(&payload);
                } else if let Some(r) = &mut self.rec {
                    if r.len() + payload.len() <= MAX_BYTES {
                        r.extend_from_slice(&payload);
                    }
                }
            }
            Message::AudioStop => self.reply_ready().await?,
            Message::SetVolume { level } => {
                self.command(None, Some(f32::from(level) / 100.0), None)
                    .await?;
            }
            Message::StopPlayback => {
                self.stop_live().await?;
                self.command(Some(player::STOP), None, None).await?;
            }
            Message::PlayUrl { url, .. } => {
                self.command(Some(player::PLAY), None, Some(url)).await?
            }
            Message::MediaControl(MediaAction::Pause) => {
                self.command(Some(player::PAUSE), None, None).await?;
            }
            Message::MediaControl(MediaAction::Resume) => {
                self.command(Some(player::PLAY), None, None).await?;
            }
            _ => {}
        }
        Ok(())
    }

    async fn command(
        &mut self,
        command: Option<u32>,
        volume: Option<f32>,
        media_url: Option<String>,
    ) -> Result<(), String> {
        let Some(key) = self.info.media_key else {
            return Ok(());
        };
        self.link
            .send(&Msg::MediaPlayerCommand {
                key,
                command,
                volume,
                media_url,
                announcement: None,
            })
            .await
    }

    async fn indicator(&mut self, i: Indicator) -> Result<(), String> {
        match i {
            Indicator::Thinking => {
                self.thinking = true;
                if let Some(run) = &mut self.run {
                    if matches!(run.stage, Stage::Waking(_) | Stage::Listening) {
                        run.stage = Stage::Thinking;
                        self.ev(event::STT_VAD_END, &[]).await?;
                        self.ev(event::STT_END, &[("text", "")]).await?;
                        self.ev(event::INTENT_START, &[]).await?;
                    }
                }
            }
            Indicator::Error | Indicator::Muted => {
                self.thinking = false;
                if self.run.take().is_some() {
                    if i == Indicator::Error {
                        self.ev(
                            event::ERROR,
                            &[("code", "pipeline-error"), ("message", "no answer")],
                        )
                        .await?;
                    }
                    self.ev(event::RUN_END, &[]).await?;
                }
            }
            Indicator::Idle => {
                self.thinking = false;
                let silent = matches!(
                    self.run,
                    Some(Run {
                        stage: Stage::Thinking
                    })
                );
                if silent {
                    // A command that needed no spoken answer.
                    self.run = None;
                    self.ev(event::INTENT_END, &[]).await?;
                    self.ev(event::RUN_END, &[]).await?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn duration_of(pcm: &[u8]) -> Duration {
        Duration::from_millis(pcm.len() as u64 * 1000 / BYTES_PER_SEC)
    }

    /// The events that open a spoken reply to a run: it counts as answered, and speech begins.
    async fn open_reply(&mut self) -> Result<(), String> {
        if let Some(run) = &mut self.run {
            run.stage = Stage::Speaking;
        }
        let cont: &[(&str, &str)] = if self.follow_up {
            &[("continue_conversation", "1")]
        } else {
            &[]
        };
        self.ev(event::INTENT_END, cont).await?;
        self.ev(event::TTS_START, &[]).await
    }

    /// A reply starts arriving (`audio-start`). In `streamed` mode, a reply to a run the device started is
    /// relayed from its first sentence: returns true when it is, false when it is to be collected and delivered
    /// at `audio-stop` (the `whole` mode, announcements, an abandoned run, no media server for a URL).
    async fn begin_live(&mut self) -> Result<bool, String> {
        if self.reply_mode != ReplyMode::Streamed || self.discard || self.run.is_none() {
            return Ok(false);
        }
        let now = Instant::now();
        let sink = if self.info.flags & flags::SPEAKER != 0 {
            self.open_reply().await?;
            self.ev(event::TTS_STREAM_START, &[]).await?;
            self.out = Some(Out {
                pcm: Vec::new(),
                pos: 0,
                next: now,
                burst: AUDIO_BURST,
                plays_until: now,
            });
            Sink::Api
        } else if let Some((url, writer)) = self.media.as_ref().and_then(MediaStore::begin_live) {
            self.open_reply().await?;
            self.ev(event::TTS_END, &[("url", &url)]).await?;
            Sink::Url(writer)
        } else {
            return Ok(false);
        };
        self.live = Some(Live {
            sink,
            bytes: 0,
            last: now,
            plays_until: now,
        });
        Ok(true)
    }

    /// Audio of a reply being relayed.
    fn live_chunk(&mut self, payload: &[u8]) {
        let Some(l) = &mut self.live else { return };
        if l.bytes + payload.len() > MAX_BYTES {
            return;
        }
        let now = Instant::now();
        l.bytes += payload.len();
        l.last = now;
        match &l.sink {
            Sink::Api => {
                if let Some(o) = &mut self.out {
                    if o.pos >= o.pcm.len() {
                        // The sender had run dry while this sentence was made. Pace from now: its schedule is
                        // as old as the gap, and following it would send the whole sentence at once.
                        o.next = o.next.max(now);
                    }
                    o.pcm.extend_from_slice(payload);
                }
            }
            Sink::Url(w) => {
                l.plays_until = l.plays_until.max(now) + playing_time(payload.len());
                w.push(payload);
            }
        }
    }

    /// The pipeline has sent the last sentence of a relayed reply. Over the API the paced sender finishes it
    /// after the last byte; for a URL the file is closed and the reply counts as played once the audio written
    /// has had time to play.
    fn end_live(&mut self) {
        let Some(l) = self.live.take() else { return };
        if let Sink::Url(w) = l.sink {
            w.finish();
            self.done_at = Some((l.plays_until + self.t.url_margin).max(Instant::now()));
        }
    }

    /// The listener stopped the reply: what the device has not been given is dropped, the stream is closed, and
    /// the run ends.
    async fn stop_live(&mut self) -> Result<(), String> {
        let Some(l) = self.live.take() else {
            return Ok(());
        };
        if matches!(l.sink, Sink::Api) {
            self.out = None;
            self.link
                .send(&Msg::VoiceAudio {
                    data: Vec::new(),
                    end: true,
                })
                .await?;
            self.ev(event::TTS_STREAM_END, &[]).await?;
            self.ev(event::TTS_END, &[]).await?;
        }
        // For a URL, dropping the writer ends the download cleanly.
        drop(l);
        if self.run.take().is_some() {
            self.ev(event::RUN_END, &[]).await?;
        }
        self.sat_send(&Message::PlaybackDone { id: None }).await
    }

    /// The pipeline finished sending a spoken reply.
    async fn reply_ready(&mut self) -> Result<(), String> {
        if self.live.is_some() {
            self.end_live();
            return Ok(());
        }
        let Some(pcm) = self.rec.take() else {
            return Ok(());
        };
        if self.discard {
            self.discard = false;
            return self.sat_send(&Message::PlaybackDone { id: None }).await;
        }
        if self.run.is_some() {
            self.speak_turn(pcm).await
        } else {
            self.speak_announcement(pcm).await
        }
    }

    /// A reply to a run the device started (REQ-ESP-008).
    async fn speak_turn(&mut self, pcm: Vec<u8>) -> Result<(), String> {
        let dur = Self::duration_of(&pcm);
        self.open_reply().await?;
        let api = self.info.flags & flags::SPEAKER != 0;
        if api {
            self.ev(event::TTS_STREAM_START, &[]).await?;
            let now = Instant::now();
            self.out = Some(Out {
                pcm,
                pos: 0,
                next: now,
                burst: AUDIO_BURST,
                plays_until: now,
            });
        } else if let Some(url) = self.media.as_ref().and_then(|m| m.put(wav_from_pcm(&pcm))) {
            self.ev(event::TTS_END, &[("url", &url)]).await?;
            self.done_at = Some(Instant::now() + dur + self.t.url_margin);
        } else {
            self.ev(
                event::ERROR,
                &[
                    ("code", "no-media"),
                    ("message", "cannot deliver the reply"),
                ],
            )
            .await?;
            self.done_at = Some(Instant::now());
        }
        Ok(())
    }

    /// Send the next paced audio messages (REQ-ESP-008).
    async fn pump_out(&mut self) -> Result<(), String> {
        for _ in 0..8 {
            let open = self.live.is_some();
            let Some(o) = &mut self.out else {
                return Ok(());
            };
            let now = Instant::now();
            if now < o.next {
                return Ok(());
            }
            if o.pos >= o.pcm.len() {
                if open {
                    // Everything made so far is sent; the next sentence is still being synthesised. A gap
                    // here is not the end of the reply.
                    return Ok(());
                }
                let play_end = o.plays_until + self.t.api_margin;
                self.out = None;
                self.link
                    .send(&Msg::VoiceAudio {
                        data: Vec::new(),
                        end: true,
                    })
                    .await?;
                self.ev(event::TTS_STREAM_END, &[]).await?;
                self.ev(event::TTS_END, &[]).await?;
                self.done_at = Some(play_end.max(now));
                return Ok(());
            }
            let end = (o.pos + AUDIO_CHUNK).min(o.pcm.len());
            let data = o.pcm[o.pos..end].to_vec();
            o.pos = end;
            o.plays_until = o.plays_until.max(now) + playing_time(data.len());
            if o.burst > 0 {
                o.burst -= 1;
            } else {
                o.next += self.t.chunk_every;
            }
            self.link
                .send(&Msg::VoiceAudio { data, end: false })
                .await?;
        }
        Ok(())
    }

    /// A spoken announcement from the inbox or an alarm (REQ-ESP-010).
    async fn speak_announcement(&mut self, pcm: Vec<u8>) -> Result<(), String> {
        let dur = Self::duration_of(&pcm);
        let url = self.media.as_ref().and_then(|m| m.put(wav_from_pcm(&pcm)));
        let Some(url) = url else {
            eprintln!(
                "clawdot: esphome: no media server is configured, so the announcement was not played"
            );
            return self.sat_send(&Message::PlaybackDone { id: None }).await;
        };
        if self.info.flags & flags::ANNOUNCE != 0 {
            let start_conversation =
                self.follow_up && self.info.flags & flags::START_CONVERSATION != 0;
            self.link
                .send(&Msg::AnnounceRequest {
                    media_id: url,
                    text: String::new(),
                    start_conversation,
                })
                .await?;
            self.announce_by = Some(Instant::now() + dur + self.t.announce_margin);
        } else if let Some(key) = self.info.media_key {
            self.link
                .send(&Msg::MediaPlayerCommand {
                    key,
                    command: None,
                    volume: None,
                    media_url: Some(url),
                    announcement: Some(true),
                })
                .await?;
            self.announce_by = Some(Instant::now() + dur + self.t.url_margin);
        } else {
            eprintln!("clawdot: esphome: the device can neither announce nor play media");
            self.sat_send(&Message::PlaybackDone { id: None }).await?;
        }
        Ok(())
    }

    async fn on_tick(&mut self) -> Result<(), String> {
        let now = Instant::now();
        if now.duration_since(self.heard) >= self.t.dead_after {
            return Err("the device stopped responding".into());
        }
        if now >= self.ping_at {
            self.link.send(&Msg::PingRequest).await?;
            self.ping_at = now + self.t.ping_every;
        }
        match self.run.as_ref().map(|r| r.stage) {
            Some(Stage::Waking(deadline)) if now >= deadline => {
                self.run = None;
                self.ev(
                    event::ERROR,
                    &[("code", "busy"), ("message", "the assistant is busy")],
                )
                .await?;
                self.ev(event::RUN_END, &[]).await?;
            }
            Some(Stage::Listening) if now.duration_since(self.last_audio) >= self.t.mic_idle => {
                self.abort_run().await?;
                self.ev(event::RUN_END, &[]).await?;
            }
            _ => {}
        }
        if self
            .live
            .as_ref()
            .is_some_and(|l| now.duration_since(l.last) >= STREAM_IDLE)
        {
            // The pipeline went quiet in the middle of a reply: end what was sent rather than hold the device.
            eprintln!(
                "clawdot: esphome {}: no audio for {} s in the middle of a reply; ending it",
                clean(&self.info.friendly),
                STREAM_IDLE.as_secs()
            );
            self.end_live();
        }
        self.pump_out().await?;
        if self.done_at.is_some_and(|d| now >= d) && self.out.is_none() {
            self.done_at = None;
            if self.run.take().is_some() {
                self.ev(event::RUN_END, &[]).await?;
            }
            self.sat_send(&Message::PlaybackDone { id: None }).await?;
        }
        if self.announce_by.is_some_and(|d| now >= d) {
            self.announce_by = None;
            self.sat_send(&Message::PlaybackDone { id: None }).await?;
        }
        // An open listening window with no run behind it (after an announcement or a reply) is closed by
        // the pipeline's own no-speech timeout, which needs audio to measure against.
        if self.listening && self.run.is_none() && now >= self.silence_at {
            self.silence_at = now + Duration::from_millis(100);
            self.silence(100).await?;
        }
        Ok(())
    }
}

#[cfg(all(test, feature = "esphome-noise"))]
mod wipe_tests {
    use super::Device;

    // REQ-ESP-014: the wipe function leaves no key bytes behind (this tests the function, not freed memory).
    #[test]
    fn req_esp_014_wiping_a_device_clears_its_key() {
        let mut d = Device {
            name: "dot".into(),
            addr: "dot.local:6053".into(),
            psk: Some([9; 32]),
        };
        let copy = d.clone();
        d.wipe_key();
        assert_eq!(d.psk, None);
        assert_eq!(copy.psk, Some([9; 32]), "a clone keeps its own copy");
    }
}
