use super::*;
use crate::assistant::{Effect, MediaCmd};
use crate::inbox::endpoint::hash_token;
use crate::vad::tests::{silence, tone};
use crate::voice::{Synthesizer, Transcriber};
use clawdot_protocol::frame::{Decoder, Limits, encode};
use clawdot_protocol::message::{
    AudioCodec, AudioFormat, Indicator, MediaAction, Message, PROTOCOL_VERSION,
};
use clawdot_protocol::message::{Capabilities, Hello, Version, Wake};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};
use tokio::io::DuplexStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{Semaphore, mpsc, oneshot};
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
async fn start_music(media_url: bool, heard: &'static str, respond: fn(&str) -> Answer) -> Peer {
    start_music_with(Mouth, media_url, heard, respond).await
}

/// Like `start_music`, with the given voice.
async fn start_music_with<T>(
    mouth: T,
    media_url: bool,
    heard: &'static str,
    respond: fn(&str) -> Answer,
) -> Peer
where
    T: Synthesizer + Send + Sync + 'static,
{
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
        Arc::new(mouth),
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

// ----- slow speech synthesis never stalls the session (REQ-SAT-008, REQ-VOI-020) -----

/// A voice that takes a while for every sentence.
struct SlowMouth(Duration);
impl Synthesizer for SlowMouth {
    async fn synthesize(&self, text: &str) -> Result<Vec<i16>, String> {
        tokio::time::sleep(self.0).await;
        Ok(vec![7; text.len() * 100])
    }
}

/// A registered session whose voice takes `delay` per sentence and whose assistant runs `handler`.
async fn start_slow<T>(
    ears: T,
    delay: Duration,
    handler: impl Fn(Ask) + Send + 'static,
) -> (Peer, Announcer)
where
    T: Transcriber + Send + Sync + 'static,
{
    start_slow_with(fast(), ears, delay, handler).await
}

/// Like `start_slow`, with the given settings.
async fn start_slow_with<T>(
    s: Settings,
    ears: T,
    delay: Duration,
    handler: impl Fn(Ask) + Send + 'static,
) -> (Peer, Announcer)
where
    T: Transcriber + Send + Sync + 'static,
{
    let ann = s.announcer.clone();
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
        Arc::new(SlowMouth(delay)),
        tx,
        s,
    ));
    let mut p = Peer {
        io: near,
        dec: Decoder::new(Limits::default()),
    };
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

/// Everything the core sends for `ms` milliseconds, with its pings answered.
async fn collect_for(p: &mut Peer, ms: u64) -> Vec<Message> {
    let end = Instant::now() + Duration::from_millis(ms);
    let mut seen = Vec::new();
    while Instant::now() < end {
        let left = end.saturating_duration_since(Instant::now());
        match timeout(left, next_message(&mut p.io, &mut p.dec)).await {
            Err(_) => break,
            Ok(Ok(Some(Message::Ping))) => p.send(&Message::Pong).await,
            Ok(Ok(Some(m))) => seen.push(m),
            Ok(_) => break,
        }
    }
    seen
}

/// An announcement is being synthesized (1.5 s, far longer than the 400 ms dead-peer timeout of these tests): a
/// ping from the satellite is still answered at once, so neither side concludes the other is gone.
#[tokio::test]
async fn a_ping_is_answered_while_an_announcement_is_being_synthesized() {
    let (mut p, ann) = start_slow(Ears("x"), Duration::from_millis(1500), |_: Ask| {}).await;
    let said = tokio::spawn(async move { ann.announce("Claude finished.").await });
    tokio::time::sleep(Duration::from_millis(100)).await;
    p.send(&Message::Ping).await;
    let answered = timeout(Duration::from_millis(400), async {
        loop {
            match p.recv().await {
                Message::Pong => return,
                Message::Ping => p.send(&Message::Pong).await,
                other => panic!("unexpected {other:?}"),
            }
        }
    })
    .await;
    assert!(
        answered.is_ok(),
        "no pong while the announcement was synthesized"
    );
    // The announcement still plays once it is made.
    let seen = until(&mut p, |m| matches!(m, Message::AudioStop)).await;
    assert_eq!(samples(&seen), "Claude finished.".len() * 100, "{seen:?}");
    assert!(said.await.unwrap());
}

/// Muting while an announcement is being synthesized takes effect at once, and the announcement is not played
/// into the muted room.
#[tokio::test]
async fn muting_while_an_announcement_is_synthesized_is_handled_and_cancels_it() {
    let (mut p, ann) = start_slow(Ears("x"), Duration::from_millis(1000), |_: Ask| {}).await;
    let said = tokio::spawn(async move { ann.announce("Claude finished.").await });
    tokio::time::sleep(Duration::from_millis(100)).await;
    p.send(&Message::Button(clawdot_protocol::message::Button {
        name: "mute".into(),
        long: false,
    }))
    .await;
    let early = collect_for(&mut p, 300).await;
    assert!(
        early.contains(&Message::SetMute { muted: true }),
        "the mute was not handled while synthesis ran: {early:?}"
    );
    let rest = collect_for(&mut p, 1500).await;
    assert!(
        !rest.iter().any(|m| matches!(m, Message::AudioStart(_))),
        "an announcement played into a muted room: {rest:?}"
    );
    assert!(!said.await.unwrap());
}

