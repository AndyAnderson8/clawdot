//! Music on a satellite (REQ-MUS-001, REQ-MUS-004).

use super::*;

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

fn jazz_url() -> Message {
    Message::PlayUrl {
        url: "http://r.example/jazz".into(),
        title: "jazz".into(),
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
