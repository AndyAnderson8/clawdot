//! The realtime voice wired into a speaker's connection (REQ-RTV-009 to REQ-RTV-011), in one process: a speaker
//! (the test), the satellite session, the core's side of the hand-off ([`serve`]) and a fake provider server on
//! loopback that follows the documented events. The real service has not been used.

use super::b64;
use super::serve::serve;
use super::tests::{Host, calls_done, next, put};
use super::ws::{WebSocket, accept};
use super::{Env, Io, Msg};
use crate::assistant::Effect;
use crate::rtvoice::{Done, Realtime, RtSettings};
use crate::satellite::{Answer, Ask, Settings, next_message, run_session};
use crate::session::{Conversation, TurnConfig};
use crate::usage::{UsageLimits, UsageTracker};
use crate::vad::tests::tone;
use crate::voice::{Synthesizer, Transcriber};
use clawdot_protocol::frame::{Decoder, Limits, encode};
use clawdot_protocol::message::{
    AudioFormat, Button, Capabilities, Hello, Indicator, Message, PROTOCOL_VERSION, Wake,
};
use serde_json::json;
use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncWriteExt, DuplexStream};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::timeout;

struct Ears;
impl Transcriber for Ears {
    async fn transcribe(&self, _: &[i16]) -> Result<String, String> {
        Ok("what time is it".into())
    }
}

/// Text to speech that remembers what it was asked to say.
#[derive(Clone, Default)]
struct Mouth(Arc<Mutex<Vec<String>>>);
impl Synthesizer for Mouth {
    async fn synthesize(&self, text: &str) -> Result<Vec<i16>, String> {
        self.0.lock().unwrap().push(text.to_owned());
        Ok(vec![7; 1600])
    }
}
impl Mouth {
    fn said(&self) -> String {
        self.0.lock().unwrap().join(" | ")
    }
}

/// The speaker's end of the connection.
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

    /// Read until `stop` accepts a message (answering keepalives), returning everything read, that one last.
    async fn until(&mut self, what: &str, stop: impl Fn(&Message) -> bool) -> Vec<Message> {
        let mut seen = Vec::new();
        loop {
            let m = timeout(
                Duration::from_secs(5),
                next_message(&mut self.io, &mut self.dec),
            )
            .await
            .unwrap_or_else(|_| panic!("timed out waiting for {what}; saw {seen:?}"))
            .unwrap()
            .unwrap_or_else(|| panic!("closed while waiting for {what}; saw {seen:?}"));
            if m == Message::Ping {
                self.send(&Message::Pong).await;
                continue;
            }
            let done = stop(&m);
            seen.push(m);
            if done {
                return seen;
            }
        }
    }

    async fn wake(&mut self) {
        self.send(&Message::Wake(Wake {
            name: "hey".into(),
            timestamp_ms: None,
        }))
        .await;
    }

    async fn talk(&mut self, ms: usize) {
        for chunk in tone(3000.0, ms).chunks(320) {
            let payload: Vec<u8> = chunk.iter().flat_map(|s| s.to_le_bytes()).collect();
            self.send(&Message::AudioChunk {
                format: AudioFormat::PCM_16K_MONO,
                timestamp_ms: None,
                payload,
            })
            .await;
        }
    }

    async fn button(&mut self, name: &str) {
        self.send(&Message::Button(Button {
            name: name.into(),
            long: false,
        }))
        .await;
    }

    /// Hear a whole spoken reply: its chunks, then its stop. Returns the samples in it.
    async fn reply(&mut self) -> usize {
        let seen = self
            .until("a reply", |m| matches!(m, Message::AudioStop))
            .await;
        assert!(
            seen.iter().any(|m| matches!(m, Message::AudioStart(_))),
            "{seen:?}"
        );
        seen.iter()
            .map(|m| match m {
                Message::AudioChunk { payload, .. } => payload.len() / 2,
                _ => 0,
            })
            .sum()
    }
}

fn is_stream_start(m: &Message) -> bool {
    matches!(m, Message::StreamStart)
}

