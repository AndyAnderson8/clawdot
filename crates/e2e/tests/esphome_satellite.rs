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
use tokio::sync::{mpsc, watch};
use tokio::time::timeout;

const PATIENCE: Duration = Duration::from_secs(20);
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
        connect: Duration::from_secs(10),
        setup_step: Duration::from_secs(10),
        ping_every: Duration::from_millis(300),
        dead_after: Duration::from_secs(10),
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
    rig_keys(media, None, None, None).await
}

/// Like `rig`, with an assistant that does not answer until `hold` says so: it is working until then, however
/// long the test takes to get there (a fixed delay would be a race against the machine's load).
async fn rig_held(media: bool, hold: watch::Receiver<bool>) -> Rig {
    rig_keys(media, Some(hold), None, None).await
}

/// Like `rig`, with Noise keys: the one the client uses and the one the fake device expects. `hold`, when
/// given, keeps the assistant from answering until it reads true.
async fn rig_keys(
    media: bool,
    hold: Option<watch::Receiver<bool>>,
    client_key: Option<[u8; 32]>,
    device_key: Option<[u8; 32]>,
) -> Rig {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let announcer = Announcer::default();
    let (asks, mut rx) = mpsc::channel::<Ask>(4);
    let (asked_tx, asked) = mpsc::unbounded_channel();
    let mut hold = hold;
    tokio::spawn(async move {
        while let Some(ask) = rx.recv().await {
            let _ = asked_tx.send(ask.text.clone());
            if let Some(hold) = hold.as_mut() {
                let _ = hold.wait_for(|open| *open).await;
            }
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
        timeout(PATIENCE, async {
            while self.announcer.connected() != 1 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the pipeline session never started");
        c
    }

    /// Wait until the pipeline is idle again (it can take an announcement), answering the client's pings
    /// meanwhile. Waits for the state, not for a number of polls.
    async fn until_idle(&self, c: &mut Conn, what: &str) {
        timeout(PATIENCE, async {
            loop {
                tokio::time::sleep(Duration::from_millis(100)).await;
                if self.announcer.is_available().await {
                    return;
                }
                let _ = timeout(Duration::from_millis(1), c.recv()).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{what}"));
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
    rig.until_idle(&mut c, "the pipeline became idle").await;
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
    rig.until_idle(&mut c, "the pipeline returned to idle")
        .await;
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
    rig.until_idle(&mut c, "the pipeline stopped listening")
        .await;
}

// REQ-ESP-007: a pipeline that is busy refuses the run instead of hanging the device.
#[tokio::test]
async fn req_esp_007_a_run_the_pipeline_cannot_start_is_ended() {
    // The assistant stays busy until the test lets it go. (A fixed think time left half a second between the
    // client's wake-up watch of one second and the end of the answer, which a loaded machine can eat.)
    let (release, hold) = watch::channel(false);
    let rig = rig_held(false, hold).await;
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
    release.send_replace(true);
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
    let mut rig = rig_keys(false, None, Some([5; 32]), Some([5; 32])).await;
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
    let rig = rig_keys(false, None, Some([5; 32]), Some([6; 32])).await;
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
    let rig = rig_keys(false, None, Some([5; 32]), None).await;
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

/// A reply that reaches the device while the pipeline is still making it (`reply: "streamed"`): the pipeline
/// speaks sentence by sentence, and the client relays each sentence at once. The speech provider here waits for
/// the test to let each sentence through, so the test decides when each synthesis finishes and no timing is
/// guessed.
mod streamed {
    use super::*;
    use clawdot_core::esphome::run_device_with;
    use clawdot_core::esphome_cfg::ReplyMode;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Semaphore;

    const S: [&str; 4] = [
        "The first sentence is here.",
        "The second sentence is here.",
        "The third sentence is here.",
        "The fourth sentence is here.",
    ];
    /// The size of the WAV header the media server writes.
    const HEADER: usize = 44;

    /// Bytes of audio the fake voice makes for a sentence.
    fn bytes_of(sentence: &str) -> usize {
        sentence.len() * 50 * 2
    }

    /// Bytes of audio for the first `n` sentences.
    fn upto(n: usize) -> usize {
        S[..n].iter().map(|s| bytes_of(s)).sum()
    }

    /// The audio of all four sentences: sentence `i` is made by call `i + 1`, whose samples have that value.
    fn all_audio() -> Vec<u8> {
        (0..4)
            .flat_map(|i| {
                std::iter::repeat_n(
                    i16::try_from(i + 1).unwrap().to_le_bytes(),
                    bytes_of(S[i]) / 2,
                )
                .flatten()
            })
            .collect()
    }

    /// A voice that reports each request and then waits for a permit. Call number `n` makes samples of value
    /// `n`, so the order of the audio can be read back. Call `fail_on` fails.
    struct Gated {
        asked: mpsc::UnboundedSender<String>,
        permits: Arc<Semaphore>,
        calls: AtomicUsize,
        fail_on: usize,
    }

    impl Synthesizer for Gated {
        async fn synthesize(&self, text: &str) -> Result<Vec<i16>, String> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            let _ = self.asked.send(text.to_owned());
            self.permits
                .acquire()
                .await
                .map_err(|e| e.to_string())?
                .forget();
            if n == self.fail_on {
                return Err("the voice failed".into());
            }
            Ok(vec![i16::try_from(n).unwrap(); text.len() * 50])
        }
    }

    struct Streams {
        rig: Rig,
        /// The sentences the voice is asked for, as it is asked.
        tts: mpsc::UnboundedReceiver<String>,
        permits: Arc<Semaphore>,
    }

    /// The client against a fake device, `mode` for its replies, an assistant that answers with all of `S`, and
    /// a voice that fails on call number `fail_on` (0: never).
    async fn streams(mode: ReplyMode, fail_on: usize) -> Streams {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let announcer = Announcer::default();
        let (asks, mut rx) = mpsc::channel::<Ask>(4);
        let (asked_tx, asked) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Some(ask) = rx.recv().await {
                let _ = asked_tx.send(ask.text.clone());
                let _ = ask.reply.send(Answer {
                    say: S.join(" "),
                    ..Answer::default()
                });
            }
        });
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        let store = MediaStore::new(&base);
        tokio::spawn(serve_media(l, store.clone(), std::future::pending()));
        let (tts_tx, tts) = mpsc::unbounded_channel();
        let permits = Arc::new(Semaphore::new(0));
        let voice = Gated {
            asked: tts_tx,
            permits: Arc::clone(&permits),
            calls: AtomicUsize::new(0),
            fail_on,
        };
        let settings = Settings {
            announcer: announcer.clone(),
            dialog: DialogConfig {
                follow_up_secs: 1,
                ..DialogConfig::default()
            },
            ..Settings::default()
        };
        tokio::spawn(run_device_with(
            Device {
                name: "kitchen".into(),
                addr: addr.to_string(),
                psk: None,
            },
            Some(store),
            Pipeline {
                stt: Arc::new(Ears),
                tts: Arc::new(voice),
                asks,
                settings,
            },
            quick(),
            mode,
            std::future::pending(),
        ));
        Streams {
            rig: Rig {
                listener,
                announcer,
                asked,
                media_base: Some(base),
                device_key: None,
            },
            tts,
            permits,
        }
    }

    /// A device with a speaker on the API connection, and one that must fetch a URL.
    fn api_device() -> Spec {
        Spec::full()
    }

    fn url_device() -> Spec {
        Spec {
            flags: flags::VOICE_ASSISTANT | flags::API_AUDIO,
            ..Spec::full()
        }
    }

    /// What the device saw of a reply, in order.
    #[derive(Debug, PartialEq)]
    enum Seen {
        Ev(u32),
        Pcm(Vec<u8>),
        End,
    }

    fn pcm_len(log: &[Seen]) -> usize {
        log.iter()
            .map(|s| if let Seen::Pcm(p) = s { p.len() } else { 0 })
            .sum()
    }

    fn pcm_of(log: &[Seen]) -> Vec<u8> {
        log.iter()
            .filter_map(|s| {
                if let Seen::Pcm(p) = s {
                    Some(p.clone())
                } else {
                    None
                }
            })
            .flatten()
            .collect()
    }

    fn has_event(log: &[Seen], kind: u32) -> bool {
        log.contains(&Seen::Ev(kind))
    }

    /// Read from the device's connection into `log` until `done` holds.
    async fn hear_until(c: &mut Conn, log: &mut Vec<Seen>, done: impl Fn(&[Seen]) -> bool) {
        while !done(log) {
            match c.recv().await {
                Msg::VoiceEvent { kind, .. } => log.push(Seen::Ev(kind)),
                Msg::VoiceAudio { end: true, .. } => log.push(Seen::End),
                Msg::VoiceAudio { data, .. } if !data.is_empty() => log.push(Seen::Pcm(data)),
                _ => {}
            }
        }
    }

    /// Like `hear_until`, for a device that is given a URL: returns it from the `TTS_END` event.
    async fn hear_url(c: &mut Conn, log: &mut Vec<Seen>) -> String {
        loop {
            match c.recv().await {
                Msg::VoiceEvent { kind, data } => {
                    log.push(Seen::Ev(kind));
                    if kind == event::TTS_END {
                        return data.iter().find(|(k, _)| k == "url").unwrap().1.clone();
                    }
                }
                Msg::VoiceAudio { .. } => panic!("a device without a speaker is sent no audio"),
                _ => {}
            }
        }
    }

    /// Start a run on the device and speak a question, ready for the reply.
    async fn ask(c: &mut Conn) {
        c.send(start_run()).await;
        speak(c, 1200, false).await;
    }

    fn stop_run() -> Msg {
        Msg::VoiceRequest {
            start: false,
            conversation_id: String::new(),
            flags: 0,
            wake_word_phrase: String::new(),
        }
    }

    /// One HTTP download of a live reply, read piece by piece.
    struct Download {
        s: TcpStream,
        buf: Vec<u8>,
        chunked: bool,
        head: String,
    }

    impl Download {
        /// None when the server closes the connection without answering.
        async fn open(url: &str, version: &str) -> Option<Self> {
            let rest = url.strip_prefix("http://").unwrap();
            let (host, path) = rest.split_once('/').unwrap();
            let mut s = TcpStream::connect(host).await.unwrap();
            s.write_all(format!("GET /{path} HTTP/{version}\r\nHost: {host}\r\n\r\n").as_bytes())
                .await
                .unwrap();
            let mut buf = Vec::new();
            loop {
                if let Some(at) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8(buf[..at].to_vec()).unwrap();
                    assert!(head.starts_with(&format!("HTTP/{version} 200")), "{head}");
                    let chunked = head.contains("Transfer-Encoding: chunked");
                    buf.drain(..at + 4);
                    return Some(Self {
                        s,
                        buf,
                        chunked,
                        head,
                    });
                }
                let mut tmp = [0u8; 512];
                let n = timeout(PATIENCE, s.read(&mut tmp))
                    .await
                    .expect("the media server answers")
                    .unwrap_or(0);
                if n == 0 {
                    return None;
                }
                buf.extend_from_slice(&tmp[..n]);
            }
        }

        /// Open, trying again until the server has a free connection (a closed one is freed asynchronously).
        async fn open_when_free(url: &str) -> Self {
            timeout(PATIENCE, async {
                loop {
                    if let Some(d) = Self::open(url, "1.1").await {
                        return d;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("a connection slot became free")
        }

        async fn more(&mut self) -> usize {
            let mut tmp = [0u8; 4096];
            let n = timeout(PATIENCE, self.s.read(&mut tmp))
                .await
                .expect("the stream makes progress")
                .unwrap_or(0);
            self.buf.extend_from_slice(&tmp[..n]);
            n
        }

        /// The next piece of the body, or None when the response has ended.
        async fn piece(&mut self) -> Option<Vec<u8>> {
            if !self.chunked {
                if self.buf.is_empty() && self.more().await == 0 {
                    return None;
                }
                return Some(std::mem::take(&mut self.buf));
            }
            loop {
                if let Some(at) = self.buf.windows(2).position(|w| w == b"\r\n") {
                    let size =
                        usize::from_str_radix(std::str::from_utf8(&self.buf[..at]).unwrap(), 16)
                            .unwrap();
                    if size == 0 {
                        if self.buf.len() < at + 4 {
                            assert!(self.more().await > 0, "the terminator is complete");
                            continue;
                        }
                        assert_eq!(&self.buf[at..at + 4], b"\r\n\r\n");
                        return None;
                    }
                    if self.buf.len() >= at + 2 + size + 2 {
                        let body = self.buf[at + 2..at + 2 + size].to_vec();
                        assert_eq!(&self.buf[at + 2 + size..at + 4 + size], b"\r\n");
                        self.buf.drain(..at + 4 + size);
                        return Some(body);
                    }
                }
                assert!(
                    self.more().await > 0,
                    "the response ended in the middle of a chunk"
                );
            }
        }

        /// Read until at least `len` bytes of body have arrived.
        async fn body_upto(&mut self, got: &mut Vec<u8>, len: usize) {
            while got.len() < len {
                got.extend(self.piece().await.expect("the body is not over yet"));
            }
        }

        async fn body_to_end(&mut self, got: &mut Vec<u8>) {
            while let Some(p) = self.piece().await {
                got.extend(p);
            }
        }
    }

    // REQ-ESP-008: over the API connection the first sentence reaches the device after ONE synthesis, the
    // rest follow as they are made, in order, and the reply ends once, after the last.
    #[tokio::test]
    async fn req_esp_008_streamed_api_audio_starts_after_one_synthesis() {
        let mut t = streams(ReplyMode::Streamed, 0).await;
        let mut c = t.rig.ready(api_device()).await;
        ask(&mut c).await;
        let mut log = Vec::new();
        for (i, sentence) in S.iter().enumerate() {
            assert_eq!(
                t.tts.recv().await.unwrap(),
                *sentence,
                "sentence {i} is asked for"
            );
            if i == 1 {
                // Hold the second sentence for much longer than the playback margin (50 ms in these tests):
                // a gap while the next sentence is made must not be taken for the end of the reply.
                tokio::time::sleep(Duration::from_millis(300)).await;
            }
            t.permits.add_permits(1);
            hear_until(&mut c, &mut log, |l| pcm_len(l) >= upto(i + 1)).await;
            if i == 0 {
                assert!(has_event(&log, event::TTS_STREAM_START));
                assert!(
                    !has_event(&log, event::TTS_STREAM_END) && !log.contains(&Seen::End),
                    "{log:?}"
                );
            }
        }
        hear_until(&mut c, &mut log, |l| has_event(l, event::RUN_END)).await;

        // The audio is the four sentences in order, then the end, then the events that close the reply.
        assert_eq!(pcm_of(&log), all_audio());
        let tail: Vec<_> = log
            .iter()
            .skip_while(|s| !matches!(s, Seen::End))
            .map(|s| match s {
                Seen::Ev(k) => *k,
                _ => 0,
            })
            .collect();
        assert_eq!(
            tail,
            [0, event::TTS_STREAM_END, event::TTS_END, event::RUN_END],
            "{log:?}"
        );
        let starts = log
            .iter()
            .filter(|s| **s == Seen::Ev(event::TTS_STREAM_START))
            .count();
        assert_eq!(starts, 1);
        let first_pcm = log.iter().position(|s| matches!(s, Seen::Pcm(_))).unwrap();
        let start = log
            .iter()
            .position(|s| *s == Seen::Ev(event::TTS_STREAM_START))
            .unwrap();
        assert!(start < first_pcm);

        // The follow-up window opened (once, after the last sentence).
        let mut opened = false;
        for _ in 0..300 {
            if !t.rig.announcer.is_available().await {
                opened = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(opened, "the follow-up window opened after the reply");
        assert!(t.tts.try_recv().is_err(), "nothing more was synthesised");
    }

    // REQ-ESP-008, REQ-ESP-010: for a device that fetches a URL, the URL is given once the first sentence is
    // ready and the body is the WAV header then the sentences in order, ending cleanly with the reply.
    #[tokio::test]
    async fn req_esp_008_streamed_url_is_fetched_while_the_reply_is_made() {
        let mut t = streams(ReplyMode::Streamed, 0).await;
        let mut c = t.rig.ready(url_device()).await;
        ask(&mut c).await;
        let mut log = Vec::new();
        assert_eq!(t.tts.recv().await.unwrap(), S[0]);
        t.permits.add_permits(1);
        let url = hear_url(&mut c, &mut log).await;
        assert!(url.starts_with(t.rig.media_base.as_ref().unwrap()));
        let mut dl = Download::open(&url, "1.1").await.unwrap();
        assert!(!dl.head.contains("Content-Length"), "{}", dl.head);
        assert!(dl.head.contains("audio/wav"), "{}", dl.head);
        let mut body = Vec::new();
        dl.body_upto(&mut body, HEADER + upto(1)).await;
        assert_eq!(&body[..4], b"RIFF");
        assert_eq!(
            &body[4..8],
            &u32::MAX.to_le_bytes(),
            "no length is declared"
        );
        assert_eq!(
            &body[40..44],
            &u32::MAX.to_le_bytes(),
            "no data length is declared"
        );
        assert!(!has_event(&log, event::RUN_END));

        for (i, sentence) in S.iter().enumerate().skip(1) {
            assert_eq!(t.tts.recv().await.unwrap(), *sentence);
            t.permits.add_permits(1);
            dl.body_upto(&mut body, HEADER + upto(i + 1)).await;
        }
        dl.body_to_end(&mut body).await;
        let want = all_audio();
        assert_eq!(body.len(), HEADER + want.len());
        assert_eq!(&body[HEADER..], &want[..]);
        hear_until(&mut c, &mut log, |l| has_event(l, event::RUN_END)).await;
        let kinds: Vec<_> = log
            .iter()
            .filter_map(|s| if let Seen::Ev(k) = s { Some(*k) } else { None })
            .collect();
        assert_eq!(
            &kinds[kinds.len() - 3..],
            [event::TTS_START, event::TTS_END, event::RUN_END],
            "the URL was announced once, with the first sentence: {kinds:?}"
        );
    }

    // REQ-ESP-008: an HTTP/1.0 fetcher gets the same body, delimited by the end of the connection.
    #[tokio::test]
    async fn req_esp_008_streamed_url_without_chunking_for_http_1_0() {
        let t = streams(ReplyMode::Streamed, 0).await;
        let mut c = t.rig.ready(url_device()).await;
        ask(&mut c).await;
        t.permits.add_permits(100);
        let url = hear_url(&mut c, &mut Vec::new()).await;
        let mut dl = Download::open(&url, "1.0").await.unwrap();
        assert!(!dl.head.contains("chunked"), "{}", dl.head);
        let mut body = Vec::new();
        dl.body_to_end(&mut body).await;
        assert_eq!(body.len(), HEADER + upto(4));
    }

    // REQ-ESP-009, REQ-VOI-020: the device ends the run in the middle of a reply (its stop button). The audio
    // stops, and no sentence that was not yet made is asked for.
    #[tokio::test]
    async fn req_esp_009_a_stop_in_the_middle_of_a_streamed_api_reply_stops_the_synthesis() {
        let mut t = streams(ReplyMode::Streamed, 0).await;
        let mut c = t.rig.ready(api_device()).await;
        ask(&mut c).await;
        let mut log = Vec::new();
        assert_eq!(t.tts.recv().await.unwrap(), S[0]);
        t.permits.add_permits(1);
        hear_until(&mut c, &mut log, |l| pcm_len(l) >= upto(1)).await;
        // The second sentence is being made; the device gives up.
        assert_eq!(t.tts.recv().await.unwrap(), S[1]);
        c.send(stop_run()).await;

        // A new run gets a whole new reply. Its first sentence is the next thing the voice is asked for, so the
        // old reply asked for nothing more, and the device hears nothing of the old one after the stop. (No
        // permit is given until then: the stopped synthesis must not be able to finish first.)
        c.send(start_run()).await;
        speak(&mut c, 1200, false).await;
        assert_eq!(t.tts.recv().await.unwrap(), S[0]);
        t.permits.add_permits(100);
        let mut again = Vec::new();
        hear_until(&mut c, &mut again, |l| has_event(l, event::RUN_END)).await;
        assert_eq!(pcm_len(&again), upto(4));
        assert!(
            pcm_of(&again)
                .chunks(2)
                .all(|s| i16::from_le_bytes([s[0], s[1]]) >= 3),
            "no audio of the stopped reply arrived"
        );
    }

    // REQ-ESP-009: the same for a reply the device is fetching: the download ends cleanly with what was made.
    #[tokio::test]
    async fn req_esp_009_a_stop_in_the_middle_of_a_streamed_url_reply_ends_the_download() {
        let mut t = streams(ReplyMode::Streamed, 0).await;
        let mut c = t.rig.ready(url_device()).await;
        ask(&mut c).await;
        assert_eq!(t.tts.recv().await.unwrap(), S[0]);
        t.permits.add_permits(1);
        let url = hear_url(&mut c, &mut Vec::new()).await;
        let mut dl = Download::open(&url, "1.1").await.unwrap();
        let mut body = Vec::new();
        dl.body_upto(&mut body, HEADER + upto(1)).await;
        assert_eq!(t.tts.recv().await.unwrap(), S[1]);
        c.send(stop_run()).await;
        dl.body_to_end(&mut body).await;
        assert_eq!(
            body.len(),
            HEADER + upto(1),
            "only the first sentence was sent"
        );
        c.send(start_run()).await;
        speak(&mut c, 1200, false).await;
        assert_eq!(
            t.tts.recv().await.unwrap(),
            S[0],
            "the stopped reply asked for nothing more"
        );
        t.permits.add_permits(100);
    }

    // REQ-VOI-006: a synthesis that fails on the third sentence ends the reply after the second, cleanly.
    #[tokio::test]
    async fn req_esp_008_a_failed_synthesis_ends_the_streamed_api_reply_cleanly() {
        let mut t = streams(ReplyMode::Streamed, 3).await;
        let mut c = t.rig.ready(api_device()).await;
        ask(&mut c).await;
        let mut log = Vec::new();
        for i in 0..2 {
            // Each of the first two sentences is on the device before the next is allowed to be made.
            t.permits.add_permits(1);
            hear_until(&mut c, &mut log, |l| pcm_len(l) >= upto(i + 1)).await;
        }
        t.permits.add_permits(100); // the third fails
        hear_until(&mut c, &mut log, |l| has_event(l, event::RUN_END)).await;
        assert_eq!(pcm_len(&log), upto(2), "sentences one and two were played");
        assert!(
            has_event(&log, event::TTS_STREAM_END) && has_event(&log, event::TTS_END),
            "{log:?}"
        );
        assert!(!has_event(&log, event::ERROR), "no error cue: {log:?}");
        // Nothing was asked after the failure.
        let mut asked = Vec::new();
        while let Ok(s) = t.tts.try_recv() {
            asked.push(s);
        }
        assert_eq!(asked, S[..3]);
    }

    #[tokio::test]
    async fn req_esp_008_a_failed_synthesis_ends_the_streamed_url_reply_cleanly() {
        let t = streams(ReplyMode::Streamed, 3).await;
        let mut c = t.rig.ready(url_device()).await;
        ask(&mut c).await;
        t.permits.add_permits(1);
        let url = hear_url(&mut c, &mut Vec::new()).await;
        let mut dl = Download::open(&url, "1.1").await.unwrap();
        let mut body = Vec::new();
        dl.body_upto(&mut body, HEADER + upto(1)).await;
        t.permits.add_permits(1);
        dl.body_upto(&mut body, HEADER + upto(2)).await;
        t.permits.add_permits(100); // the third fails
        dl.body_to_end(&mut body).await;
        assert_eq!(body.len(), HEADER + upto(2));
        let mut log = Vec::new();
        hear_until(&mut c, &mut log, |l| has_event(l, event::RUN_END)).await;
        assert!(!has_event(&log, event::ERROR), "{log:?}");
    }

    // REQ-ESP-010: a device that drops its download in the middle of a reply frees its connection slot, even
    // while no audio is arriving; one that drops the API connection ends the download.
    #[tokio::test]
    async fn req_esp_010_a_device_that_disconnects_mid_stream_frees_its_slot() {
        let mut t = streams(ReplyMode::Streamed, 0).await;
        let mut c = t.rig.ready(url_device()).await;
        ask(&mut c).await;
        assert_eq!(t.tts.recv().await.unwrap(), S[0]);
        t.permits.add_permits(1);
        let url = hear_url(&mut c, &mut Vec::new()).await;
        // The media server serves four at once. Take all four, and let go of them mid-reply (sentence two is
        // being made, so the downloads are idle).
        let mut holders = Vec::new();
        for _ in 0..4 {
            let mut d = Download::open_when_free(&url).await;
            let mut got = Vec::new();
            d.body_upto(&mut got, HEADER + upto(1)).await;
            holders.push(d);
        }
        assert_eq!(t.tts.recv().await.unwrap(), S[1]);
        drop(holders);
        let mut d = Download::open_when_free(&url).await;
        let mut got = Vec::new();
        d.body_upto(&mut got, HEADER + upto(1)).await;

        // Now the device itself goes away: its download ends with what had been made.
        drop(c);
        d.body_to_end(&mut got).await;
        assert_eq!(got.len(), HEADER + upto(1));
        // And it can come back.
        t.permits.add_permits(100);
        let mut c2 = t.rig.ready(url_device()).await;
        ask(&mut c2).await;
        let again = hear_url(&mut c2, &mut Vec::new()).await;
        assert_ne!(again, url);
    }

    // REQ-ESP-008: the way a reply is delivered is the device's own setting, and `whole` is how it always was.
    #[tokio::test]
    async fn req_esp_008_whole_replies_arrive_complete_after_every_sentence_is_made() {
        let mut t = streams(ReplyMode::Whole, 0).await;
        let mut c = t.rig.ready(api_device()).await;
        t.permits.add_permits(100);
        ask(&mut c).await;
        let mut log = Vec::new();
        hear_until(&mut c, &mut log, |l| has_event(l, event::RUN_END)).await;
        assert_eq!(pcm_len(&log), upto(4));
        let start = log
            .iter()
            .position(|s| *s == Seen::Ev(event::TTS_STREAM_START))
            .unwrap();
        assert!(log[..start].contains(&Seen::Ev(event::TTS_START)));
        let mut asked = 0;
        while t.tts.try_recv().is_ok() {
            asked += 1;
        }
        assert_eq!(asked, 4);
    }
}
