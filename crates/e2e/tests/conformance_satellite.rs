//! Satellite-side conformance: the scripted fake core drives the reference satellite client (spec 0003).
//!
//! The same scenarios can be pointed at a port by replacing `spawn_satellite` with the port's own launcher and
//! connecting it to `FakeCore::addr()`. Each test names the requirement it covers.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use clawdot_e2e::{FakeCore, Peer};
use clawdot_protocol::message::{
    AudioCodec, AudioFormat, Indicator, MediaAction, Message, PROTOCOL_VERSION, Reject, Welcome,
};
use clawdot_protocol::{Frame, message::kind};
use clawdot_satellite::{Config, DEAD_AFTER, FileDevice, Report, is_refusal, run};
use std::net::{SocketAddr, TcpStream};
use std::thread::{self, JoinHandle};
use std::time::Duration;

type Finished = (Result<Report, String>, FileDevice);

fn config(wake: bool) -> Config {
    Config {
        device_id: "conformance".into(),
        token: Some("tok".into()),
        wake_on_connect: wake,
        dead_after: DEAD_AFTER,
    }
}

fn spawn_satellite(addr: SocketAddr, mut dev: FileDevice, cfg: Config) -> JoinHandle<Finished> {
    thread::spawn(move || {
        let sock = TcpStream::connect(addr).unwrap();
        let result = run(sock, &mut dev, &cfg);
        (result, dev)
    })
}

/// Wait for a satellite whose core hung up; a clean end is expected.
fn finish(h: JoinHandle<Finished>) {
    let (result, _) = h.join().unwrap();
    result.unwrap();
}

fn welcome() -> Message {
    Message::Welcome(Welcome {
        protocol: PROTOCOL_VERSION,
        session_id: "s".into(),
        audio_out: AudioCodec::Pcm,
    })
}

/// Start a satellite with a short utterance on its microphone and complete the handshake.
fn welcomed(wake: bool) -> (Peer, JoinHandle<Finished>) {
    let core = FakeCore::listen();
    let handle = spawn_satellite(core.addr(), FileDevice::new(vec![500; 3200]), config(wake));
    let (mut peer, _hello) = core.accept();
    peer.send(&welcome());
    (peer, handle)
}

fn is_audio(m: &Message) -> bool {
    matches!(m, Message::AudioChunk { .. })
}

// ---- handshake -------------------------------------------------------------------------------------------

#[test]
fn req_sat_002_the_hello_carries_version_id_and_token() {
    let core = FakeCore::listen();
    let h = spawn_satellite(core.addr(), FileDevice::new(vec![]), config(false));
    let (mut peer, hello) = core.accept();
    assert_eq!(hello.protocol, PROTOCOL_VERSION);
    assert_eq!(hello.device_id, "conformance");
    assert_eq!(hello.token.as_deref(), Some("tok"));
    peer.send(&welcome());
    drop(peer);
    finish(h);
}

#[test]
fn req_sat_013_a_plain_device_declares_no_optional_capabilities() {
    let core = FakeCore::listen();
    let h = spawn_satellite(core.addr(), FileDevice::new(vec![]), config(false));
    let (mut peer, hello) = core.accept();
    peer.send(&welcome());
    let c = &hello.capabilities;
    assert!(!c.wake_word && !c.leds && !c.buttons && !c.display && !c.hardware_mute);
    assert!(!c.aec && !c.noise_suppression && !c.vad && !c.beamforming);
    assert!(!c.media_url, "no player, so no stream commands");
    assert!(c.audio_out.is_empty(), "empty means PCM only");
    drop(peer);
    finish(h);
}

#[test]
fn req_sat_003_a_device_that_can_play_streams_declares_media_url() {
    let core = FakeCore::listen();
    let mut dev = FileDevice::new(vec![]);
    dev.can_stream = true;
    let h = spawn_satellite(core.addr(), dev, config(true));
    let (mut peer, hello) = core.accept();
    peer.send(&welcome());
    assert!(hello.capabilities.media_url);
    assert!(hello.capabilities.wake_word);
    drop(peer);
    finish(h);
}

