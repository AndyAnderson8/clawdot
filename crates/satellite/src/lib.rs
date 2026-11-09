//! ClawDot reference satellite: the smallest correct client of the satellite protocol (spec 0003).
//!
//! A port to real hardware implements [`Device`] (a microphone, a speaker, and some lights) and calls
//! [`run`]. Everything protocol-shaped (handshake, keepalive, streaming on request, playback bookkeeping,
//! ignoring unknown events) is here, in plain blocking `std` code, so it can be read in one sitting and
//! translated to C or Go. [`FileDevice`] is a development device that "speaks" samples from memory and
//! collects the reply.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use clawdot_protocol::message::{
    AudioFormat, Capabilities, Hello, Indicator, Message, PROTOCOL_VERSION, Wake,
};
use clawdot_protocol::{Decoder, Limits, encode};
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
}

/// Connection settings.
#[derive(Debug, Clone)]
pub struct Config {
    /// Stable device identifier.
    pub device_id: String,
    /// Shared token, if the core requires one.
    pub token: Option<String>,
    /// Send `wake` right after the handshake, as a device with a wake word or button would.
    pub wake_on_connect: bool,
    /// Treat the core as gone after this long without hearing anything (REQ-SAT-008). The core pings every
    /// 5 s, so silence this long means the connection is dead. [`DEAD_AFTER`] is the protocol default.
    pub dead_after: Duration,
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

fn send(sock: &mut TcpStream, m: &Message) -> Result<(), String> {
    let bytes = encode(&m.to_frame()).map_err(|e| e.to_string())?;
    sock.write_all(&bytes).map_err(|e| format!("write: {e}"))
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
    let mut dec = Decoder::new(Limits::default());
    let mut closed = false;
    send(
        &mut sock,
        &Message::Hello(Hello {
            protocol: PROTOCOL_VERSION,
            device_id: cfg.device_id.clone(),
            name: None,
            token: cfg.token.clone(),
            capabilities: Capabilities {
                wake_word: cfg.wake_on_connect,
                media_url: device.supports_streams(),
                ..Capabilities::default()
            },
        }),
    )?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match poll(&mut sock, &mut dec, &mut closed)? {
            Some(Message::Welcome(_)) => break,
            Some(Message::Reject(r)) => {
                return Err(format!("{REFUSED}: {}", r.reason));
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
    loop {
        if closed || device.finished() {
            return Ok(report);
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
            if let Some(pcm) = device.read_mic() {
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
            Message::StreamStart => streaming = true,
            Message::StreamStop => streaming = false,
            Message::SetIndicator(i) => {
                report.indicators.push(i);
                device.indicator(i);
            }
            Message::SetVolume { level } => device.set_volume(level),
            Message::SetMute { muted: m } => {
                muted = m;
                device.set_mute(m);
            }
            Message::StopPlayback => device.stop_playback(),
            Message::PlayUrl { url, title } if device.supports_streams() => {
                device.play_url(&url, &title)
            }
            Message::MediaControl(a) if device.supports_streams() => {
                device.pause_stream(a == clawdot_protocol::message::MediaAction::Pause);
            }
            Message::AudioStart(_) => device.begin_playback(),
            Message::AudioChunk { payload, .. } => {
                let pcm = samples(&payload);
                report.samples_played += pcm.len();
                device.play(&pcm);
            }
            Message::AudioStop => {
                device.end_playback();
                send(&mut sock, &Message::PlaybackDone { id: None })?;
            }
            Message::Reject(r) => return Err(format!("the core ended the session: {}", r.reason)),
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

    #[test]
    fn req_sat_008_backoff_doubles_and_is_capped() {
        let secs: Vec<u64> = (0..8).map(|n| backoff(n).as_secs()).collect();
        assert_eq!(secs, [1, 2, 4, 8, 16, 30, 30, 30]);
        assert_eq!(backoff(u32::MAX).as_secs(), 30, "no overflow");
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