fn is_stream_stop(m: &Message) -> bool {
    matches!(m, Message::StreamStop)
}

/// A provider that accepts connections, counts them, and runs `script` on the first.
async fn provider<F, Fut>(script: F) -> (String, Arc<AtomicUsize>, JoinHandle<()>)
where
    F: FnOnce(WebSocket<TcpStream>) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "ws://127.0.0.1:{}/v1/realtime",
        listener.local_addr().unwrap().port()
    );
    let accepted = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&accepted);
    let h = tokio::spawn(async move {
        let mut script = Some(script);
        while let Ok((sock, _)) = listener.accept().await {
            count.fetch_add(1, Ordering::SeqCst);
            if let Some(s) = script.take() {
                if let Ok((ws, _)) = accept(sock).await {
                    s(ws).await;
                }
            }
        }
    });
    (url, accepted, h)
}

/// Wait for the first piece of microphone audio the session sends.
async fn first_audio(ws: &mut WebSocket<TcpStream>) {
    loop {
        let Msg::Text(t) = ws.recv().await.unwrap() else {
            panic!("closed before any audio")
        };
        if t.contains("input_audio_buffer.append") {
            return;
        }
    }
}

/// Read until the provider's side of the session ends (the core closed it).
async fn until_closed(ws: &mut WebSocket<TcpStream>) {
    while let Ok(m) = ws.recv().await {
        if m == Msg::Close {
            return;
        }
    }
}

fn delta(samples_24k: usize) -> serde_json::Value {
    json!({"type": "response.output_audio.delta", "delta": b64::encode(&vec![9u8; samples_24k * 2])})
}

struct Rig {
    peer: Peer,
    rt: Realtime,
    spoken: Mouth,
    brain: JoinHandle<(Host, UsageTracker)>,
    _session: JoinHandle<Result<(), String>>,
}

struct Opts {
    url: String,
    enabled: bool,
    spent: f64,
    cap: Option<f64>,
    follow_up_secs: u64,
    rt_idle: Duration,
}

impl Opts {
    fn new(url: &str) -> Self {
        Self {
            url: url.to_owned(),
            enabled: true,
            spent: 0.0,
            cap: None,
            follow_up_secs: 0,
            rt_idle: Duration::from_secs(30),
        }
    }
}

/// A speaker connected to a satellite session, a core that runs `jobs` realtime sessions, and nothing else.
async fn rig(o: Opts, jobs: usize) -> Rig {
    let rt = Realtime::with_env(
        RtSettings {
            enabled: o.enabled,
            url: Some(o.url),
            model: Some("m-1".into()),
            max_session_secs: 20,
            ..RtSettings::default()
        },
        Box::new(|_| Some("sk-test-secret".into())),
    );
    let mut jobs_rx = rt.attach();
    let core = rt.clone();
    let spent = o.spent;
    let limits = UsageLimits {
        daily_spend_cap_usd: o.cap,
        ..UsageLimits::default()
    };
    let brain = tokio::spawn(async move {
        let mut host = Host::default();
        let mut usage = UsageTracker::new(limits);
        usage.add_cost(1, spent);
        for _ in 0..jobs {
            let Some(job) = jobs_rx.recv().await else {
                break;
            };
            core.set_error(None);
            let mut conv = Conversation::new("system", 20, 20_000, 600);
            let tcfg = TurnConfig::default();
            let clock = || (1_i64, 1_000_i64);
            let finish = {
                let mut env = Env {
                    host: &mut host,
                    conv: &mut conv,
                    turn_cfg: &tcfg,
                    usage: &mut usage,
                    clock: &clock,
                };
                serve(
                    &core.settings(),
                    core.key().as_deref(),
                    "",
                    Io {
                        mic: job.mic,
                        speaker: job.speaker,
                    },
                    &mut env,
                )
                .await
            };
            core.set_error(finish.error.clone());
            let _ = job.done.send(Done {
                speak: finish.speak,
                failed: finish.failed,
                effects: vec![Effect::VolumeSet(30)],
            });
        }
        (host, usage)
    });

    let (near, far) = tokio::io::duplex(1 << 20);
    let (asks_tx, mut asks_rx) = mpsc::channel::<Ask>(4);
    tokio::spawn(async move {
        while let Some(ask) = asks_rx.recv().await {
            let _ = ask.reply.send(Answer {
                say: format!("classic heard {}", ask.text),
                ..Answer::default()
            });
        }
    });
    let spoken = Mouth::default();
    let mut settings = Settings {
        keepalive: Duration::from_millis(50),
        dead_after: Duration::from_secs(10),
        realtime: Some(rt.clone()),
        rt_idle: o.rt_idle,
        ..Settings::default()
    };
    settings.dialog.follow_up_secs = o.follow_up_secs;
    let session = tokio::spawn(run_session(
        far,
        Arc::new(Ears),
        Arc::new(spoken.clone()),
        asks_tx,
        settings,
    ));
    let mut peer = Peer {
        io: near,
        dec: Decoder::new(Limits::default()),
    };
    peer.send(&Message::Hello(Hello {
        protocol: PROTOCOL_VERSION,
        device_id: "test-dot".into(),
        name: None,
        token: None,
        capabilities: Capabilities {
            wake_word: true,
            ..Capabilities::default()
        },
    }))
    .await;
    peer.until("welcome", |m| matches!(m, Message::Welcome(_)))
        .await;
    Rig {
        peer,
        rt,
        spoken,
        brain,
        _session: session,
    }
}

