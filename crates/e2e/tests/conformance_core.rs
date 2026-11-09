//! Core-side conformance: a scripted fake satellite talks to the real core's satellite server (spec 0003).
//!
//! Each test is named for the requirement it covers. Speech providers are fakes, so these tests exercise the
//! protocol, not the models.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use clawdot_core::assistant::Effect;
use clawdot_core::inbox::endpoint::hash_token;
use clawdot_core::satellite::{Answer, Ask, Settings, serve};
use clawdot_core::voice::{Synthesizer, Transcriber};
use clawdot_e2e::FakeSatellite;
use clawdot_protocol::message::{
    AudioCodec, AudioFormat, Button, Capabilities, ErrorReport, Hello, Indicator, Message,
    PROTOCOL_VERSION, Version, Wake,
};
use clawdot_protocol::{Frame, message::kind};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::{mpsc as tmpsc, oneshot};

struct Ears;
impl Transcriber for Ears {
    async fn transcribe(&self, _pcm: &[i16]) -> Result<String, String> {
        Ok("what is on my plate".to_owned())
    }
}

struct Mouth;
impl Synthesizer for Mouth {
    async fn synthesize(&self, text: &str) -> Result<Vec<i16>, String> {
        Ok(vec![9; text.len() * 50])
    }
}

/// A running core; dropping it stops the server.
struct Core {
    addr: SocketAddr,
    _stop: oneshot::Sender<()>,
}

fn start(settings: Settings) -> Core {
    let (addr_tx, addr_rx) = mpsc::channel();
    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            addr_tx.send(listener.local_addr().unwrap()).unwrap();
            let (asks, mut rx) = tmpsc::channel::<Ask>(4);
            tokio::spawn(async move {
                while let Some(ask) = rx.recv().await {
                    let _ = ask.reply.send(Answer {
                        say: "Nothing is waiting.".to_owned(),
                        effects: vec![Effect::VolumeSet(40)],
                    });
                }
            });
            let _ = serve(
                listener,
                Arc::new(Ears),
                Arc::new(Mouth),
                asks,
                settings,
                async {
                    let _ = stop_rx.await;
                },
            )
            .await;
        });
    });
    Core {
        addr: addr_rx.recv().unwrap(),
        _stop: stop_tx,
    }
}

fn open_core() -> Core {
    start(Settings::default())
}

fn is_ping(m: &Message) -> bool {
    matches!(m, Message::Ping)
}

/// Twenty-millisecond frames of a short utterance followed by silence, as a microphone would deliver it.
fn mic_frames() -> Vec<Vec<u8>> {
    let mut pcm = vec![0i16; 3200];
    pcm.extend((0..9600).map(|i| (3000.0 * (f64::from(i) * 0.35).sin()) as i16));
    pcm.extend(std::iter::repeat_n(0i16, 24_000));
    pcm.chunks(320)
        .map(|c| c.iter().flat_map(|s| s.to_le_bytes()).collect())
        .collect()
}

fn chunk(payload: Vec<u8>) -> Message {
    Message::AudioChunk {
        format: AudioFormat::PCM_16K_MONO,
        timestamp_ms: None,
        payload,
    }
}

// ---- handshake -------------------------------------------------------------------------------------------

#[test]
fn req_sat_002_hello_is_answered_with_welcome() {
    let core = open_core();
    let mut sat = FakeSatellite::connect(core.addr);
    sat.send(&FakeSatellite::hello("kitchen", None));
    let Message::Welcome(w) = sat.recv() else {
        panic!("expected welcome");
    };
    assert_eq!(w.protocol, PROTOCOL_VERSION);
    assert!(!w.session_id.is_empty());
    assert_eq!(w.audio_out, AudioCodec::Pcm);
}

#[test]
fn req_sat_018_the_first_message_must_be_hello() {
    let core = open_core();
    let mut sat = FakeSatellite::connect(core.addr);
    sat.send(&Message::Ping);
    let Message::Reject(r) = sat.recv() else {
        panic!("expected reject");
    };
    assert!(r.reason.contains("hello"), "{}", r.reason);
    sat.expect_closed();
}

