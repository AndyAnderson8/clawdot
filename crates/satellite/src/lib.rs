//! ClawDot reference satellite: the smallest correct client of the satellite protocol (spec 0003).
//!
//! A port to real hardware implements [`Device`] (a microphone, a speaker, and some lights) and calls
//! [`run`]. Everything protocol-shaped (handshake, keepalive, streaming on request, playback bookkeeping,
//! ignoring unknown events) is here, in plain blocking `std` code, so it can be read in one sitting and
//! translated to C or Go. [`FileDevice`] is a development device that "speaks" samples from memory and
//! collects the reply.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod audio;
pub mod command;
pub mod front;
pub mod hw;

use clawdot_protocol::message::{
    AudioFormat, Capabilities, Hello, Indicator, Message, PROTOCOL_VERSION, Wake,
};
use clawdot_protocol::{Decoder, Limits, encode_checked};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

/// What a satellite must provide. A method that does nothing is valid for hardware the device lacks.
pub trait Device {
    /// The next 20 ms of microphone audio (320 samples at 16 kHz), or `None` when there is none right now.
    /// Called only while the core has asked for audio.
    fn read_mic(&mut self) -> Option<Vec<i16>>;
    /// A reply is about to arrive: `begin_playback`, many `play`, then `end_playback`.
    fn begin_playback(&mut self);
    /// One piece of reply audio (16 kHz mono).
    fn play(&mut self, pcm: &[i16]);
    /// The reply is complete; finish playing it. The client then reports `playback-done`.
    fn end_playback(&mut self);
    /// Stop playing immediately.
    fn stop_playback(&mut self);
    /// Show a state on the lights or screen.
    fn indicator(&mut self, state: Indicator);
    /// Set the speaker volume, 0 to 100.
    fn set_volume(&mut self, level: u8);
    /// Mute or unmute the microphone as the core asked.
    fn set_mute(&mut self, muted: bool);
    /// Whether this device can fetch and play an audio stream from a URL itself (internet radio). If false, the
    /// core never sends stream commands.
    fn supports_streams(&self) -> bool {
        false
    }
    /// Start playing the stream at `url`, replacing whatever was playing. Only called if `supports_streams`.
    fn play_url(&mut self, _url: &str, _title: &str) {}
    /// Pause or resume stream playback. Only called if `supports_streams`.
    fn pause_stream(&mut self, _paused: bool) {}
    /// Return true to disconnect cleanly (for development devices that do one exchange).
    fn finished(&self) -> bool {
        false
    }
    /// The core started (`true`) or stopped (`false`) asking for microphone audio. A device may open or close
    /// its capture here.
    fn mic_active(&mut self, _on: bool) {}
    /// How many frames [`Device::read_mic`] may be called for in one pass of the loop, to catch up when the
    /// loop was busy. One frame per pass cannot keep up with a live microphone.
    fn mic_burst(&self) -> usize {
        1
    }
    /// Called on every pass of the loop. Return true when the user pressed the talk key or a wake word was
    /// heard; the satellite then sends `wake` (REQ-VOI-017). This is where a wake-word detector plugs in.
    fn poll_wake(&mut self) -> bool {
        false
    }
    /// The `name` sent in the `detection` after [`Device::poll_wake`] returned true: the wake word that was
    /// heard, or `button` when a key or button stood in for it (spec 0003 section 6.3).
    fn wake_name(&self) -> String {
        "button".to_owned()
    }
    /// Whether the reply has finished playing. The satellite reports `playback-done` only when this is true,
    /// polling it on every pass so a long reply never stops the keepalive (REQ-VOI-018).
    fn playback_idle(&mut self) -> bool {
        true
    }
    /// What this device declares in `hello` (REQ-SAT-003). The default is the mandatory set only. `wake_on_connect`
    /// and `supports_streams` are added to it.
    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }
    /// A mute change made on the device itself (a button), to be reported to the core as `clawdot.mute-changed`.
    /// Polled on every pass of the loop; `None` when there is nothing to report. A mute the core asked for is not
    /// reported back.
    fn poll_mute_change(&mut self) -> Option<bool> {
        None
    }
    /// The owner held the action button long enough to ask for the administrator reset window (REQ-DEV-031).
    /// Polled on every pass; return true once per request and the satellite sends `clawdot.admin-reset`.
    fn poll_admin_reset(&mut self) -> bool {
        false
    }
    /// The core answered an administrator reset request: it is open for `open_secs` seconds, or refused when
    /// that is 0 (REQ-SAT-020). A device shows this with a tone or light.
    fn admin_reset_window(&mut self, _open_secs: u32, _reason: Option<&str>) {}
}