fn closed_signal() -> (oneshot::Sender<()>, oneshot::Receiver<()>) {
    oneshot::channel()
}

async fn closed(rx: oneshot::Receiver<()>) {
    timeout(Duration::from_secs(5), rx)
        .await
        .expect("the provider's connection was not closed")
        .unwrap();
}

async fn brain_of(r: Rig) -> (Host, UsageTracker) {
    timeout(Duration::from_secs(5), r.brain)
        .await
        .unwrap()
        .unwrap()
}

// REQ-RTV-009, REQ-RTV-004, REQ-RTV-003, REQ-RTV-011
#[tokio::test(flavor = "current_thread")]
async fn a_whole_turn_runs_through_the_speaker_connection_and_the_host() {
    let (closed_tx, closed_rx) = closed_signal();
    let (url, accepted, _srv) = provider(|mut ws| async move {
        let su = next(&mut ws).await;
        assert_eq!(su["type"], "session.update");
        assert!(!su.to_string().contains("sk-test-secret"));
        first_audio(&mut ws).await;
        put(&mut ws, json!({"type": "response.created"})).await;
        put(&mut ws, delta(2400)).await;
        put(&mut ws, calls_done(&[("c1", "get_time", "{}")])).await;
        assert_eq!(next(&mut ws).await["item"]["call_id"], "c1");
        assert_eq!(next(&mut ws).await["type"], "response.create");
        put(&mut ws, delta(2400)).await;
        until_closed(&mut ws).await;
        let _ = closed_tx.send(());
    })
    .await;
    let mut r = rig(
        Opts {
            rt_idle: Duration::from_millis(600),
            ..Opts::new(&url)
        },
        1,
    )
    .await;

    r.peer.wake().await;
    let seen = r.peer.until("the stream", is_stream_start).await;
    assert!(seen.contains(&Message::SetIndicator(Indicator::Listening)));
    assert_eq!(r.rt.status()["active"], true, "a session is open");
    r.peer.talk(300).await;

    // The model's 2 x 2400 samples at 24 kHz arrive as 2 x 1600 at 16 kHz in one streamed reply.
    assert_eq!(r.peer.reply().await, 3200);
    r.peer.send(&Message::PlaybackDone { id: None }).await;

    // Nobody speaks, so the session ends by itself, the speaker stops streaming and the provider is closed.
    r.peer.until("the end", is_stream_stop).await;
    closed(closed_rx).await;
    let st = r.rt.status();
    let spoken = r.spoken.said();
    let (host, usage) = brain_of(r).await;
    assert_eq!(host.ran, vec![("get_time".to_owned(), json!({}))]);
    assert!(usage.spend_today() > 0.0, "the audio was counted");
    assert_eq!(accepted.load(Ordering::SeqCst), 1);
    assert_eq!(st["last_error"], json!(null));
    assert!(spoken.is_empty(), "the normal voice said nothing: {spoken}");
}