#[test]
fn req_sat_009_an_incompatible_version_is_rejected_with_both_versions() {
    let core = open_core();
    let mut sat = FakeSatellite::connect(core.addr);
    sat.send(&Message::Hello(Hello {
        protocol: Version { major: 7, minor: 3 },
        device_id: "future".into(),
        name: None,
        token: None,
        capabilities: Capabilities::default(),
    }));
    let Message::Reject(r) = sat.recv() else {
        panic!("expected reject");
    };
    assert!(
        r.reason.contains("7.3"),
        "names the satellite's version: {}",
        r.reason
    );
    assert!(
        r.reason.contains(&format!(
            "{}.{}",
            PROTOCOL_VERSION.major, PROTOCOL_VERSION.minor
        )),
        "names the core's version: {}",
        r.reason
    );
    sat.expect_closed();
}

#[test]
fn req_sat_010_a_token_is_required_when_the_core_has_one() {
    let core = start(Settings {
        token_hashes: vec![hash_token("sat-token")],
        ..Settings::default()
    });
    for token in [None, Some("wrong")] {
        let mut sat = FakeSatellite::connect(core.addr);
        sat.send(&FakeSatellite::hello("k", token));
        let Message::Reject(r) = sat.recv() else {
            panic!("expected reject");
        };
        assert_eq!(r.reason, "unauthorized", "and nothing else (REQ-SAT-018)");
        sat.expect_closed();
    }
    let _ = FakeSatellite::connect_and_welcome(core.addr, Some("sat-token"));
}

#[test]
fn req_sat_010_every_configured_token_is_accepted() {
    // Per-satellite tokens today: give each device its own entry in the list.
    let core = start(Settings {
        token_hashes: vec![hash_token("one"), hash_token("two")],
        ..Settings::default()
    });
    let _ = FakeSatellite::connect_and_welcome(core.addr, Some("one"));
    let _ = FakeSatellite::connect_and_welcome(core.addr, Some("two"));
}

#[test]
fn req_sat_018_a_connection_that_never_says_hello_is_dropped() {
    let core = start(Settings {
        hello_timeout: Duration::from_millis(200),
        ..Settings::default()
    });
    let mut sat = FakeSatellite::connect(core.addr);
    sat.expect_closed();
}

#[test]
fn req_sat_013_a_satellite_with_no_capabilities_can_hold_a_conversation() {
    let core = open_core();
    let mut sat = FakeSatellite::connect_and_welcome(core.addr, None);
    // No wake word, no LEDs: a button press stands in for the wake word.
    sat.send(&Message::Button(Button {
        name: "action".into(),
        long: false,
    }));
    sat.expect("indicator listening", |m| {
        matches!(m, Message::SetIndicator(Indicator::Listening))
    });
    sat.expect("stream-start", |m| matches!(m, Message::StreamStart));
}

#[test]
fn req_sat_005_the_core_answers_pcm_even_when_opus_is_offered() {
    // Opus downlink is not implemented yet (spec 0003 section 10, question 6). When it lands this test should
    // change to expect Opus for a satellite that offers it.
    let core = open_core();
    let mut sat = FakeSatellite::connect(core.addr);
    sat.send(&Message::Hello(Hello {
        protocol: PROTOCOL_VERSION,
        device_id: "opus".into(),
        name: None,
        token: None,
        capabilities: Capabilities {
            audio_out: vec![AudioCodec::Pcm, AudioCodec::Opus],
            ..Capabilities::default()
        },
    }));
    let Message::Welcome(w) = sat.recv() else {
        panic!("expected welcome");
    };
    assert_eq!(w.audio_out, AudioCodec::Pcm);
}

// ---- a spoken turn ---------------------------------------------------------------------------------------