/// Connection settings.
#[derive(Clone)]
pub struct Config {
    /// Stable device identifier.
    pub device_id: String,
    /// Shared token, if the core requires one. Never printed: `Debug` shows only whether it is set.
    pub token: Option<String>,
    /// Send `wake` right after the handshake, as a device with a wake word or button would.
    pub wake_on_connect: bool,
    /// Treat the core as gone after this long without hearing anything (REQ-SAT-008). The core pings every
    /// 5 s, so silence this long means the connection is dead. [`DEAD_AFTER`] is the protocol default.
    pub dead_after: Duration,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("device_id", &self.device_id)
            .field("token", &self.token.as_ref().map(|_| "<set>"))
            .field("wake_on_connect", &self.wake_on_connect)
            .field("dead_after", &self.dead_after)
            .finish()
    }
}

/// Default dead-peer timeout (REQ-SAT-008).
pub const DEAD_AFTER: Duration = Duration::from_secs(20);

/// How long to wait before reconnect attempt number `attempt` (0 is the first retry): 1 s, doubling, at most 30 s
/// (REQ-SAT-008).
#[must_use]
pub fn backoff(attempt: u32) -> Duration {
    Duration::from_secs((1u64 << attempt.min(5)).min(30))
}

/// Whether an error from [`run`] is the core refusing this satellite (bad token or version). Retrying cannot fix
/// that, so a reconnect loop should stop and show the reason.
#[must_use]
pub fn is_refusal(error: &str) -> bool {
    error.starts_with(REFUSED)
}

const REFUSED: &str = "the core refused this satellite";

/// What happened during a session.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Report {
    /// Indicator states, in order.
    pub indicators: Vec<Indicator>,
    /// Samples of reply audio received.
    pub samples_played: usize,
}

const FRAME_SAMPLES: usize = 320;
const IO_TIMEOUT: Duration = Duration::from_millis(20);

/// Most indicator states kept in [`Report::indicators`]; the oldest are dropped, so a long session stays small.
pub const MAX_REPORTED_INDICATORS: usize = 256;
/// Longest `reject` reason shown, in characters.
const MAX_REASON_CHARS: usize = 200;

/// Encode within the protocol limits (REQ-SAT-017): a message the core would refuse is an error here, not on
/// the wire. A write that cannot finish in time means the peer has stopped reading, which is a dead peer
/// (REQ-SAT-008).
fn send(sock: &mut TcpStream, m: &Message) -> Result<(), String> {
    let bytes = encode_checked(&m.to_frame(), Limits::default()).map_err(|e| e.to_string())?;
    sock.write_all(&bytes).map_err(|e| {
        if matches!(
            e.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
        ) {
            "the core stopped responding (it is not reading what is sent to it)".to_owned()
        } else {
            format!("write: {e}")
        }
    })
}

/// Text from the peer, safe to print: control characters removed, at most [`MAX_REASON_CHARS`] characters.
fn plain(text: &str) -> String {
    let mut out: String = text
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_REASON_CHARS + 1)
        .collect();
    if out.chars().count() > MAX_REASON_CHARS {
        out = out.chars().take(MAX_REASON_CHARS).collect();
        out.push_str("...");
    }
    out
}

/// Whether audio is the format the satellite plays (REQ-SAT-004): 16 kHz, 16-bit, mono.
fn check_format(f: AudioFormat) -> Result<(), String> {
    if f == AudioFormat::PCM_16K_MONO {
        Ok(())
    } else {
        Err(format!(
            "the core sent audio as {} Hz, {} bytes per sample, {} channels; only 16000 Hz, 2 bytes, 1 channel is supported",
            f.rate, f.width, f.channels
        ))
    }
}

/// Return one message if ready. `Ok(None)` means nothing yet; `closed` is set when the peer hung up.
fn poll(
    sock: &mut TcpStream,
    dec: &mut Decoder,
    closed: &mut bool,
) -> Result<Option<Message>, String> {
    if let Some(f) = dec.next_frame().map_err(|e| e.to_string())? {
        return Message::from_frame(f).map(Some).map_err(|e| e.to_string());
    }
    let mut buf = [0u8; 8192];
    match sock.read(&mut buf) {
        Ok(0) => {
            *closed = true;
            Ok(None)
        }
        Ok(n) => {
            dec.push(&buf[..n]);
            Ok(None)
        }
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ) =>
        {
            Ok(None)
        }
        Err(e) => Err(format!("read: {e}")),
    }
}