// REQ-RTV-009
#[tokio::test(flavor = "current_thread")]
async fn the_user_cutting_in_stops_playback_and_the_action_button_ends_the_session() {
    let (closed_tx, closed_rx) = closed_signal();
    let (url, _, _srv) = provider(|mut ws| async move {
        next(&mut ws).await;
        first_audio(&mut ws).await;
        put(&mut ws, json!({"type": "response.created"})).await;
        put(&mut ws, delta(2400)).await;
        put(
            &mut ws,
            json!({"type": "input_audio_buffer.speech_started"}),
        )
        .await;
        until_closed(&mut ws).await;
        let _ = closed_tx.send(());
    })
    .await;
    let mut r = rig(Opts::new(&url), 1).await;
    r.peer.wake().await;
    r.peer.until("the stream", is_stream_start).await;
    r.peer.talk(100).await;
    let seen = r
        .peer
        .until("playback to stop", |m| matches!(m, Message::StopPlayback))
        .await;
    assert!(seen.iter().any(|m| matches!(m, Message::AudioStart(_))));
    assert!(
        !seen.contains(&Message::AudioStop),
        "the reply was cut off, not finished: {seen:?}"
    );
    r.peer.button("action").await;
    r.peer.until("the end", is_stream_stop).await;
    closed(closed_rx).await;
    brain_of(r).await;
}

// REQ-RTV-009
#[tokio::test(flavor = "current_thread")]
async fn muting_the_speaker_ends_the_session() {
    let (closed_tx, closed_rx) = closed_signal();
    let (url, _, _srv) = provider(|mut ws| async move {
        next(&mut ws).await;
        until_closed(&mut ws).await;
        let _ = closed_tx.send(());
    })
    .await;
    let mut r = rig(Opts::new(&url), 1).await;
    r.peer.wake().await;
    r.peer.until("the stream", is_stream_start).await;
    r.peer.button("mute").await;
    r.peer
        .until("the mute", |m| {
            matches!(m, Message::SetMute { muted: true })
        })
        .await;
    closed(closed_rx).await;
    brain_of(r).await;
}

// REQ-RTV-010, REQ-RTV-006, REQ-RTV-011
#[tokio::test(flavor = "current_thread")]
async fn a_provider_failure_is_spoken_and_the_normal_path_takes_over() {
    let (url, accepted, _srv) = provider(|mut ws| async move {
        next(&mut ws).await;
        first_audio(&mut ws).await;
        put(
            &mut ws,
            json!({"type": "error", "error": {"code": "insufficient_quota", "message": "x"}}),
        )
        .await;
        until_closed(&mut ws).await;
    })
    .await;
    let mut r = rig(Opts::new(&url), 1).await;
    r.peer.wake().await;
    r.peer.until("the stream", is_stream_start).await;
    r.peer.talk(300).await;

    // The plain message is said in the normal voice, and the tool effects of the session are applied.
    let before = r
        .peer
        .until("the message", |m| matches!(m, Message::AudioStop))
        .await;
    assert!(
        before.contains(&Message::SetVolume { level: 30 }),
        "{before:?}"
    );
    assert!(
        r.spoken.said().contains("out of credit"),
        "{}",
        r.spoken.said()
    );
    r.peer.send(&Message::PlaybackDone { id: None }).await;
    let st = r.rt.status();
    assert!(st["last_error"].as_str().unwrap().contains("out of credit"));

    // The next request goes the normal way (listen, think, speak) and no second connection is made.
    r.peer.wake().await;
    r.peer.until("listening", is_stream_start).await;
    let mut audio = tone(3000.0, 300);
    audio.extend(vec![0i16; 16 * 1200]);
    for chunk in audio.chunks(320) {
        let payload: Vec<u8> = chunk.iter().flat_map(|s| s.to_le_bytes()).collect();
        r.peer
            .send(&Message::AudioChunk {
                format: AudioFormat::PCM_16K_MONO,
                timestamp_ms: None,
                payload,
            })
            .await;
    }
    r.peer.reply().await;
    assert!(
        r.spoken.said().contains("classic heard what time is it"),
        "{}",
        r.spoken.said()
    );
    assert_eq!(accepted.load(Ordering::SeqCst), 1);
    brain_of(r).await;
}

