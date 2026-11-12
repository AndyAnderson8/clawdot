//! The reply is spoken as it is made: sentence one plays while the rest are still being synthesised.

use super::*;

/// What the fake voice has been asked, and the gate that decides when it may answer. The test releases one
/// synthesis at a time, so no timing is guessed.
#[derive(Clone)]
struct Probe {
    gate: Arc<Semaphore>,
    started: Arc<std::sync::atomic::AtomicUsize>,
    finished: Arc<std::sync::atomic::AtomicUsize>,
}

impl Probe {
    fn new() -> Self {
        Self {
            gate: Arc::new(Semaphore::new(0)),
            started: Arc::default(),
            finished: Arc::default(),
        }
    }
    /// Let `n` more syntheses finish.
    fn release(&self, n: usize) {
        self.gate.add_permits(n);
    }
    fn started(&self) -> usize {
        self.started.load(Ordering::SeqCst)
    }
    fn finished(&self) -> usize {
        self.finished.load(Ordering::SeqCst)
    }
    /// Wait until `n` syntheses have been asked for (a request counts once it is first polled).
    async fn wait_started(&self, n: usize) {
        timeout(Duration::from_secs(5), async {
            while self.started() < n {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("only {} syntheses were asked for, not {n}", self.started()));
    }
}

/// A voice that answers only when the test lets it. Its n-th synthesis is filled with the number n, so the
/// order that reaches the wire can be read from the audio, and it fails on `fail_on` when set.
struct GatedMouth {
    probe: Probe,
    fail_on: Option<usize>,
    /// Samples per character of text.
    per_char: usize,
}

impl Synthesizer for GatedMouth {
    async fn synthesize(&self, text: &str) -> Result<Vec<i16>, String> {
        let n = self.probe.started.fetch_add(1, Ordering::SeqCst) + 1;
        self.probe.gate.acquire().await.expect("gate").forget();
        self.probe.finished.fetch_add(1, Ordering::SeqCst);
        if self.fail_on == Some(n) {
            return Err("voice unavailable".into());
        }
        Ok(vec![i16::try_from(n).unwrap(); text.len() * self.per_char])
    }
}

/// A session with a gated voice.
async fn start_gated<E>(
    s: Settings,
    ears: E,
    probe: &Probe,
    fail_on: Option<usize>,
    respond: fn(&str) -> Answer,
) -> Peer
where
    E: Transcriber + Send + Sync + 'static,
{
    start_gated_long(s, ears, probe, fail_on, 100, respond).await
}

/// Like `start_gated`, with `per_char` samples of audio for every character of text.
async fn start_gated_long<E>(
    s: Settings,
    ears: E,
    probe: &Probe,
    fail_on: Option<usize>,
    per_char: usize,
    respond: fn(&str) -> Answer,
) -> Peer
where
    E: Transcriber + Send + Sync + 'static,
{
    let (near, far) = tokio::io::duplex(1 << 20);
    let (tx, mut rx) = mpsc::channel::<Ask>(4);
    tokio::spawn(async move {
        while let Some(ask) = rx.recv().await {
            let _ = ask.reply.send(respond(&ask.text));
        }
    });
    let mouth = GatedMouth {
        probe: probe.clone(),
        fail_on,
        per_char,
    };
    tokio::spawn(run_session(far, Arc::new(ears), Arc::new(mouth), tx, s));
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
    p
}

fn four(_: &str) -> Answer {
    Answer {
        say: "One. Two. Three. Four.".into(),
        ..Answer::default()
    }
}

/// Settings that keep the core from holding back synthesis, so a test sees every sentence made at once.
fn eager() -> Settings {
    Settings {
        reply_lead: Duration::from_secs(3600),
        ..fast()
    }
}

/// The next message within `ms`, with pings answered; `None` when nothing else comes.
async fn recv_for(p: &mut Peer, ms: u64) -> Option<Message> {
    let end = Instant::now() + Duration::from_millis(ms);
    loop {
        let left = end.saturating_duration_since(Instant::now());
        match timeout(left, next_message(&mut p.io, &mut p.dec)).await {
            Ok(Ok(Some(Message::Ping))) => p.send(&Message::Pong).await,
            Ok(Ok(Some(m))) => return Some(m),
            _ => return None,
        }
    }
}

/// Which sentences the audio chunks carry, in the order they were sent (each synthesis is filled with its number).
fn sentence_ids(seen: &[Message]) -> Vec<i16> {
    let mut ids: Vec<i16> = Vec::new();
    for m in seen {
        if let Message::AudioChunk { payload, .. } = m {
            let id = i16::from_le_bytes([payload[0], payload[1]]);
            if ids.last() != Some(&id) {
                ids.push(id);
            }
        }
    }
    ids
}

fn count(seen: &[Message], what: &Message) -> usize {
    seen.iter().filter(|m| *m == what).count()
}

fn starts(seen: &[Message]) -> usize {
    seen.iter()
        .filter(|m| matches!(m, Message::AudioStart(_)))
        .count()
}

fn chunks_or_stop(seen: &[Message]) -> bool {
    seen.iter()
        .any(|m| matches!(m, Message::AudioChunk { .. } | Message::AudioStop))
}

fn action_button() -> Message {
    Message::Button(clawdot_protocol::message::Button {
        name: "action".into(),
        long: false,
    })
}

/// Let syntheses finish one at a time until the first audio reaches the satellite; returns how many it took and
/// everything seen up to and including that first chunk.
async fn until_first_audio(p: &mut Peer, probe: &Probe, most: usize) -> (usize, Vec<Message>) {
    let mut seen = Vec::new();
    for released in 1..=most {
        probe.release(1);
        while let Some(m) = recv_for(p, 1000).await {
            let chunk = matches!(m, Message::AudioChunk { .. });
            seen.push(m);
            if chunk {
                return (released, seen);
            }
        }
    }
    panic!("no audio even after {most} syntheses: {seen:?}");
}

/// Time to first sound counts the syntheses that had to finish before it. Before this change it was every
/// sentence of the reply (four here); now it is one, and the rest follow in order inside one audio stream.
#[tokio::test]
async fn the_first_sentence_plays_after_one_synthesis_not_all_of_them() {
    let probe = Probe::new();
    let mut p = start_gated(eager(), Ears("tell me four things"), &probe, None, four).await;
    speak(&mut p).await;
    let (needed, first) = until_first_audio(&mut p, &probe, 4).await;
    assert_eq!(needed, 1, "the first sound waited for {needed} syntheses");
    assert_eq!(probe.finished(), 1);
    assert_eq!(sentence_ids(&first), [1]);
    assert!(first.contains(&Message::SetIndicator(Indicator::Speaking)));
    assert_eq!(starts(&first), 1);
    // Nothing closes the stream while the rest is still being made.
    assert!(p.quiet_for(150).await, "the stream ended early");

    probe.release(3);
    let rest = until(&mut p, |m| matches!(m, Message::AudioStop)).await;
    assert_eq!(sentence_ids(&rest), [2, 3, 4], "order is kept: {rest:?}");
    assert_eq!(starts(&rest), 0, "one stream for the whole reply");
    assert!(p.quiet_for(100).await);
    assert_eq!(probe.started(), 4);

    // The follow-up window opens once, after the last sentence has played.
    p.send(&Message::PlaybackDone { id: None }).await;
    let after = collect_for(&mut p, 200).await;
    assert_eq!(
        count(&after, &Message::SetIndicator(Indicator::Listening)),
        1
    );
    assert_eq!(count(&after, &Message::StreamStart), 1);
}

/// The thinking cue ends when the reply starts, not when it is complete.
#[tokio::test]
async fn the_thinking_cue_stops_when_the_first_sentence_starts() {
    let probe = Probe::new();
    let mut s = eager();
    s.dialog.thinking_delay_ms = 300;
    let mut p = start_gated(s, Ears("tell me four things"), &probe, None, four).await;
    speak(&mut p).await;
    let (_, first) = until_first_audio(&mut p, &probe, 4).await;
    assert!(!first.contains(&Message::SetIndicator(Indicator::Error)));
    // The second sentence is slow: far longer than the cue's delay. The cue must not play over the reply.
    let waiting = collect_for(&mut p, 700).await;
    assert!(
        !waiting.contains(&Message::SetIndicator(Indicator::Thinking)),
        "the thinking cue played during the reply: {waiting:?}"
    );
    probe.release(3);
    until(&mut p, |m| matches!(m, Message::AudioStop)).await;
}

/// The action button while the reply is playing stops it and the synthesis behind it: audio nobody will hear is
/// not paid for.
#[tokio::test]
async fn stopping_during_the_second_sentence_means_the_rest_is_never_synthesised() {
    let probe = Probe::new();
    let mut p = start_gated(eager(), Ears("tell me four things"), &probe, None, four).await;
    speak(&mut p).await;
    until_first_audio(&mut p, &probe, 4).await;
    // Sentence 2 is being made now.
    probe.wait_started(2).await;
    p.send(&action_button()).await;
    let seen = until(&mut p, |m| *m == Message::StopPlayback).await;
    assert!(!chunks_or_stop(&seen), "{seen:?}");
    probe.release(10);
    let after = collect_for(&mut p, 300).await;
    assert!(
        !chunks_or_stop(&after),
        "audio was sent after the stop: {after:?}"
    );
    assert_eq!(
        probe.started(),
        2,
        "sentence 3 was synthesised after the stop"
    );
    // The stop ends the reply like a finished one: the follow-up window opens, once.
    assert_eq!(
        count(&after, &Message::SetIndicator(Indicator::Listening)),
        1
    );
}

/// A stop phrase before the first sentence is ready: nothing is played at all.
#[tokio::test]
async fn a_stop_phrase_before_the_first_sentence_is_ready_plays_nothing() {
    let probe = Probe::new();
    let mut p = start_gated(
        fast(),
        Cycle::new(&["tell me four things", "stop"]),
        &probe,
        None,
        four,
    )
    .await;
    ask_and_wait_for_thinking(&mut p).await;
    probe.wait_started(1).await;
    p.send(&wake()).await;
    until(&mut p, |m| *m == Message::StreamStart).await;
    p.say(500).await;
    let seen = until(&mut p, |m| *m == Message::SetIndicator(Indicator::Idle)).await;
    assert!(seen.contains(&Message::StreamStop), "{seen:?}");
    probe.release(10);
    let after = collect_for(&mut p, 300).await;
    assert!(
        !after
            .iter()
            .any(|m| matches!(m, Message::AudioStart(_) | Message::AudioChunk { .. })),
        "the reply played after it was stopped: {after:?}"
    );
    assert_eq!(
        probe.started(),
        1,
        "only the sentence in flight was asked for"
    );
}

/// The satellite reporting that playback ended before the reply was complete (its own stop button) also ends the
/// synthesis.
#[tokio::test]
async fn playback_done_before_the_last_sentence_stops_the_synthesis() {
    let probe = Probe::new();
    let mut p = start_gated(eager(), Ears("tell me four things"), &probe, None, four).await;
    speak(&mut p).await;
    until_first_audio(&mut p, &probe, 4).await;
    probe.wait_started(2).await;
    p.send(&Message::PlaybackDone { id: None }).await;
    // Once the core has acted on it (the follow-up window opens) the rest may be made; it must not be.
    let seen = until(&mut p, |m| *m == Message::StreamStart).await;
    assert!(!chunks_or_stop(&seen), "{seen:?}");
    probe.release(10);
    let after = collect_for(&mut p, 300).await;
    assert!(!chunks_or_stop(&after), "{after:?}");
    assert_eq!(probe.started(), 2);
    assert_eq!(
        count(&seen, &Message::SetIndicator(Indicator::Listening)),
        1
    );
}

/// What was played stays played, and the reply ends cleanly: the stream is closed, no error cue, and the
/// follow-up window opens once.
#[tokio::test]
async fn a_failure_on_the_third_sentence_leaves_the_first_two_played_and_ends_cleanly() {
    let probe = Probe::new();
    let mut p = start_gated(eager(), Ears("tell me four things"), &probe, Some(3), four).await;
    speak(&mut p).await;
    probe.release(10);
    let seen = until(&mut p, |m| matches!(m, Message::AudioStop)).await;
    assert_eq!(sentence_ids(&seen), [1, 2], "{seen:?}");
    assert!(!seen.contains(&Message::SetIndicator(Indicator::Error)));
    assert_eq!(starts(&seen), 1);
    let quiet = collect_for(&mut p, 200).await;
    assert!(
        !quiet.iter().any(|m| matches!(
            m,
            Message::AudioChunk { .. } | Message::SetIndicator(Indicator::Error)
        )),
        "{quiet:?}"
    );
    assert_eq!(probe.started(), 3, "the fourth sentence is not attempted");
    p.send(&Message::PlaybackDone { id: None }).await;
    let after = collect_for(&mut p, 200).await;
    assert_eq!(
        count(&after, &Message::SetIndicator(Indicator::Listening)),
        1
    );
}

/// With nothing played yet, a failing voice is the error cue, as before.
#[tokio::test]
async fn a_failure_on_the_first_sentence_is_the_error_cue() {
    let probe = Probe::new();
    let mut p = start_gated(fast(), Ears("tell me four things"), &probe, Some(1), four).await;
    speak(&mut p).await;
    probe.release(10);
    let seen = until(&mut p, |m| *m == Message::SetIndicator(Indicator::Error)).await;
    assert!(
        !seen.iter().any(|m| matches!(m, Message::AudioStart(_))),
        "{seen:?}"
    );
}

/// Music is paused before the first sentence and comes back once, after the last one has played.
#[tokio::test]
async fn music_resumes_once_after_the_last_sentence() {
    fn jazz_then_four(heard: &str) -> Answer {
        if heard.contains("jazz") {
            play_jazz(heard)
        } else {
            four(heard)
        }
    }
    let probe = Probe::new();
    let mut p = start_gated(
        eager(),
        Cycle::new(&["play jazz", "tell me four things"]),
        &probe,
        None,
        jazz_then_four,
    )
    .await;
    speak(&mut p).await;
    probe.release(1);
    until(&mut p, |m| matches!(m, Message::AudioStop)).await;
    p.send(&Message::PlaybackDone { id: None }).await;
    until(&mut p, |m| matches!(m, Message::StreamStart)).await;

    // The radio plays; now a four-sentence reply.
    p.say(600).await;
    let (_, first) = until_first_audio(&mut p, &probe, 4).await;
    let pause = first
        .iter()
        .position(|m| *m == Message::MediaControl(MediaAction::Pause));
    let start = first
        .iter()
        .position(|m| matches!(m, Message::AudioStart(_)));
    assert!(
        pause.is_some() && pause < start,
        "paused before speech: {first:?}"
    );
    probe.release(2);
    let middle = collect_for(&mut p, 200).await;
    assert!(
        !middle.contains(&Message::MediaControl(MediaAction::Resume)),
        "music came back between sentences: {middle:?}"
    );
    probe.release(1);
    let last = until(&mut p, |m| matches!(m, Message::AudioStop)).await;
    assert!(!last.contains(&Message::MediaControl(MediaAction::Resume)));
    p.send(&Message::PlaybackDone { id: None }).await;
    let after = collect_for(&mut p, 300).await;
    assert_eq!(
        count(&after, &Message::MediaControl(MediaAction::Resume)),
        1,
        "{after:?}"
    );
    assert_eq!(
        count(&after, &Message::SetIndicator(Indicator::Listening)),
        1
    );
}

/// Synthesis keeps about `reply_lead` ahead of the playback it estimates, no more: with a lead of nothing the
/// next sentence is not asked for until the one before it has had time to play, so a stop pays for at most one
/// sentence that is never heard.
#[tokio::test]
async fn synthesis_stays_about_one_sentence_ahead_of_playback() {
    let probe = Probe::new();
    let s = Settings {
        reply_lead: Duration::from_millis(100),
        ..fast()
    };
    // 1600 samples per character: "One." is 4 characters, 0.4 s of audio.
    let mut p = start_gated_long(s, Ears("tell me four things"), &probe, None, 1600, four).await;
    speak(&mut p).await;
    probe.release(10);
    let mut seen = until(&mut p, |m| matches!(m, Message::AudioChunk { .. })).await;
    let asked = probe.started();
    assert_eq!(
        asked, 1,
        "the second sentence was asked for while the first had barely started"
    );
    // The rest follow as the clock catches up, in order, and the stream closes once.
    seen.extend(until(&mut p, |m| matches!(m, Message::AudioStop)).await);
    assert_eq!(sentence_ids(&seen), [1, 2, 3, 4]);
    assert_eq!(probe.started(), 4);
}