fn samples(payload: &[u8]) -> Vec<i16> {
    payload
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]))
        .collect()
}

/// Run one session on an open connection until the device is finished or the core hangs up.
///
/// # Errors
/// A rejected handshake (with the core's reason), a protocol violation, or a network failure.
pub fn run(mut sock: TcpStream, device: &mut impl Device, cfg: &Config) -> Result<Report, String> {
    sock.set_read_timeout(Some(IO_TIMEOUT))
        .map_err(|e| e.to_string())?;
    // Without this a core that stops reading blocks write_all for ever, and the dead-peer rule never runs.
    sock.set_write_timeout(Some(cfg.dead_after.max(IO_TIMEOUT)))
        .map_err(|e| e.to_string())?;
    let mut dec = Decoder::new(Limits::default());
    let mut closed = false;
    send(
        &mut sock,
        &Message::Hello(Hello {
            protocol: PROTOCOL_VERSION,
            device_id: cfg.device_id.clone(),
            name: None,
            token: cfg.token.clone(),
            capabilities: {
                let mut caps = device.capabilities();
                caps.wake_word |= cfg.wake_on_connect;
                caps.media_url |= device.supports_streams();
                caps
            },
        }),
    )?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match poll(&mut sock, &mut dec, &mut closed)? {
            Some(Message::Welcome(_)) => break,
            Some(Message::Reject(r)) => {
                return Err(format!("{REFUSED}: {}", plain(&r.reason)));
            }
            Some(_) => {}
            None if closed => {
                return Err("the core closed the connection during the handshake".into());
            }
            None if Instant::now() > deadline => return Err("no welcome from the core".into()),
            None => {}
        }
    }
    if cfg.wake_on_connect {
        send(
            &mut sock,
            &Message::Wake(Wake {
                name: "button".into(),
                timestamp_ms: None,
            }),
        )?;
    }

    let mut report = Report::default();
    let mut streaming = false;
    let mut muted = false;
    let mut last_heard = Instant::now();
    let mut last_ping = Instant::now();
    let mut done_pending = false;
    loop {
        if done_pending && device.playback_idle() {
            done_pending = false;
            send(&mut sock, &Message::PlaybackDone { id: None })?;
        }
        if closed || device.finished() {
            return Ok(report);
        }
        if let Some(m) = device.poll_mute_change() {
            muted = m;
            send(&mut sock, &Message::MuteChanged { muted: m })?;
        }
        if device.poll_admin_reset() {
            send(&mut sock, &Message::AdminReset)?;
        }
        if device.poll_wake() {
            send(
                &mut sock,
                &Message::Wake(Wake {
                    name: device.wake_name(),
                    timestamp_ms: None,
                }),
            )?;
        }
        if last_heard.elapsed() >= cfg.dead_after {
            return Err("the core stopped responding".into());
        }
        // Either side may ping (REQ-SAT-008); this also exposes a half-open connection when the core is quiet.
        if last_ping.elapsed() >= cfg.dead_after / 4 {
            last_ping = Instant::now();
            send(&mut sock, &Message::Ping)?;
        }
        if streaming && !muted {
            for _ in 0..device.mic_burst().max(1) {
                let Some(pcm) = device.read_mic() else { break };
                let payload: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
                send(
                    &mut sock,
                    &Message::AudioChunk {
                        format: AudioFormat::PCM_16K_MONO,
                        timestamp_ms: None,
                        payload,
                    },
                )?;
            }
        }
        let Some(msg) = poll(&mut sock, &mut dec, &mut closed)? else {
            continue;
        };
        last_heard = Instant::now();
        match msg {
            Message::Ping => send(&mut sock, &Message::Pong)?,
            Message::StreamStart => {
                streaming = true;
                device.mic_active(true);
            }
            Message::StreamStop => {
                streaming = false;
                device.mic_active(false);
            }
            Message::SetIndicator(i) => {
                if report.indicators.len() >= MAX_REPORTED_INDICATORS {
                    report.indicators.remove(0);
                }
                report.indicators.push(i);
                device.indicator(i);
            }
            Message::SetVolume { level } => device.set_volume(level),
            Message::SetMute { muted: m } => {
                muted = m;
                device.set_mute(m);
            }
            Message::StopPlayback => device.stop_playback(),
            Message::AdminResetWindow { open_secs, reason } => {
                device.admin_reset_window(open_secs, reason.as_deref());
            }
            Message::PlayUrl { url, title } if device.supports_streams() => {
                device.play_url(&url, &title)
            }
            Message::MediaControl(a) if device.supports_streams() => {
                device.pause_stream(a == clawdot_protocol::message::MediaAction::Pause);
            }
            Message::AudioStart(f) => {
                check_format(f)?;
                device.begin_playback();
            }
            Message::AudioChunk {
                format, payload, ..
            } => {
                check_format(format)?;
                let pcm = samples(&payload);
                report.samples_played += pcm.len();
                device.play(&pcm);
            }
            Message::AudioStop => {
                device.end_playback();
                if device.playback_idle() {
                    send(&mut sock, &Message::PlaybackDone { id: None })?;
                } else {
                    done_pending = true;
                }
            }
            Message::Reject(r) => {
                return Err(format!("the core ended the session: {}", plain(&r.reason)));
            }
            // Welcome again, errors, events from the satellite side, and anything unknown: ignore (REQ-SAT-009).
            _ => {}
        }
    }
}