#[test]
fn req_sat_004_to_007_a_spoken_turn_follows_the_documented_sequence() {
    let core = open_core();
    let mut sat = FakeSatellite::connect_and_welcome(core.addr, None);
    sat.send(&Message::Wake(Wake {
        name: "hey".into(),
        timestamp_ms: None,
    }));
    sat.expect("indicator listening", |m| {
        matches!(m, Message::SetIndicator(Indicator::Listening))
    });
    sat.expect("stream-start", |m| matches!(m, Message::StreamStart));

    for frame in mic_frames() {
        assert_eq!(
            frame.len(),
            640,
            "20 ms of 16 kHz 16-bit mono (REQ-SAT-004)"
        );
        sat.send(&chunk(frame));
    }
    sat.expect("stream-stop", |m| matches!(m, Message::StreamStop));
    sat.expect("indicator thinking", |m| {
        matches!(m, Message::SetIndicator(Indicator::Thinking))
    });
    sat.expect("indicator speaking", |m| {
        matches!(m, Message::SetIndicator(Indicator::Speaking))
    });
    let start = sat.expect("audio-start", |m| matches!(m, Message::AudioStart(_)));
    assert_eq!(start, Message::AudioStart(AudioFormat::PCM_16K_MONO));
    let mut samples = 0;
    loop {
        match sat.recv() {
            Message::AudioChunk {
                format, payload, ..
            } => {
                assert_eq!(format, AudioFormat::PCM_16K_MONO);
                samples += payload.len() / 2;
            }
            Message::AudioStop => break,
            Message::SetVolume { .. } => {}
            other => panic!("unexpected {other:?} in the reply audio"),
        }
    }
    assert_eq!(samples, "Nothing is waiting.".len() * 50);
    sat.send(&Message::PlaybackDone { id: None });
    // The follow-up window opens a new listening turn without a wake word.
    sat.expect("follow-up stream-start", |m| {
        matches!(m, Message::StreamStart)
    });
}

#[test]
fn req_sat_004_audio_in_another_format_is_reported_once_and_dropped() {
    let core = open_core();
    let mut sat = FakeSatellite::connect_and_welcome(core.addr, None);
    sat.send(&Message::Wake(Wake {
        name: "hey".into(),
        timestamp_ms: None,
    }));
    sat.expect("stream-start", |m| matches!(m, Message::StreamStart));
    let wrong = AudioFormat {
        rate: 48_000,
        width: 2,
        channels: 2,
    };
    for _ in 0..3 {
        sat.send(&Message::AudioChunk {
            format: wrong,
            timestamp_ms: None,
            payload: vec![0; 640],
        });
    }
    let Message::Error(ErrorReport { code, .. }) =
        sat.expect("error", |m| matches!(m, Message::Error(_)))
    else {
        unreachable!()
    };
    assert_eq!(code.as_deref(), Some("bad-audio-format"));
    sat.expect_none(Duration::from_millis(300), |m| {
        matches!(m, Message::Error(_))
    });
}

// ---- mute ------------------------------------------------------------------------------------------------

#[test]
fn req_sat_006_a_muted_satellite_is_not_woken() {
    let core = open_core();
    let mut sat = FakeSatellite::connect_and_welcome(core.addr, None);
    sat.send(&Message::MuteChanged { muted: true });
    sat.expect("indicator muted", |m| {
        matches!(m, Message::SetIndicator(Indicator::Muted))
    });
    sat.send(&Message::Wake(Wake {
        name: "hey".into(),
        timestamp_ms: None,
    }));
    sat.expect_none(Duration::from_millis(300), |m| {
        matches!(m, Message::StreamStart)
    });
    sat.send(&Message::MuteChanged { muted: false });
    sat.send(&Message::Wake(Wake {
        name: "hey".into(),
        timestamp_ms: None,
    }));
    sat.expect("stream-start after unmute", |m| {
        matches!(m, Message::StreamStart)
    });
}

