//! Handshake, authorisation, a full spoken turn, mute, wire errors, and the limits that protect the session.

use super::*;

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

// REQ-SEC-011: every connection has its own speaker id. (A failure of the system's random source cannot be
// forced here; it makes the id an error and the handshake answers `unavailable`.)
#[test]
fn session_ids_are_random_and_differ() {
    let a = crate::satellite::wire::new_session_id().unwrap();
    let b = crate::satellite::wire::new_session_id().unwrap();
    assert_eq!(a.len(), 16);
    assert_ne!(a, "0".repeat(16));
    assert_ne!(a, b);
}
