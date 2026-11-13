//! Slow speech synthesis never stalls the session (REQ-SAT-008, REQ-VOI-020).

use super::*;

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