#[test]
fn req_sat_002_nothing_is_sent_before_welcome() {
    let core = FakeCore::listen();
    let h = spawn_satellite(core.addr(), FileDevice::new(vec![500; 3200]), config(true));
    let (mut peer, _) = core.accept();
    peer.expect_none(Duration::from_millis(300), |_| true);
    peer.send(&welcome());
    peer.expect("detection after welcome", |m| matches!(m, Message::Wake(_)));
    drop(peer);
    finish(h);
}

#[test]
fn req_sat_018_a_rejection_ends_the_session_with_the_reason_and_is_not_retried() {
    let core = FakeCore::listen();
    let h = spawn_satellite(core.addr(), FileDevice::new(vec![]), config(false));
    let (mut peer, _) = core.accept();
    peer.send(&Message::Reject(Reject {
        reason: "unauthorized".into(),
    }));
    let (result, _) = h.join().unwrap();
    let e = result.unwrap_err();
    assert!(e.contains("unauthorized"), "{e}");
    assert!(is_refusal(&e));
}

// ---- microphone ------------------------------------------------------------------------------------------

#[test]
fn req_sat_004_microphone_audio_flows_only_between_stream_start_and_stop() {
    let (mut peer, h) = welcomed(false);
    peer.expect_none(Duration::from_millis(300), is_audio);
    peer.send(&Message::StreamStart);
    let Message::AudioChunk {
        format, payload, ..
    } = peer.expect("microphone audio", is_audio)
    else {
        unreachable!()
    };
    assert_eq!(format, AudioFormat::PCM_16K_MONO);
    assert_eq!(payload.len(), 640, "20 ms frames");
    peer.send(&Message::StreamStop);
    // Anything already in flight may still arrive; after that, silence.
    peer.expect_none(Duration::from_millis(100), |_| false);
    peer.expect_none(Duration::from_millis(300), is_audio);
    drop(peer);
    finish(h);
}

#[test]
fn req_sat_007_a_muted_satellite_sends_no_audio_even_when_streaming() {
    let (mut peer, h) = welcomed(false);
    peer.send(&Message::SetMute { muted: true });
    peer.send(&Message::StreamStart);
    peer.expect_none(Duration::from_millis(400), is_audio);
    peer.send(&Message::SetMute { muted: false });
    peer.expect("audio after unmute", is_audio);
    drop(peer);
    finish(h);
}

#[test]
fn req_sat_006_wake_is_announced_after_the_handshake() {
    let (mut peer, h) = welcomed(true);
    let Message::Wake(w) = peer.expect("detection", |m| matches!(m, Message::Wake(_))) else {
        unreachable!()
    };
    assert!(!w.name.is_empty());
    drop(peer);
    finish(h);
}

// ---- playback and commands --------------------------------------------------------------------------------

#[test]
fn req_sat_006_playback_done_follows_a_complete_reply() {
    let (mut peer, h) = welcomed(false);
    peer.send(&Message::SetIndicator(Indicator::Speaking));
    peer.send(&Message::AudioStart(AudioFormat::PCM_16K_MONO));
    for _ in 0..3 {
        peer.send(&Message::AudioChunk {
            format: AudioFormat::PCM_16K_MONO,
            timestamp_ms: None,
            payload: vec![7, 0, 8, 0],
        });
    }
    peer.expect_none(Duration::from_millis(200), |m| {
        matches!(m, Message::PlaybackDone { .. })
    });
    peer.send(&Message::AudioStop);
    peer.expect("playback-done", |m| {
        matches!(m, Message::PlaybackDone { .. })
    });
    drop(peer);
    let (result, dev) = h.join().unwrap();
    assert_eq!(result.unwrap().indicators, vec![Indicator::Speaking]);
    assert_eq!(dev.reply, vec![7, 8, 7, 8, 7, 8]);
}

