//! The session phases, each exercised alone against an in-memory writer (REQ-SAT-002, REQ-MUS-004, REQ-SAT-020).

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
