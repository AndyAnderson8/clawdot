//! The ESPHome voice-satellite client against a fake ESPHome device (spec 0004, REQ-ESP-016).
//!
//! The fake device is an ESPHome *server*: it listens, answers the client's setup, and then plays the
//! device's part of a voice run. The client under test is the real one, wired to the real satellite pipeline
//! with fake speech providers.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use clawdot_core::esphome::frame::{Link, plain_frame};
use clawdot_core::esphome::media::{MediaStore, serve as serve_media};
use clawdot_core::esphome::noise;
use clawdot_core::esphome::proto::{Msg, event, flags, media as player};
use clawdot_core::esphome::{Device, Pipeline, Timing, run_device};
use clawdot_core::satellite::{Announcer, Answer, Ask, Settings};
use clawdot_core::voice::{Config as DialogConfig, Synthesizer, Transcriber};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::time::timeout;

const PATIENCE: Duration = Duration::from_secs(8);
/// Voice, speaker, API audio, announce, start conversation.
const FULL: u32 = flags::VOICE_ASSISTANT
    | flags::SPEAKER
    | flags::API_AUDIO
    | flags::ANNOUNCE
    | flags::START_CONVERSATION;

struct Ears;
impl Transcriber for Ears {
    async fn transcribe(&self, pcm: &[i16]) -> Result<String, String> {
        assert!(pcm.len() > 8000, "a real utterance arrives: {}", pcm.len());
        Ok("what is on my plate".to_owned())
    }
}

struct Mouth;
impl Synthesizer for Mouth {
    async fn synthesize(&self, text: &str) -> Result<Vec<i16>, String> {
        Ok(vec![9; text.len() * 50])
    }
}

fn quick() -> Timing {
    Timing {
        connect: Duration::from_secs(2),
        setup_step: Duration::from_secs(2),
        ping_every: Duration::from_millis(300),
        dead_after: Duration::from_secs(3),
        wake_watch: Duration::from_secs(1),
        mic_idle: Duration::from_secs(3),
        api_margin: Duration::from_millis(50),
        url_margin: Duration::from_millis(100),
        announce_margin: Duration::from_secs(2),
        chunk_every: Duration::from_millis(1),
        backoff_min: Duration::from_millis(30),
        backoff_max: Duration::from_millis(120),
    }
}

/// What the fake device is like.
#[derive(Clone, Copy)]
struct Spec {
    flags: u32,
    major: u32,
    media_player: bool,
}

impl Spec {
    const fn full() -> Self {
        Self {
            flags: FULL,
            major: 1,
            media_player: true,
        }
    }
}

/// The device side of one connection.
struct Conn {
    link: Link<TcpStream>,
}

impl Conn {
    /// The next message, answering pings on the way.
    async fn recv(&mut self) -> Msg {
        timeout(PATIENCE, async {
            loop {
                match self.link.recv().await.expect("a valid message") {
                    Some(Msg::PingRequest) => self.link.send(&Msg::PingResponse).await.unwrap(),
                    Some(m) => return m,
                    None => panic!("the client closed the connection"),
                }
            }
        })
        .await
        .expect("timed out waiting for the client")
    }

    async fn send(&mut self, m: Msg) {
        self.link.send(&m).await.unwrap();
    }