// REQ-RTV-010, REQ-RTV-006
#[tokio::test(flavor = "current_thread")]
async fn a_service_that_cannot_be_reached_is_spoken_plainly() {
    let mut r = rig(Opts::new("ws://127.0.0.1:1/v1/realtime"), 1).await;
    r.peer.wake().await;
    r.peer.reply().await;
    assert!(
        r.spoken
            .said()
            .contains("couldn't reach the realtime voice service"),
        "{}",
        r.spoken.said()
    );
    let st = r.rt.status();
    assert!(
        st["last_error"]
            .as_str()
            .unwrap()
            .contains("couldn't reach")
    );
    assert!(!st.to_string().contains("sk-test-secret"));
    brain_of(r).await;
}

// REQ-RTV-010, REQ-RTV-004
#[tokio::test(flavor = "current_thread")]
async fn a_day_over_its_cap_starts_no_session_and_says_why() {
    let (url, accepted, _srv) = provider(|_ws| async move {}).await;
    let mut r = rig(
        Opts {
            spent: 0.02,
            cap: Some(0.01),
            ..Opts::new(&url)
        },
        1,
    )
    .await;
    r.peer.wake().await;
    r.peer.reply().await;
    assert!(
        r.spoken.said().contains("spending limit"),
        "{}",
        r.spoken.said()
    );
    assert_eq!(
        accepted.load(Ordering::SeqCst),
        0,
        "no connection was opened"
    );
    assert!(
        r.rt.status()["last_error"]
            .as_str()
            .unwrap()
            .contains("spending limit")
    );
    brain_of(r).await;
}

// REQ-RTV-009, REQ-RTV-011
#[test]
fn only_one_session_runs_at_a_time_and_status_follows_it() {
    let rt = Realtime::with_env(
        RtSettings {
            enabled: true,
            url: Some("ws://127.0.0.1:1/x".into()),
            model: Some("m".into()),
            ..RtSettings::default()
        },
        Box::new(|_| Some("sk-test-secret".into())),
    );
    assert!(rt.claim().is_none(), "nothing is taking sessions yet");
    let _rx = rt.attach();
    assert_eq!(rt.status()["configured"], true);
    let first = rt.claim().expect("the first claim");
    assert!(rt.claim().is_none(), "a second session is refused");
    assert_eq!(rt.status()["active"], true);
    drop(first);
    assert_eq!(rt.status()["active"], false);
    assert!(rt.claim().is_some());
    // A key that is missing or an off switch means nothing can be claimed.
    let no_key = Realtime::with_env(rt.settings(), Box::new(|_| None));
    let _rx2 = no_key.attach();
    assert!(no_key.claim().is_none());
    assert_eq!(no_key.status()["configured"], false);
}

// REQ-RTV-001, REQ-RTV-009
#[tokio::test(flavor = "current_thread")]
async fn when_realtime_is_off_the_wake_word_takes_the_normal_path() {
    let (url, accepted, _srv) = provider(|_ws| async move {}).await;
    let mut r = rig(
        Opts {
            enabled: false,
            ..Opts::new(&url)
        },
        0,
    )
    .await;
    r.peer.wake().await;
    r.peer.until("listening", is_stream_start).await;
    assert_eq!(r.rt.status()["configured"], false);
    assert_eq!(r.rt.status()["active"], false);
    assert_eq!(accepted.load(Ordering::SeqCst), 0);
    brain_of(r).await;
}