/// Stopping the turn while a progress phrase is being synthesized reaches the turn at once, and the phrase is
/// not played afterwards.
#[tokio::test]
async fn stopping_a_turn_while_a_progress_phrase_is_synthesized_is_not_delayed() {
    let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = Arc::clone(&cancelled);
    let (mut p, _ann) = start_slow(
        Ears("check the runs"),
        Duration::from_millis(800),
        move |ask: Ask| {
            let flag = Arc::clone(&flag);
            tokio::spawn(async move {
                if let Some(tx) = &ask.progress {
                    let _ = tx.send("Checking the runs.".into());
                }
                let stopped = timeout(Duration::from_secs(3), ask.cancel.cancelled())
                    .await
                    .is_ok();
                flag.store(stopped, Ordering::SeqCst);
                let _ = ask.reply.send(Answer {
                    say: STOPPED.into(),
                    ..Answer::default()
                });
            });
        },
    )
    .await;
    ask_and_wait_for_thinking(&mut p).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    p.send(&Message::Button(clawdot_protocol::message::Button {
        name: "action".into(),
        long: false,
    }))
    .await;
    collect_for(&mut p, 300).await;
    assert!(
        cancelled.load(Ordering::SeqCst),
        "the turn was not stopped while the progress phrase was synthesized"
    );
    let seen = until(&mut p, |m| matches!(m, Message::AudioStop)).await;
    assert_eq!(
        samples(&seen),
        STOPPED.len() * 100,
        "the progress phrase played after the stop: {seen:?}"
    );
}

/// A slow-voiced session with a realtime session open: the wake word starts it, then the test ends it with
/// `speak` as the closing line. Returns the speaker's end.
#[cfg(feature = "realtime")]
async fn start_realtime_closing(delay: Duration, speak: &str) -> Peer {
    use crate::rtvoice::{Done, Realtime, RtSettings};
    let rt = Realtime::with_env(
        RtSettings {
            enabled: true,
            url: Some("wss://example.invalid/v1".into()),
            model: Some("m-1".into()),
            ..RtSettings::default()
        },
        Box::new(|_| Some("sk-test".into())),
    );
    let mut jobs = rt.attach();
    let settings = Settings {
        realtime: Some(rt),
        ..fast()
    };
    let (mut p, _ann) = start_slow_with(settings, Ears("x"), delay, |_: Ask| {}).await;
    p.send(&wake()).await;
    let job = timeout(Duration::from_secs(5), jobs.recv())
        .await
        .expect("no realtime job")
        .expect("job channel closed");
    let speak = speak.to_owned();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let _ = job.done.send(Done {
            speak: Some(speak),
            failed: false,
            effects: Vec::new(),
        });
    });
    p
}

/// The closing line of a realtime session is being synthesized (1.5 s, far longer than the 400 ms dead-peer
/// timeout of these tests): a ping is still answered at once, and the line plays once it is made.
#[cfg(feature = "realtime")]
#[tokio::test]
async fn a_ping_is_answered_while_the_closing_line_of_a_realtime_session_is_synthesized() {
    let mut p = start_realtime_closing(Duration::from_millis(1500), "Goodbye.").await;
    // The core stops the stream when the session ends; synthesis of the closing line then runs.
    until(&mut p, |m| matches!(m, Message::StreamStop)).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    p.send(&Message::Ping).await;
    let answered = timeout(Duration::from_millis(400), async {
        loop {
            match p.recv().await {
                Message::Pong => return,
                Message::Ping => p.send(&Message::Pong).await,
                _ => {}
            }
        }
    })
    .await;
    assert!(
        answered.is_ok(),
        "no pong while the closing line was synthesized"
    );
    let seen = until(&mut p, |m| matches!(m, Message::AudioStop)).await;
    assert_eq!(samples(&seen), "Goodbye.".len() * 100, "{seen:?}");
}

/// Muting while the closing line of a realtime session is being synthesized is handled at once.
#[cfg(feature = "realtime")]
#[tokio::test]
async fn muting_while_the_closing_line_of_a_realtime_session_is_synthesized_is_handled() {
    let mut p = start_realtime_closing(Duration::from_millis(1500), "Goodbye.").await;
    until(&mut p, |m| matches!(m, Message::StreamStop)).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    p.send(&Message::Button(clawdot_protocol::message::Button {
        name: "mute".into(),
        long: false,
    }))
    .await;
    let early = collect_for(&mut p, 300).await;
    assert!(
        early.contains(&Message::SetMute { muted: true }),
        "the mute was not handled while synthesis ran: {early:?}"
    );
}

// ----- a stream command never outlives a reply that could not be spoken (REQ-MUS-004) -----