/// A development device: its microphone is a list of samples (then silence), and the reply is collected.
#[derive(Debug, Default)]
pub struct FileDevice {
    mic: Vec<i16>,
    pos: usize,
    /// Reply audio received, concatenated.
    pub reply: Vec<i16>,
    /// Number of replies completed.
    pub replies: usize,
    /// Last volume the core set.
    pub volume: Option<u8>,
    /// Stop after this many replies.
    pub stop_after: usize,
    /// Streams the core asked to play, as (url, title).
    pub streams: Vec<(String, String)>,
    /// Pause (true) and resume (false) commands received, in order.
    pub pauses: Vec<bool>,
    /// Whether to declare stream support.
    pub can_stream: bool,
    /// Keep the session open until this many streams have been started (for tests of radio).
    pub wait_for_streams: usize,
}

impl FileDevice {
    /// A device that will "say" `utterance` followed by about 1.5 s of silence, and stop after one reply.
    #[must_use]
    pub fn new(mut utterance: Vec<i16>) -> Self {
        utterance.extend(std::iter::repeat_n(0i16, 24_000));
        Self {
            mic: utterance,
            stop_after: 1,
            ..Self::default()
        }
    }
}

impl Device for FileDevice {
    fn read_mic(&mut self) -> Option<Vec<i16>> {
        if self.pos >= self.mic.len() {
            return None;
        }
        let end = (self.pos + FRAME_SAMPLES).min(self.mic.len());
        let frame = self.mic[self.pos..end].to_vec();
        self.pos = end;
        Some(frame)
    }
    fn begin_playback(&mut self) {}
    fn play(&mut self, pcm: &[i16]) {
        self.reply.extend_from_slice(pcm);
    }
    fn end_playback(&mut self) {
        self.replies += 1;
    }
    fn stop_playback(&mut self) {}
    fn indicator(&mut self, _: Indicator) {}
    fn set_volume(&mut self, level: u8) {
        self.volume = Some(level);
    }
    fn supports_streams(&self) -> bool {
        self.can_stream
    }
    fn play_url(&mut self, url: &str, title: &str) {
        self.streams.push((url.to_owned(), title.to_owned()));
    }
    fn pause_stream(&mut self, paused: bool) {
        self.pauses.push(paused);
    }
    fn set_mute(&mut self, _: bool) {}
    fn finished(&self) -> bool {
        self.replies >= self.stop_after && self.streams.len() >= self.wait_for_streams
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clawdot_protocol::message::{AudioCodec, Reject, Welcome};
    use std::net::TcpListener;

    /// A scripted core: reads the satellite's hello, then runs `script` on the connection.
    fn fake_core(
        script: impl FnOnce(&mut TcpStream, &mut Decoder) + Send + 'static,
    ) -> (u16, std::thread::JoinHandle<()>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let h = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            s.set_read_timeout(Some(Duration::from_millis(20))).unwrap();
            let mut dec = Decoder::new(Limits::default());
            let mut closed = false;
            let hello = loop {
                if let Some(m) = poll(&mut s, &mut dec, &mut closed).unwrap() {
                    break m;
                }
                assert!(!closed);
            };
            assert!(matches!(hello, Message::Hello(_)));
            script(&mut s, &mut dec);
        });
        (port, h)
    }

    fn next(s: &mut TcpStream, dec: &mut Decoder, closed: &mut bool) -> Message {
        let end = Instant::now() + Duration::from_secs(5);
        while Instant::now() < end {
            if let Some(m) = poll(s, dec, closed).unwrap() {
                return m;
            }
            assert!(!*closed, "the satellite closed the connection");
        }
        panic!("timed out waiting for the satellite");
    }

    fn cfg() -> Config {
        Config {
            device_id: "t".into(),
            token: Some("tok".into()),
            wake_on_connect: true,
            dead_after: DEAD_AFTER,
        }
    }

    fn welcome() -> Message {
        Message::Welcome(Welcome {
            protocol: PROTOCOL_VERSION,
            session_id: "s".into(),
            audio_out: AudioCodec::Pcm,
        })
    }

    #[test]
    fn req_sat_008_a_silent_core_is_declared_dead_and_the_satellite_pings() {
        let (port, h) = fake_core(|s, dec| {
            let mut closed = false;
            send(s, &welcome()).unwrap();
            // Say nothing more; the satellite must ping on its own, then give up.
            assert!(matches!(next(s, dec, &mut closed), Message::Ping));
            let end = Instant::now() + Duration::from_secs(5);
            while Instant::now() < end && !closed {
                let _ = poll(s, dec, &mut closed);
            }
            assert!(closed, "the satellite hung up on the silent core");
        });
        let sock = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let cfg = Config {
            wake_on_connect: false,
            dead_after: Duration::from_millis(400),
            ..cfg()
        };
        let e = run(sock, &mut FileDevice::new(vec![]), &cfg).unwrap_err();
        assert!(e.contains("stopped responding"), "{e}");
        assert!(!is_refusal(&e), "a dead link is worth retrying");
        h.join().unwrap();
    }

    /// A device that declares LEDs and reports one local mute press.
    struct Pressing {
        mute: Option<bool>,
    }

    impl Device for Pressing {
        fn read_mic(&mut self) -> Option<Vec<i16>> {
            None
        }
        fn begin_playback(&mut self) {}
        fn play(&mut self, _: &[i16]) {}
        fn end_playback(&mut self) {}
        fn stop_playback(&mut self) {}
        fn indicator(&mut self, _: Indicator) {}
        fn set_volume(&mut self, _: u8) {}
        fn set_mute(&mut self, _: bool) {}
        fn capabilities(&self) -> Capabilities {
            Capabilities {
                leds: true,
                buttons: true,
                ..Capabilities::default()
            }
        }
        fn poll_mute_change(&mut self) -> Option<bool> {
            self.mute.take()
        }
        fn finished(&self) -> bool {
            self.mute.is_none()
        }
    }

    #[test]
    fn req_dev_002_a_device_declares_its_capabilities_and_reports_a_local_mute() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let h = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            s.set_read_timeout(Some(Duration::from_millis(20))).unwrap();
            let mut dec = Decoder::new(Limits::default());
            let mut closed = false;
            let hello = loop {
                if let Some(m) = poll(&mut s, &mut dec, &mut closed).unwrap() {
                    break m;
                }
            };
            let Message::Hello(hello) = hello else {
                panic!("not a hello")
            };
            assert!(hello.capabilities.leds && hello.capabilities.buttons);
            assert!(
                hello.capabilities.wake_word,
                "wake_on_connect is added to the device's own set"
            );
            assert!(!hello.capabilities.hardware_mute);
            send(&mut s, &welcome()).unwrap();
            loop {
                if let Message::MuteChanged { muted } = next(&mut s, &mut dec, &mut closed) {
                    assert!(muted);
                    return;
                }
            }
        });
        let sock = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let mut dev = Pressing { mute: Some(true) };
        // `finished` turns true once the press was taken, which happens on the first pass after the handshake.
        run(sock, &mut dev, &cfg()).unwrap();
        h.join().unwrap();
    }

    #[test]
    fn req_sat_008_backoff_doubles_and_is_capped() {
        let secs: Vec<u64> = (0..8).map(|n| backoff(n).as_secs()).collect();
        assert_eq!(secs, [1, 2, 4, 8, 16, 30, 30, 30]);
        assert_eq!(backoff(u32::MAX).as_secs(), 30, "no overflow");
    }

    /// A device with a configurable microphone and wake, and nothing else.
    #[derive(Default)]
    struct Plain {
        mic_samples: usize,
        wake: Option<String>,
    }

    impl Device for Plain {
        fn read_mic(&mut self) -> Option<Vec<i16>> {
            (self.mic_samples > 0).then(|| vec![0; self.mic_samples])
        }
        fn begin_playback(&mut self) {}
        fn play(&mut self, _: &[i16]) {}
        fn end_playback(&mut self) {}
        fn stop_playback(&mut self) {}
        fn indicator(&mut self, _: Indicator) {}
        fn set_volume(&mut self, _: u8) {}
        fn set_mute(&mut self, _: bool) {}
        fn poll_wake(&mut self) -> bool {
            self.wake.is_some()
        }
        fn wake_name(&self) -> String {
            self.wake.clone().unwrap_or_default()
        }
        fn finished(&self) -> bool {
            false
        }
    }

    /// Run `plain` against a core script on a thread; fail the test if the session does not end by itself.
    fn run_bounded(
        port: u16,
        mut dev: Plain,
        cfg: Config,
        limit: Duration,
    ) -> Result<Report, String> {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let sock = TcpStream::connect(("127.0.0.1", port)).unwrap();
            let _ = tx.send(run(sock, &mut dev, &cfg));
        });
        rx.recv_timeout(limit)
            .expect("the satellite never ended the session")
    }

    #[test]
    fn req_sat_008_a_core_that_stops_reading_is_a_dead_peer_not_a_hang() {
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        let (port, h) = fake_core(move |s, _| {
            send(s, &welcome()).unwrap();
            send(s, &Message::StreamStart).unwrap();
            // Read nothing more, but keep the connection open until the test is over.
            let _ = done_rx.recv_timeout(Duration::from_secs(60));
        });
        let cfg = Config {
            wake_on_connect: false,
            dead_after: Duration::from_millis(400),
            ..cfg()
        };
        let dev = Plain {
            mic_samples: 400_000,
            ..Plain::default()
        };
        let e = run_bounded(port, dev, cfg, Duration::from_secs(30)).unwrap_err();
        assert!(e.contains("stopped responding"), "{e}");
        assert!(!is_refusal(&e), "a dead link is worth retrying");
        let _ = done_tx.send(());
        h.join().unwrap();
    }

    #[test]
    fn req_sat_017_a_message_over_the_limits_is_not_sent() {
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        let (port, h) = fake_core(move |s, _| {
            send(s, &welcome()).unwrap();
            send(s, &Message::StreamStart).unwrap();
            let _ = done_rx.recv_timeout(Duration::from_secs(60));
        });
        let cfg = Config {
            wake_on_connect: false,
            ..cfg()
        };
        // 1.2 MB in one chunk is over the 1 MiB payload limit.
        let dev = Plain {
            mic_samples: 600_000,
            ..Plain::default()
        };
        let e = run_bounded(port, dev, cfg, Duration::from_secs(30)).unwrap_err();
        assert!(e.contains("payload too long"), "{e}");
        let _ = done_tx.send(());
        h.join().unwrap();
    }

    #[test]
    fn req_sat_006_a_wake_word_is_sent_under_its_own_name() {
        let (name_tx, name_rx) = std::sync::mpsc::channel();
        let (port, h) = fake_core(move |s, dec| {
            let mut closed = false;
            send(s, &welcome()).unwrap();
            loop {
                if let Message::Wake(w) = next(s, dec, &mut closed) {
                    name_tx.send(w.name).unwrap();
                    return;
                }
            }
        });
        let dev = Plain {
            wake: Some("okay nabu".into()),
            ..Plain::default()
        };
        let cfg = Config {
            wake_on_connect: false,
            ..cfg()
        };
        let _ = run_bounded(port, dev, cfg, Duration::from_secs(30));
        assert_eq!(name_rx.recv().unwrap(), "okay nabu");
        h.join().unwrap();
    }

    #[test]
    fn req_sat_004_audio_in_another_format_is_a_protocol_error() {
        let bad = AudioFormat {
            rate: 24_000,
            width: 2,
            channels: 1,
        };
        for chunk in [false, true] {
            let (port, h) = fake_core(move |s, _| {
                send(s, &welcome()).unwrap();
                if chunk {
                    send(s, &Message::AudioStart(AudioFormat::PCM_16K_MONO)).unwrap();
                    send(
                        s,
                        &Message::AudioChunk {
                            format: bad,
                            timestamp_ms: None,
                            payload: vec![0; 640],
                        },
                    )
                    .unwrap();
                } else {
                    send(s, &Message::AudioStart(bad)).unwrap();
                }
                std::thread::sleep(Duration::from_millis(300));
            });
            let cfg = Config {
                wake_on_connect: false,
                ..cfg()
            };
            let e = run_bounded(port, Plain::default(), cfg, Duration::from_secs(30)).unwrap_err();
            assert!(e.contains("24000"), "{e}");
            assert!(!is_refusal(&e));
            h.join().unwrap();
        }
    }

    #[test]
    fn the_indicator_history_is_bounded_and_keeps_the_newest() {
        let (port, h) = fake_core(|s, _| {
            send(s, &welcome()).unwrap();
            for _ in 0..MAX_REPORTED_INDICATORS * 2 {
                send(s, &Message::SetIndicator(Indicator::Idle)).unwrap();
            }
            send(s, &Message::SetIndicator(Indicator::Error)).unwrap();
        });
        let cfg = Config {
            wake_on_connect: false,
            ..cfg()
        };
        let r = run_bounded(port, Plain::default(), cfg, Duration::from_secs(30)).unwrap();
        assert_eq!(r.indicators.len(), MAX_REPORTED_INDICATORS);
        assert_eq!(r.indicators.last(), Some(&Indicator::Error));
        h.join().unwrap();
    }

    #[test]
    fn a_reject_reason_from_the_peer_is_plain_and_short() {
        for after_welcome in [false, true] {
            let reason = format!("\u{1b}[31mred\nline\r{}", "x".repeat(500));
            let (port, h) = fake_core(move |s, _| {
                if after_welcome {
                    send(s, &welcome()).unwrap();
                }
                send(s, &Message::Reject(Reject { reason })).unwrap();
            });
            let e = run(
                TcpStream::connect(("127.0.0.1", port)).unwrap(),
                &mut FileDevice::new(vec![]),
                &cfg(),
            )
            .unwrap_err();
            assert!(!e.chars().any(char::is_control), "{e:?}");
            assert!(e.contains("[31mred"), "text is kept: {e}");
            assert!(e.chars().count() < 300, "{}", e.chars().count());
            h.join().unwrap();
        }
    }

    #[test]
    fn the_token_is_not_in_debug_output() {
        let shown = format!("{:?}", cfg());
        assert!(!shown.contains("tok\""), "{shown}");
        assert!(shown.contains("<set>"), "{shown}");
    }

    #[test]
    fn a_rejection_is_reported_with_the_cores_reason() {
        let (port, h) = fake_core(|s, _| {
            send(
                s,
                &Message::Reject(Reject {
                    reason: "unauthorized".into(),
                }),
            )
            .unwrap();
        });
        let sock = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let e = run(sock, &mut FileDevice::new(vec![]), &cfg()).unwrap_err();
        assert!(e.contains("unauthorized"), "{e}");
        assert!(is_refusal(&e), "retrying with the same token cannot help");
        h.join().unwrap();
    }

    #[test]
    fn the_protocol_steps_are_followed() {
        let (port, h) = fake_core(|s, dec| {
            let mut closed = false;
            send(s, &welcome()).unwrap();
            assert!(matches!(next(s, dec, &mut closed), Message::Wake(_)));
            send(s, &Message::Ping).unwrap();
            send(s, &Message::StreamStart).unwrap();
            send(
                s,
                &Message::Unknown(clawdot_protocol::Frame::new("future.thing")),
            )
            .unwrap();
            let (mut pong, mut audio) = (false, 0);
            while !(pong && audio >= 3) {
                match next(s, dec, &mut closed) {
                    Message::Pong => pong = true,
                    Message::AudioChunk { payload, .. } => {
                        assert_eq!(payload.len(), 640, "20 ms of 16-bit audio");
                        audio += 1;
                    }
                    other => panic!("unexpected {other:?}"),
                }
            }
            send(s, &Message::StreamStop).unwrap();
            send(s, &Message::SetVolume { level: 35 }).unwrap();
            send(s, &Message::SetIndicator(Indicator::Speaking)).unwrap();
            send(s, &Message::AudioStart(AudioFormat::PCM_16K_MONO)).unwrap();
            for _ in 0..2 {
                send(
                    s,
                    &Message::AudioChunk {
                        format: AudioFormat::PCM_16K_MONO,
                        timestamp_ms: None,
                        payload: vec![1, 0, 2, 0],
                    },
                )
                .unwrap();
            }
            send(s, &Message::AudioStop).unwrap();
            // Some audio may still be in flight before the stop took effect; wait for playback-done.
            loop {
                if matches!(next(s, dec, &mut closed), Message::PlaybackDone { .. }) {
                    break;
                }
            }
        });
        let sock = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let mut dev = FileDevice::new(vec![500; 3200]);
        let report = run(sock, &mut dev, &cfg()).unwrap();
        assert_eq!(report.indicators, vec![Indicator::Speaking]);
        assert_eq!(report.samples_played, 4);
        assert_eq!(dev.reply, vec![1, 2, 1, 2]);
        assert_eq!(dev.volume, Some(35));
        h.join().unwrap();
    }

    #[test]
    fn muting_stops_the_microphone() {
        let (port, h) = fake_core(|s, dec| {
            let mut closed = false;
            send(s, &welcome()).unwrap();
            send(s, &Message::SetMute { muted: true }).unwrap();
            send(s, &Message::StreamStart).unwrap();
            let mut audio = false;
            let end = Instant::now() + Duration::from_millis(300);
            while Instant::now() < end {
                if let Some(Message::AudioChunk { .. }) = poll(s, dec, &mut closed).unwrap() {
                    audio = true;
                }
            }
            assert!(!audio, "a muted satellite sends no audio");
            send(
                s,
                &Message::Reject(Reject {
                    reason: "done".into(),
                }),
            )
            .unwrap();
        });
        let sock = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let mut dev = FileDevice::new(vec![500; 3200]);
        let cfg = Config {
            wake_on_connect: false,
            ..cfg()
        };
        let e = run(sock, &mut dev, &cfg).unwrap_err();
        assert!(e.contains("done"));
        h.join().unwrap();
    }

    #[test]
    fn stream_commands_reach_a_device_that_declared_support() {
        let (port, h) = fake_core(|s, dec| {
            let mut closed = false;
            send(s, &welcome()).unwrap();
            send(
                s,
                &Message::PlayUrl {
                    url: "http://r.example/jazz".into(),
                    title: "jazz".into(),
                },
            )
            .unwrap();
            send(
                s,
                &Message::MediaControl(clawdot_protocol::message::MediaAction::Pause),
            )
            .unwrap();
            send(
                s,
                &Message::MediaControl(clawdot_protocol::message::MediaAction::Resume),
            )
            .unwrap();
            send(s, &Message::AudioStart(AudioFormat::PCM_16K_MONO)).unwrap();
            send(s, &Message::AudioStop).unwrap();
            loop {
                if matches!(next(s, dec, &mut closed), Message::PlaybackDone { .. }) {
                    break;
                }
            }
        });
        let sock = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let mut dev = FileDevice::new(vec![]);
        dev.can_stream = true;
        dev.wait_for_streams = 1;
        run(
            sock,
            &mut dev,
            &Config {
                wake_on_connect: false,
                ..cfg()
            },
        )
        .unwrap();
        assert_eq!(
            dev.streams,
            vec![("http://r.example/jazz".to_owned(), "jazz".to_owned())]
        );
        assert_eq!(dev.pauses, vec![true, false]);
        h.join().unwrap();
    }

    #[test]
    fn a_device_without_stream_support_declares_so_and_ignores_the_commands() {
        let (port, h) = fake_core(|s, _| {
            send(s, &welcome()).unwrap();
            send(
                s,
                &Message::PlayUrl {
                    url: "http://r.example/jazz".into(),
                    title: "jazz".into(),
                },
            )
            .unwrap();
            send(
                s,
                &Message::Reject(Reject {
                    reason: "bye".into(),
                }),
            )
            .unwrap();
        });
        let sock = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let mut dev = FileDevice::new(vec![]);
        let _ = run(
            sock,
            &mut dev,
            &Config {
                wake_on_connect: false,
                ..cfg()
            },
        );
        assert!(dev.streams.is_empty());
        h.join().unwrap();
    }
}