#[test]
fn req_sat_007_the_mute_button_toggles_and_the_core_says_so() {
    let core = open_core();
    let mut sat = FakeSatellite::connect_and_welcome(core.addr, None);
    let press = Message::Button(Button {
        name: "mute".into(),
        long: false,
    });
    sat.send(&press);
    sat.expect("set-mute true", |m| *m == Message::SetMute { muted: true });
    sat.send(&press);
    sat.expect("set-mute false", |m| {
        *m == Message::SetMute { muted: false }
    });
}

// ---- keepalive, unknown input, limits ---------------------------------------------------------------------

#[test]
fn req_sat_008_the_core_pings_and_answers_pings() {
    let core = start(Settings {
        keepalive: Duration::from_millis(100),
        ..Settings::default()
    });
    let mut sat = FakeSatellite::connect_and_welcome(core.addr, None);
    sat.expect("a ping from the core", is_ping);
    sat.send(&Message::Pong);
    sat.send(&Message::Ping);
    sat.expect("pong", |m| matches!(m, Message::Pong));
}

#[test]
fn req_sat_008_a_silent_satellite_is_dropped() {
    let core = start(Settings {
        keepalive: Duration::from_millis(50),
        dead_after: Duration::from_millis(400),
        ..Settings::default()
    });
    let mut sat = FakeSatellite::connect_and_welcome(core.addr, None);
    sat.expect_closed();
}

#[test]
fn req_sat_008_a_satellite_that_answers_pings_is_kept() {
    let core = start(Settings {
        keepalive: Duration::from_millis(50),
        dead_after: Duration::from_millis(400),
        ..Settings::default()
    });
    let mut sat = FakeSatellite::connect_and_welcome(core.addr, None);
    for _ in 0..4 {
        sat.expect("ping", is_ping);
        sat.send(&Message::Pong);
    }
    assert!(!sat.closed());
}

#[test]
fn req_sat_009_unknown_events_and_extra_fields_are_ignored() {
    let core = open_core();
    let mut sat = FakeSatellite::connect_and_welcome(core.addr, None);
    sat.send_frame(&Frame::new("clawdot.from-the-future"));
    sat.send_raw(b"{\"type\":\"clawdot.mute-changed\",\"data\":{\"muted\":false,\"extra\":[1,2]},\"more\":1}\n");
    sat.send(&Message::Ping);
    sat.expect("pong, so the session survived", |m| {
        matches!(m, Message::Pong)
    });
}

#[test]
fn req_sat_017_a_framing_violation_closes_the_connection() {
    for garbage in [
        &b"this is not json\n"[..],
        b"[1,2,3]\n",
        b"{\"data\":{}}\n",
        b"{\"type\":\"audio-chunk\",\"payload_length\":99999999}\n",
        b"{\"type\":\"x\",\"payload_length\":-1}\n",
    ] {
        let core = open_core();
        let mut sat = FakeSatellite::connect_and_welcome(core.addr, None);
        sat.send_raw(garbage);
        sat.expect_closed();
    }
}

#[test]
fn req_sat_017_an_endless_header_line_closes_the_connection() {
    let core = open_core();
    let mut sat = FakeSatellite::connect_and_welcome(core.addr, None);
    sat.send_raw(&vec![b'a'; 64 * 1024]);
    sat.expect_closed();
}

#[test]
fn req_sat_017_a_known_event_with_bad_fields_ends_the_session() {
    let core = open_core();
    let mut sat = FakeSatellite::connect_and_welcome(core.addr, None);
    sat.send_raw(format!("{{\"type\":\"{}\"}}\n", kind::MUTE_CHANGED).as_bytes());
    sat.expect_closed();
}

#[test]
fn the_core_serves_up_to_its_session_limit_and_closes_the_rest() {
    let core = start(Settings {
        max_sessions: 1,
        ..Settings::default()
    });
    let mut first = FakeSatellite::connect_and_welcome(core.addr, None);
    let mut second = FakeSatellite::connect(core.addr);
    second.send(&FakeSatellite::hello("second", None));
    second.expect_closed();
    first.send(&Message::Ping);
    first.expect("pong on the first session", |m| matches!(m, Message::Pong));
}