/// A voice that is down.
struct BrokenMouth;
impl Synthesizer for BrokenMouth {
    async fn synthesize(&self, _: &str) -> Result<Vec<i16>, String> {
        Err("the speech service is down".into())
    }
}

/// The reply to "play jazz" cannot be spoken, so nothing is playing that the stream must wait for: the stream
/// starts at once, instead of waiting in the queue to start after some later, unrelated speech.
#[tokio::test]
async fn a_stream_command_is_not_held_back_by_a_reply_that_could_not_be_spoken() {
    let mut p = start_music_with(BrokenMouth, true, "play jazz", play_jazz).await;
    speak(&mut p).await;
    let seen = timeout(
        Duration::from_secs(2),
        until(&mut p, |m| matches!(m, Message::PlayUrl { .. })),
    )
    .await
    .expect("the stream never started: it is stuck in the queue");
    assert_eq!(seen.last(), Some(&jazz_url()));
    assert_eq!(samples(&seen), 0, "{seen:?}");
}

// ----- a satellite that stops reading cannot wedge the session (REQ-SAT-010) -----

/// The satellite says hello, reads the welcome, and then never reads again. Its receive buffer fills, the core's
/// next write waits for room that never comes, and the keepalive tick that would notice a dead peer cannot run
/// while the loop is stuck inside the write. The write must give up after the dead-peer time and end the session.
#[tokio::test]
async fn a_satellite_that_stops_reading_ends_the_session_instead_of_wedging_it() {
    let (near, far) = tokio::io::duplex(8);
    let (tx, _rx) = mpsc::channel::<Ask>(4);
    let session = tokio::spawn(run_session(
        far,
        Arc::new(Ears("x")),
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
    // From here the peer neither reads nor writes; the keepalive pings pile up in the pipe.
    let ended = timeout(Duration::from_secs(5), session)
        .await
        .expect("the session is wedged on a write to a peer that does not read")
        .unwrap();
    let why = ended.expect_err("a peer that does not read is a dead peer");
    assert!(why.contains("write"), "{why}");
}

// ----- a preview voice travels with the request (REQ-CFG-080) -----

/// A voice with a configured setting. A line made with `synthesize_voice` uses the voice it was given; a
/// normal line uses the configured one. The voice shows in the length of the audio: A is 1x, B 2x, C 3x.
struct VoiceMouth {
    configured: std::sync::Mutex<String>,
}
impl VoiceMouth {
    fn say(text: &str, voice: &str) -> Vec<i16> {
        let times = match voice {
            "B" => 2,
            "C" => 3,
            _ => 1,
        };
        vec![7; text.len() * 100 * times]
    }
}
impl Synthesizer for VoiceMouth {
    async fn synthesize(&self, text: &str) -> Result<Vec<i16>, String> {
        let voice = self.configured.lock().unwrap().clone();
        tokio::time::sleep(Duration::from_millis(150)).await;
        Ok(Self::say(text, &voice))
    }
    async fn synthesize_voice(&self, text: &str, voice: &str) -> Result<Vec<i16>, String> {
        tokio::time::sleep(Duration::from_millis(150)).await;
        Ok(Self::say(text, voice))
    }
}

async fn session_with(tts: Arc<VoiceMouth>) -> (Peer, Announcer) {
    let s = fast();
    let ann = s.announcer.clone();
    let (near, far) = tokio::io::duplex(1 << 20);
    let (tx, _rx) = mpsc::channel::<Ask>(4);
    tokio::spawn(run_session(far, Arc::new(Ears("x")), tts, tx, s));
    let mut p = Peer {
        io: near,
        dec: Decoder::new(Limits::default()),
    };
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

/// A preview in voice B, a normal line and a preview in voice C are made at the same moment: each comes out in
/// its own voice and the configured voice is unchanged afterwards.
#[tokio::test]
async fn overlapping_previews_and_a_normal_line_each_use_their_own_voice() {
    let tts = Arc::new(VoiceMouth {
        configured: std::sync::Mutex::new("A".into()),
    });
    let (mut b, ann_b) = session_with(Arc::clone(&tts)).await;
    let (mut n, ann_n) = session_with(Arc::clone(&tts)).await;
    let (mut c, ann_c) = session_with(Arc::clone(&tts)).await;
    let (rb, rn, rc) = tokio::join!(
        ann_b.announce_in_voice("Hello.", Some("B")),
        ann_n.announce_in_voice("Hello.", None),
        ann_c.announce_in_voice("Hello.", Some("C")),
    );
    assert!(rb && rn && rc);
    let stop = |m: &Message| matches!(m, Message::AudioStop);
    let sb = samples(&until(&mut b, stop).await);
    let sn = samples(&until(&mut n, stop).await);
    let sc = samples(&until(&mut c, stop).await);
    assert_eq!(
        (sb, sn, sc),
        (
            "Hello.".len() * 200,
            "Hello.".len() * 100,
            "Hello.".len() * 300
        )
    );
    assert_eq!(&*tts.configured.lock().unwrap(), "A");
}

mod reply;