    /// Wait for a message matching `want`, skipping others.
    async fn expect(&mut self, what: &str, want: impl Fn(&Msg) -> bool) -> Msg {
        timeout(PATIENCE, async {
            loop {
                let m = self.recv().await;
                if want(&m) {
                    return m;
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
    }

    /// The device's side of the client's setup; returns the subscription message.
    async fn setup(&mut self, spec: Spec) -> Option<Msg> {
        let Msg::HelloRequest { major, .. } = self.recv().await else {
            panic!("expected a hello")
        };
        assert_eq!(major, 1);
        self.send(Msg::HelloResponse {
            major: spec.major,
            minor: 12,
            server_info: "fake".into(),
            name: "fake-dot".into(),
        })
        .await;
        if spec.major != 1 {
            return None;
        }
        assert_eq!(self.recv().await, Msg::DeviceInfoRequest);
        self.send(Msg::DeviceInfoResponse {
            name: "fake-dot".into(),
            esphome_version: "2026.1".into(),
            friendly_name: "Fake Dot".into(),
            voice_flags: spec.flags,
        })
        .await;
        if spec.flags & flags::VOICE_ASSISTANT == 0 || spec.flags & flags::API_AUDIO == 0 {
            return None;
        }
        assert_eq!(self.recv().await, Msg::ListEntitiesRequest);
        if spec.media_player {
            self.send(Msg::ListEntitiesMediaPlayer {
                object_id: "speaker".into(),
                key: 77,
                name: "Speaker".into(),
            })
            .await;
        }
        self.send(Msg::ListEntitiesDone).await;
        Some(self.recv().await)
    }
}

struct Rig {
    listener: TcpListener,
    announcer: Announcer,
    asked: mpsc::UnboundedReceiver<String>,
    media_base: Option<String>,
    device_key: Option<[u8; 32]>,
}

/// Start the client against a fake device. `media` also starts the file server.
async fn rig(media: bool) -> Rig {
    rig_slow(media, 0).await
}

/// Like `rig`, with an assistant that takes `think_ms` to answer.
async fn rig_slow(media: bool, think_ms: u64) -> Rig {
    rig_keys(media, think_ms, None, None).await
}

/// Like `rig_slow`, with Noise keys: the one the client uses and the one the fake device expects.
async fn rig_keys(
    media: bool,
    think_ms: u64,
    client_key: Option<[u8; 32]>,
    device_key: Option<[u8; 32]>,
) -> Rig {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let announcer = Announcer::default();
    let (asks, mut rx) = mpsc::channel::<Ask>(4);
    let (asked_tx, asked) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Some(ask) = rx.recv().await {
            let _ = asked_tx.send(ask.text.clone());
            tokio::time::sleep(Duration::from_millis(think_ms)).await;
            let _ = ask.reply.send(Answer {
                say: "You have nothing.".into(),
                effects: vec![clawdot_core::assistant::Effect::VolumeSet(40)],
                ..Answer::default()
            });
        }
    });
    let mut media_base = None;
    let mut store = None;
    if media {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        let s = MediaStore::new(&base);
        tokio::spawn(serve_media(l, s.clone(), std::future::pending()));
        media_base = Some(base);
        store = Some(s);
    }
    let settings = Settings {
        announcer: announcer.clone(),
        dialog: DialogConfig {
            follow_up_secs: 1,
            ..DialogConfig::default()
        },
        ..Settings::default()
    };
    tokio::spawn(run_device(
        Device {
            name: "kitchen".into(),
            addr: addr.to_string(),
            psk: client_key,
        },
        store,
        Pipeline {
            stt: std::sync::Arc::new(Ears),
            tts: std::sync::Arc::new(Mouth),
            asks,
            settings,
        },
        quick(),
        std::future::pending(),
    ));
    Rig {
        listener,
        announcer,
        asked,
        media_base,
        device_key,
    }
}

impl Rig {
    /// Accept the client and finish its setup.
    async fn connect(&self, spec: Spec) -> (Conn, Option<Msg>) {
        let (s, _) = timeout(PATIENCE, self.listener.accept())
            .await
            .expect("the client dials the device")
            .unwrap();
        let link = match &self.device_key {
            Some(key) => noise::accept(s, key, "fake-dot").await.unwrap(),
            None => Link::plain(s),
        };
        let mut c = Conn { link };
        let sub = c.setup(spec).await;
        (c, sub)
    }

    /// Accept the client and finish a successful setup.
    async fn ready(&self, spec: Spec) -> Conn {
        let (c, sub) = self.connect(spec).await;
        assert_eq!(
            sub,
            Some(Msg::SubscribeVoice {
                subscribe: true,
                flags: 1
            })
        );
        // The pipeline session registers shortly after setup.
        for _ in 0..200 {
            if self.announcer.connected() == 1 {
                return c;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("the pipeline session never started");
    }
}

fn pcm_bytes(samples: &[i16]) -> Vec<u8> {
    samples.iter().flat_map(|s| s.to_le_bytes()).collect()
}

/// 200 ms of silence, 600 ms of tone, then `tail_ms` of silence, as 1024-byte messages.
fn utterance(tail_ms: usize) -> Vec<Vec<u8>> {
    let mut v = vec![0i16; 3200];
    v.extend((0..9600).map(|i| (3000.0 * (f64::from(i) * 0.35).sin()) as i16));
    v.extend(vec![0i16; tail_ms * 16]);
    pcm_bytes(&v).chunks(1024).map(<[u8]>::to_vec).collect()
}

async fn speak(c: &mut Conn, tail_ms: usize, end: bool) {
    for chunk in utterance(tail_ms) {
        c.send(Msg::VoiceAudio {
            data: chunk,
            end: false,
        })
        .await;
    }
    if end {
        c.send(Msg::VoiceAudio {
            data: vec![],
            end: true,
        })
        .await;
    }
}

fn start_run() -> Msg {
    Msg::VoiceRequest {
        start: true,
        conversation_id: String::new(),
        flags: 3,
        wake_word_phrase: "okay nabu".into(),
    }
}

/// Collect the event kinds and reply audio up to and including `RUN_END`.
async fn until_run_end(c: &mut Conn) -> (Vec<u32>, Vec<u8>) {
    let (mut kinds, mut audio) = (Vec::new(), Vec::new());
    loop {
        match c.recv().await {
            Msg::VoiceEvent { kind, .. } => {
                kinds.push(kind);
                if kind == event::RUN_END {
                    return (kinds, audio);
                }
            }
            Msg::VoiceAudio { data, .. } => audio.extend(data),
            _ => {}
        }
    }
}

async fn http_get(url: &str) -> Vec<u8> {
    let rest = url.strip_prefix("http://").unwrap();
    let (host, path) = rest.split_once('/').unwrap();
    let mut s = TcpStream::connect(host).await.unwrap();
    s.write_all(format!("GET /{path} HTTP/1.1\r\nHost: {host}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut out = Vec::new();
    s.read_to_end(&mut out).await.unwrap();
    let split = out.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
    assert!(out.starts_with(b"HTTP/1.1 200"), "{:?}", &out[..split]);
    out[split..].to_vec()
}

// REQ-ESP-005, REQ-ESP-006
#[tokio::test]
async fn req_esp_005_the_client_sets_up_and_subscribes() {
    let rig = rig(false).await;
    let mut c = rig.ready(Spec::full()).await;
    // Pings are answered both ways: the client pings us, and we ping it.
    c.send(Msg::PingRequest).await;
    c.expect("a ping response", |m| *m == Msg::PingResponse)
        .await;
    timeout(PATIENCE, async {
        while c.link.recv().await.unwrap() != Some(Msg::PingRequest) {}
    })
    .await
    .expect("the client pings the device");
}

// REQ-ESP-005, REQ-ESP-012
#[tokio::test]
async fn req_esp_005_a_version_mismatch_or_missing_feature_is_refused_and_retried() {
    let rig = rig(false).await;
    for spec in [
        Spec {
            major: 2,
            ..Spec::full()
        },
        Spec {
            flags: 0,
            ..Spec::full()
        },
        Spec {
            flags: flags::VOICE_ASSISTANT,
            ..Spec::full()
        },
    ] {
        let (mut c, sub) = rig.connect(spec).await;
        assert_eq!(sub, None);
        // The client hangs up without subscribing, and dials again.
        let end = timeout(PATIENCE, async {
            loop {
                match c.link.recv().await {
                    Ok(Some(Msg::PingRequest)) => {}
                    Ok(Some(m)) => panic!("unexpected {m:?}"),
                    Ok(None) | Err(_) => return,
                }
            }
        })
        .await;
        assert!(end.is_ok(), "the client closed the connection");
    }
    // A good device is then accepted.
    rig.ready(Spec::full()).await;
}

// REQ-ESP-007, REQ-ESP-008, REQ-ESP-013
#[tokio::test]
async fn req_esp_007_audio_becomes_a_transcript_and_the_reply_comes_back_as_audio() {
    let mut rig = rig(false).await;
    let mut c = rig.ready(Spec::full()).await;
    c.send(start_run()).await;
    assert_eq!(
        c.recv().await,
        Msg::VoiceResponse {
            port: 0,
            error: false
        }
    );
    speak(&mut c, 1200, false).await;
    let (kinds, audio) = until_run_end(&mut c).await;
    assert_eq!(
        kinds,
        [
            event::RUN_START,
            event::STT_START,
            event::STT_VAD_END,
            event::STT_END,
            event::INTENT_START,
            event::INTENT_END,
            event::TTS_START,
            event::TTS_STREAM_START,
            event::TTS_STREAM_END,
            event::TTS_END,
            event::RUN_END
        ]
    );
    assert_eq!(rig.asked.recv().await.unwrap(), "what is on my plate");
    assert_eq!(
        audio.len(),
        "You have nothing.".len() * 50 * 2,
        "the whole reply arrives as 16 kHz PCM"
    );
    assert!(audio.chunks(2).all(|s| s == 9i16.to_le_bytes()));
}

// REQ-ESP-007: a device that ends its stream without waiting for us still gets an answer.
#[tokio::test]
async fn req_esp_007_an_end_marker_closes_the_utterance() {
    let mut rig = rig(false).await;
    let mut c = rig.ready(Spec::full()).await;
    c.send(start_run()).await;
    speak(&mut c, 0, true).await;
    let (kinds, _) = until_run_end(&mut c).await;
    assert!(kinds.contains(&event::TTS_STREAM_END), "{kinds:?}");
    assert_eq!(rig.asked.recv().await.unwrap(), "what is on my plate");
}

// REQ-ESP-013: pipeline commands reach the media player.
#[tokio::test]
async fn req_esp_013_a_volume_effect_reaches_the_media_player() {
    let rig = rig(false).await;
    let mut c = rig.ready(Spec::full()).await;
    c.send(start_run()).await;
    speak(&mut c, 1200, false).await;
    let m = c
        .expect("a volume command", |m| {
            matches!(m, Msg::MediaPlayerCommand { .. })
        })
        .await;
    assert_eq!(
        m,
        Msg::MediaPlayerCommand {
            key: 77,
            command: None,
            volume: Some(0.4),
            media_url: None,
            announcement: None
        }
    );
    let _ = player::PLAY;
}

// REQ-ESP-008: a device with no API speaker gets a URL.
#[tokio::test]
async fn req_esp_008_without_api_speaker_the_reply_is_a_url() {
    let rig = rig(true).await;
    let spec = Spec {
        flags: flags::VOICE_ASSISTANT | flags::API_AUDIO,
        ..Spec::full()
    };
    let mut c = rig.ready(spec).await;
    c.send(start_run()).await;
    speak(&mut c, 1200, false).await;
    let ev = c
        .expect(
            "TTS_END with a url",
            |m| matches!(m, Msg::VoiceEvent { kind, .. } if *kind == event::TTS_END),
        )
        .await;
    let Msg::VoiceEvent { data, .. } = ev else {
        panic!()
    };
    let url = &data.iter().find(|(k, _)| k == "url").unwrap().1;
    assert!(url.starts_with(rig.media_base.as_ref().unwrap()));
    let wav = http_get(url).await;
    assert_eq!(&wav[..4], b"RIFF");
    assert_eq!(wav.len(), 44 + "You have nothing.".len() * 50 * 2);
    let (kinds, _) = until_run_end(&mut c).await;
    assert_eq!(kinds.last(), Some(&event::RUN_END));
}

// REQ-ESP-010, REQ-ESP-011
#[tokio::test]
async fn req_esp_010_an_announcement_starts_a_conversation() {
    let mut rig = rig(true).await;
    let mut c = rig.ready(Spec::full()).await;
    assert!(rig.announcer.is_available().await);
    assert!(rig.announcer.announce("The build finished.").await);

    let Msg::AnnounceRequest {
        media_id,
        start_conversation,
        ..
    } = c
        .expect("an announce request", |m| {
            matches!(m, Msg::AnnounceRequest { .. })
        })
        .await
    else {
        panic!()
    };
    assert!(start_conversation, "the follow-up window is on");
    let wav = http_get(&media_id).await;
    assert_eq!(wav.len(), 44 + "The build finished.".len() * 50 * 2);

    // The device plays it, reports that, and starts the conversation.
    c.send(Msg::AnnounceFinished { success: true }).await;
    c.send(start_run()).await;
    speak(&mut c, 1200, false).await;
    let (kinds, _) = until_run_end(&mut c).await;
    assert!(kinds.contains(&event::TTS_STREAM_END), "{kinds:?}");
    assert_eq!(
        rig.asked.recv().await.unwrap(),
        "what is on my plate",
        "the answer to the announcement reaches the assistant"
    );
}

// REQ-ESP-011: a device that never starts the conversation does not block later announcements.
#[tokio::test]
async fn req_esp_011_an_unanswered_window_closes() {
    let rig = rig(true).await;
    let mut c = rig.ready(Spec::full()).await;
    assert!(rig.announcer.announce("First.").await);
    c.expect("an announce request", |m| {
        matches!(m, Msg::AnnounceRequest { .. })
    })
    .await;
    c.send(Msg::AnnounceFinished { success: true }).await;
    // The follow-up window is one second; afterwards the pipeline is idle again.
    let mut ok = false;
    for _ in 0..100 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        // Keep the device side alive while waiting.
        if rig.announcer.is_available().await {
            ok = true;
            break;
        }
        let _ = timeout(Duration::from_millis(1), c.recv()).await;
    }
    assert!(ok, "the pipeline became idle");
    assert!(rig.announcer.announce("Second.").await);
    c.expect("a second announce request", |m| {
        matches!(m, Msg::AnnounceRequest { .. })
    })
    .await;
}

// REQ-ESP-010: a device without the announce flag gets a media-player command.
#[tokio::test]
async fn req_esp_010_without_announce_support_the_media_player_is_used() {
    let rig = rig(true).await;
    let spec = Spec {
        flags: flags::VOICE_ASSISTANT | flags::API_AUDIO | flags::SPEAKER,
        ..Spec::full()
    };
    let mut c = rig.ready(spec).await;
    assert!(rig.announcer.announce("Hello there.").await);
    let Msg::MediaPlayerCommand {
        key,
        media_url,
        announcement,
        ..
    } = c
        .expect("a media command", |m| {
            matches!(m, Msg::MediaPlayerCommand { .. })
        })
        .await
    else {
        panic!()
    };
    assert_eq!((key, announcement), (77, Some(true)));
    assert!(!http_get(&media_url.unwrap()).await.is_empty());
}

// REQ-ESP-010: with no file server an announcement is declined without wedging the pipeline.
#[tokio::test]
async fn req_esp_010_no_file_server_means_no_announcement_but_no_hang() {
    let rig = rig(false).await;
    let mut c = rig.ready(Spec::full()).await;
    assert!(rig.announcer.announce("Anyone there?").await);
    // Nothing is sent; the pipeline returns to idle after the follow-up window.
    let mut idle = false;
    for _ in 0..100 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if rig.announcer.is_available().await {
            idle = true;
            break;
        }
        let _ = timeout(Duration::from_millis(1), c.recv()).await;
    }
    assert!(idle);
}

// REQ-ESP-009
#[tokio::test]
async fn req_esp_009_an_aborted_run_leaves_the_pipeline_idle() {
    let rig = rig(true).await;
    let mut c = rig.ready(Spec::full()).await;
    c.send(start_run()).await;
    c.expect("the response", |m| matches!(m, Msg::VoiceResponse { .. }))
        .await;
    assert!(!rig.announcer.is_available().await, "listening is busy");
    c.send(Msg::VoiceRequest {
        start: false,
        conversation_id: String::new(),
        flags: 0,
        wake_word_phrase: String::new(),
    })
    .await;
    let mut idle = false;
    for _ in 0..50 {
        if rig.announcer.is_available().await {
            idle = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(idle, "the pipeline stopped listening");
}

// REQ-ESP-007: a pipeline that is busy refuses the run instead of hanging the device.
#[tokio::test]
async fn req_esp_007_a_run_the_pipeline_cannot_start_is_ended() {
    let rig = rig_slow(false, 1500).await;
    let mut c = rig.ready(Spec::full()).await;
    c.send(start_run()).await;
    speak(&mut c, 1200, false).await;
    c.expect(
        "the thinking event",
        |m| matches!(m, Msg::VoiceEvent { kind, .. } if *kind == event::INTENT_START),
    )
    .await;
    // The assistant is still working, so a new run cannot start listening.
    c.send(start_run()).await;
    let (kinds, _) = until_run_end(&mut c).await;
    assert!(kinds.contains(&event::ERROR), "{kinds:?}");
}

// REQ-ESP-012
#[tokio::test]
async fn req_esp_012_a_lost_connection_is_redialled_with_a_fresh_session() {
    let mut rig = rig(false).await;
    let mut c = rig.ready(Spec::full()).await;
    c.send(start_run()).await;
    drop(c); // the device vanishes in the middle of a run
    let mut c2 = rig.ready(Spec::full()).await;
    c2.send(start_run()).await;
    speak(&mut c2, 1200, false).await;
    let (kinds, _) = until_run_end(&mut c2).await;
    assert!(kinds.contains(&event::INTENT_START), "{kinds:?}");
    assert_eq!(rig.asked.recv().await.unwrap(), "what is on my plate");
}

// REQ-ESP-003, REQ-ESP-004, REQ-ESP-012
#[tokio::test]
async fn req_esp_003_malformed_frames_end_the_connection_and_it_is_redialled() {
    let rig = rig(false).await;
    let oversize = {
        let mut f = vec![0u8];
        f.extend_from_slice(&[0xff, 0xff, 0x7f, 0x07]); // length about 2 MiB, type 7
        f
    };
    // A wrong first byte, an oversize length, a truncated varint that never ends, a known type with a bad
    // body, and a Noise marker on a plaintext link.
    let bad: Vec<Vec<u8>> = vec![
        vec![0x47, 0x00, 0x00],
        oversize,
        vec![0x00, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x01],
        plain_frame(90, &[0x0a, 0x05, b'a']),
        plain_frame(2, &[0x22, 0x02, 0xff, 0xfe]),
        vec![0x01, 0x00, 0x00],
    ];
    for frame in bad {
        let mut c = rig.ready(Spec::full()).await;
        c.link.io_mut().write_all(&frame).await.unwrap();
        // The client hangs up (it may have sent a ping or two first) and dials again.
        let closed = timeout(PATIENCE, async {
            let mut sink = [0u8; 256];
            loop {
                match c.link.io_mut().read(&mut sink).await {
                    Ok(0) | Err(_) => return,
                    Ok(_) => {}
                }
            }
        })
        .await;
        assert!(closed.is_ok(), "the client closed the connection");
    }
    // After all of that, a well-behaved device still works.
    let mut c = rig.ready(Spec::full()).await;
    c.send(Msg::PingRequest).await;
    c.expect("a ping response", |m| *m == Msg::PingResponse)
        .await;
}

// REQ-ESP-014: the whole voice run works over the encrypted link.
#[tokio::test]
async fn req_esp_014_a_voice_run_works_over_noise() {
    let mut rig = rig_keys(false, 0, Some([5; 32]), Some([5; 32])).await;
    let mut c = rig.ready(Spec::full()).await;
    c.send(start_run()).await;
    speak(&mut c, 1200, false).await;
    let (kinds, audio) = until_run_end(&mut c).await;
    assert!(kinds.contains(&event::TTS_STREAM_END), "{kinds:?}");
    assert_eq!(audio.len(), "You have nothing.".len() * 50 * 2);
    assert_eq!(rig.asked.recv().await.unwrap(), "what is on my plate");
}

// REQ-ESP-014: a wrong key never gets as far as a voice session, and the client keeps trying.
#[tokio::test]
async fn req_esp_014_a_wrong_key_is_refused_and_retried() {
    let rig = rig_keys(false, 0, Some([5; 32]), Some([6; 32])).await;
    for _ in 0..2 {
        let (s, _) = timeout(PATIENCE, rig.listener.accept())
            .await
            .unwrap()
            .unwrap();
        let e = noise::accept(s, &[6; 32], "fake-dot").await.unwrap_err();
        assert!(e.contains("MAC"), "{e}");
    }
    assert_eq!(rig.announcer.connected(), 0);
}

// REQ-ESP-014, REQ-ESP-003: a device that is not encrypted, dialled with a key, is refused.
#[tokio::test]
async fn req_esp_014_a_plaintext_device_is_refused_when_a_key_is_set() {
    let rig = rig_keys(false, 0, Some([5; 32]), None).await;
    let (mut s, _) = timeout(PATIENCE, rig.listener.accept())
        .await
        .unwrap()
        .unwrap();
    // The client's empty hello arrives; the device answers in plaintext.
    let mut hello = [0u8; 3];
    s.read_exact(&mut hello).await.unwrap();
    assert_eq!(hello, [1, 0, 0]);
    s.write_all(&plain_frame(2, &[])).await.unwrap();
    // The client gives up and redials.
    let again = timeout(PATIENCE, rig.listener.accept()).await;
    assert!(again.is_ok(), "the client dials again");
}

// REQ-ESP-004: a message of a type the client does not use is ignored, not fatal.
#[tokio::test]
async fn req_esp_004_unknown_message_types_are_ignored() {
    let rig = rig(false).await;
    let mut c = rig.ready(Spec::full()).await;
    c.link
        .io_mut()
        .write_all(&plain_frame(4242, &[0x08, 0x01]))
        .await
        .unwrap();
    c.send(Msg::PingRequest).await;
    c.expect("a ping response", |m| *m == Msg::PingResponse)
        .await;
}
