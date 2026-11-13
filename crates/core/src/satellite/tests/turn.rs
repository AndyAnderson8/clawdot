//! A turn that takes a while: speech while it works, and stopping it (REQ-AST-020, REQ-AST-022).

use super::*;

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

#[tokio::test]
async fn a_turn_is_cancelled_when_the_speaker_goes_away() {
    let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut p = start_with(
        Ears("check the runs"),
        wait_for_stop(Duration::from_secs(5), Arc::clone(&cancelled)),
    )
    .await;
    ask_and_wait_for_thinking(&mut p).await;
    drop(p);
    for _ in 0..40 {
        if cancelled.load(Ordering::SeqCst) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the model call kept running after the session ended");
}