#[test]
fn req_sat_007_volume_reaches_the_device() {
    let (mut peer, h) = welcomed(false);
    peer.send(&Message::SetVolume { level: 0 });
    peer.send(&Message::SetVolume { level: 100 });
    peer.send(&Message::SetVolume { level: 35 });
    peer.send(&Message::Ping);
    peer.expect("pong", |m| matches!(m, Message::Pong));
    drop(peer);
    let (_, dev) = h.join().unwrap();
    assert_eq!(dev.volume, Some(35));
}

#[test]
fn req_sat_016_media_commands_are_ignored_by_a_device_that_did_not_declare_them() {
    let (mut peer, h) = welcomed(false);
    peer.send(&Message::PlayUrl {
        url: "http://r.example/jazz".into(),
        title: "Jazz".into(),
    });
    peer.send(&Message::MediaControl(MediaAction::Pause));
    peer.send(&Message::Ping);
    peer.expect("pong, so the session survived", |m| {
        matches!(m, Message::Pong)
    });
    drop(peer);
    let (result, dev) = h.join().unwrap();
    result.unwrap();
    assert!(dev.streams.is_empty() && dev.pauses.is_empty());
}

// ---- keepalive, unknown input, limits ---------------------------------------------------------------------

#[test]
fn req_sat_008_pings_are_answered() {
    let (mut peer, h) = welcomed(false);
    peer.send(&Message::Ping);
    peer.expect("pong", |m| matches!(m, Message::Pong));
    drop(peer);
    finish(h);
}

#[test]
fn req_sat_008_a_satellite_gives_up_on_a_silent_core_after_pinging_it() {
    let core = FakeCore::listen();
    let cfg = Config {
        dead_after: Duration::from_millis(400),
        ..config(false)
    };
    let h = spawn_satellite(core.addr(), FileDevice::new(vec![]), cfg);
    let (mut peer, _) = core.accept();
    peer.send(&welcome());
    peer.expect("a ping from the satellite", |m| matches!(m, Message::Ping));
    peer.expect_closed();
    let (result, _) = h.join().unwrap();
    let e = result.unwrap_err();
    assert!(!is_refusal(&e), "a dead link is retried: {e}");
}

#[test]
fn req_sat_009_unknown_events_and_extra_fields_are_ignored() {
    let (mut peer, h) = welcomed(false);
    peer.send_frame(&Frame::new("clawdot.display.state"));
    peer.send_raw(
        b"{\"type\":\"clawdot.set-volume\",\"data\":{\"level\":20,\"future\":true},\"x\":1}\n",
    );
    peer.send(&Message::Ping);
    peer.expect("pong", |m| matches!(m, Message::Pong));
    drop(peer);
    let (result, dev) = h.join().unwrap();
    result.unwrap();
    assert_eq!(dev.volume, Some(20));
}

#[test]
fn req_sat_017_a_framing_violation_ends_the_session() {
    for garbage in [
        &b"not json at all\n"[..],
        b"{\"type\":\"audio-chunk\",\"payload_length\":99999999}\n",
        b"{\"data\":{}}\n",
    ] {
        let (mut peer, h) = welcomed(false);
        peer.send_raw(garbage);
        let (result, _) = h.join().unwrap();
        assert!(result.is_err(), "the satellite must not keep talking");
        peer.expect_closed();
    }
}

#[test]
fn req_sat_017_a_known_event_with_bad_fields_ends_the_session() {
    let (mut peer, h) = welcomed(false);
    peer.send_raw(
        format!(
            "{{\"type\":\"{}\",\"data\":{{\"level\":101}}}}\n",
            kind::SET_VOLUME
        )
        .as_bytes(),
    );
    let (result, _) = h.join().unwrap();
    assert!(result.is_err());
}
