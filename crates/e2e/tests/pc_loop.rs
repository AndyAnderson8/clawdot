//! The PC voice loop with no audio hardware: an always-on microphone, a wake word, an utterance, a reply and
//! back to listening (REQ-DEV-025..028). The microphone is a queue the test fills, the wake engine is a
//! scripted one that "hears" a frame of one fixed value, the speaker keeps what it is given. Nothing here opens
//! a sound device.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use clawdot_core::assistant::Effect;
use clawdot_core::satellite::{Answer, Ask, Settings, serve};
use clawdot_core::voice::{Config as DialogConfig, Synthesizer, Transcriber};
use clawdot_e2e::FakeCore;
use clawdot_protocol::message::{AudioFormat, Message};
use clawdot_satellite::audio::{AudioError, AudioIn, AudioOut, FRAME_SAMPLES};
use clawdot_satellite::front::{AudioDevice, DeviceConfig, NoTrigger, ScriptedWake};
use clawdot_satellite::{Config, run};
use std::collections::VecDeque;
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};
use tokio::net::TcpListener;
use tokio::sync::{mpsc as tmpsc, oneshot};

// ---- fakes ---------------------------------------------------------------------------------------------------

#[derive(Debug, Default)]
struct MicState {
    open: bool,
    starts: usize,
    frames: VecDeque<Vec<i16>>,
    fail: Option<AudioError>,
}

/// A microphone the test fills.
#[derive(Debug, Clone, Default)]
struct Mic(Arc<Mutex<MicState>>);

impl Mic {
    fn state(&self) -> std::sync::MutexGuard<'_, MicState> {
        self.0.lock().unwrap()
    }
    fn push(&self, frame: Vec<i16>) {
        self.state().frames.push_back(frame);
    }
    /// Push samples as 20 ms frames, a few at a time, like a live source.
    fn paced(&self, pcm: &[i16]) {
        for batch in pcm.chunks(FRAME_SAMPLES * 5) {
            for f in batch.chunks(FRAME_SAMPLES) {
                let mut f = f.to_vec();
                f.resize(FRAME_SAMPLES, 0);
                self.push(f);
            }
            thread::sleep(Duration::from_millis(4));
        }
    }
    fn open(&self) -> bool {
        self.state().open
    }
    fn starts(&self) -> usize {
        self.state().starts
    }
    fn fail_with(&self, e: AudioError) {
        self.state().fail = Some(e);
    }
}

impl AudioIn for Mic {
    fn start(&mut self) -> Result<(), AudioError> {
        let mut s = self.state();
        if !s.open {
            s.open = true;
            s.starts += 1;
        }
        Ok(())
    }
    fn read_frame(&mut self, _wait: Duration) -> Result<Option<Vec<i16>>, AudioError> {
        let mut s = self.state();
        if !s.open {
            return Err(AudioError::NotStarted);
        }
        if let Some(e) = s.fail.take() {
            return Err(e);
        }
        Ok(s.frames.pop_front())
    }
    fn stop(&mut self) {
        self.state().open = false;
    }
}

/// A speaker that keeps what it is given and can be made to take its time.
#[derive(Debug, Clone, Default)]
struct Speaker {
    samples: Arc<Mutex<Vec<i16>>>,
    replies: Arc<AtomicUsize>,
    hold: Arc<AtomicBool>,
    finishing: Arc<AtomicBool>,
}

impl AudioOut for Speaker {
    fn begin(&mut self) -> Result<(), AudioError> {
        Ok(())
    }
    fn play(&mut self, pcm: &[i16]) -> Result<(), AudioError> {
        self.samples.lock().unwrap().extend_from_slice(pcm);
        Ok(())
    }
    fn finish(&mut self) {
        self.finishing.store(true, Ordering::SeqCst);
    }
    fn poll_done(&mut self) -> Result<bool, AudioError> {
        if self.hold.load(Ordering::SeqCst) {
            return Ok(false);
        }
        if self.finishing.swap(false, Ordering::SeqCst) {
            self.replies.fetch_add(1, Ordering::SeqCst);
        }
        Ok(true)
    }
    fn stop(&mut self) {
        self.finishing.store(false, Ordering::SeqCst);
    }
}

fn until(what: &str, cond: impl Fn() -> bool) {
    let end = Instant::now() + Duration::from_secs(10);
    while !cond() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(10));
    }
}

fn quiet(frames: usize) -> Vec<i16> {
    vec![7; frames * FRAME_SAMPLES]
}

/// A question: lead-in silence, a second of tone, then the pause that ends it.
fn speech() -> Vec<i16> {
    let mut v = vec![0i16; 3200];
    v.extend((0..9600).map(|i| (3000.0 * (f64::from(i) * 0.35).sin()) as i16));
    v.extend(std::iter::repeat_n(0i16, 24_000));
    v
}

fn cfg() -> Config {
    Config {
        device_id: "e2e-pc-loop".into(),
        token: None,
        wake_on_connect: false,
        dead_after: clawdot_satellite::DEAD_AFTER,
    }
}

fn live_config(max_turns: Option<usize>) -> DeviceConfig {
    DeviceConfig {
        always_on: true,
        max_turns,
        ..DeviceConfig::default()
    }
}

// ---- the real core ---------------------------------------------------------------------------------------------

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

fn start_core() -> (SocketAddr, oneshot::Sender<()>, mpsc::Receiver<String>) {
    let (addr_tx, addr_rx) = mpsc::channel();
    let (asked_tx, asked_rx) = mpsc::channel();
    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            addr_tx.send(listener.local_addr().unwrap()).unwrap();
            let (asks, mut rx) = tmpsc::channel::<Ask>(4);
            tokio::spawn(async move {
                while let Some(ask) = rx.recv().await {
                    asked_tx.send(ask.text.clone()).unwrap();
                    let _ = ask.reply.send(Answer {
                        say: "Nothing is waiting.".to_owned(),
                        effects: vec![Effect::VolumeSet(40)],
                        ..Answer::default()
                    });
                }
            });
            let _ = serve(
                listener,
                Arc::new(Ears),
                Arc::new(Mouth),
                asks,
                Settings {
                    dialog: DialogConfig {
                        follow_up_secs: 0,
                        ..DialogConfig::default()
                    },
                    ..Settings::default()
                },
                async {
                    let _ = stop_rx.await;
                },
            )
            .await;
        });
    });
    (addr_rx.recv().unwrap(), stop_tx, asked_rx)
}

#[test]
fn req_dev_025_wake_then_question_then_reply_then_listening_again_through_the_real_core() {
    let (addr, stop, asked) = start_core();
    let (mic, spk, wake) = (Mic::default(), Speaker::default(), ScriptedWake::default());
    let mut dev = AudioDevice::new(mic.clone(), spk.clone(), NoTrigger, live_config(Some(2)))
        .with_wake(wake.clone());
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let r = run(TcpStream::connect(addr).unwrap(), &mut dev, &cfg());
        let _ = tx.send((r, dev));
    });

    // A quiet room: a second of noise is not the wake word, and the core hears nothing.
    mic.paced(&quiet(50));
    until("the microphone to be opened", || mic.open());
    thread::sleep(Duration::from_millis(100));
    assert_eq!(wake.stats.lock().unwrap().heard, 0);
    assert!(asked.try_recv().is_err(), "nothing was asked");

    // Turn one. The reply is held so that a wake word can be spoken over it.
    spk.hold.store(true, Ordering::SeqCst);
    mic.push(ScriptedWake::frame());
    mic.paced(&speech());
    assert_eq!(
        asked.recv_timeout(Duration::from_secs(10)).unwrap(),
        "what is on my plate",
        "the question after the wake word reached the assistant"
    );
    until("the reply to start playing", || {
        !spk.samples.lock().unwrap().is_empty()
    });
    let heard = wake.stats.lock().unwrap().heard;
    assert_eq!(heard, 1);
    mic.push(ScriptedWake::frame());
    mic.paced(&quiet(10));
    thread::sleep(Duration::from_millis(200));
    assert_eq!(
        wake.stats.lock().unwrap().heard,
        1,
        "the wake word is not heard while the assistant speaks"
    );
    assert!(asked.try_recv().is_err(), "and nothing new was asked");
    spk.hold.store(false, Ordering::SeqCst);
    until("the first reply to finish", || {
        spk.replies.load(Ordering::SeqCst) == 1
    });

    // The microphone stalls between turns: it is reopened by itself (about a second later).
    until("the hold-off to pass", || {
        thread::sleep(Duration::from_millis(600));
        true
    });
    let starts = mic.starts();
    mic.fail_with(AudioError::Stalled("no audio for 5 s".into()));
    until("the microphone to be reopened", || {
        mic.open() && mic.starts() > starts
    });

    // Turn two works as well.
    mic.push(ScriptedWake::frame());
    mic.paced(&speech());
    assert_eq!(
        asked.recv_timeout(Duration::from_secs(10)).unwrap(),
        "what is on my plate"
    );
    let (r, dev) = rx
        .recv_timeout(Duration::from_secs(20))
        .expect("the loop finished after two replies");
    r.unwrap();
    assert_eq!(spk.replies.load(Ordering::SeqCst), 2);
    assert_eq!(dev.wakes(), 2);
    assert_eq!(dev.problems.len(), 1, "{:?}", dev.problems);
    assert!(dev.problems[0].contains("stalled"), "{:?}", dev.problems);
    let _ = stop.send(());
}

// ---- a scripted core -------------------------------------------------------------------------------------------

fn is_wake(m: &Message) -> bool {
    matches!(m, Message::Wake(_))
}

fn is_audio(m: &Message) -> bool {
    matches!(m, Message::AudioChunk { .. })
}

/// An always-on device on a thread, connected to a scripted core.
fn device_on(
    core: &FakeCore,
    mic: &Mic,
    spk: &Speaker,
    wake: &ScriptedWake,
) -> thread::JoinHandle<()> {
    let addr = core.addr();
    let mut dev = AudioDevice::new(mic.clone(), spk.clone(), NoTrigger, live_config(None))
        .with_wake(wake.clone());
    thread::spawn(move || {
        let _ = run(TcpStream::connect(addr).unwrap(), &mut dev, &cfg());
    })
}

fn welcome(peer: &mut clawdot_e2e::Peer) {
    peer.send(&Message::Welcome(clawdot_protocol::message::Welcome {
        protocol: clawdot_protocol::message::PROTOCOL_VERSION,
        session_id: "s".into(),
        audio_out: clawdot_protocol::message::AudioCodec::Pcm,
    }));
}

#[test]
fn req_dev_027_a_muted_device_hears_nothing_and_releases_the_microphone() {
    let core = FakeCore::listen();
    let (mic, spk, wake) = (Mic::default(), Speaker::default(), ScriptedWake::default());
    let h = device_on(&core, &mic, &spk, &wake);
    let (mut peer, _hello) = core.accept();
    welcome(&mut peer);
    until("the microphone to be opened", || mic.open());

    peer.send(&Message::SetMute { muted: true });
    until("the microphone to be released", || !mic.open());
    mic.push(ScriptedWake::frame());
    peer.expect_none(Duration::from_millis(300), is_wake);
    assert!(!mic.open(), "still closed while muted");
    assert_eq!(wake.stats.lock().unwrap().fed, 0, "the engine was not fed");

    peer.send(&Message::SetMute { muted: false });
    until("the microphone to be reopened", || mic.open());
    mic.state().frames.clear();
    mic.push(ScriptedWake::frame());
    peer.expect("a wake after unmute", is_wake);
    drop(peer);
    h.join().unwrap();
}

#[test]
fn req_dev_025_the_words_spoken_right_after_the_wake_word_are_not_lost() {
    let core = FakeCore::listen();
    let (mic, spk, wake) = (Mic::default(), Speaker::default(), ScriptedWake::default());
    let h = device_on(&core, &mic, &spk, &wake);
    let (mut peer, _hello) = core.accept();
    welcome(&mut peer);
    until("the microphone to be opened", || mic.open());

    // The wake word and, at once, the start of a question: the core takes a moment to ask for audio.
    mic.push(ScriptedWake::frame());
    for i in 1..=10i16 {
        mic.push(vec![i; FRAME_SAMPLES]);
    }
    peer.expect("the wake event", is_wake);
    thread::sleep(Duration::from_millis(150));
    peer.send(&Message::StreamStart);
    let mut firsts = Vec::new();
    while firsts.len() < 3 {
        if let Message::AudioChunk { payload, .. } = peer.expect("audio", is_audio) {
            firsts.push(i16::from_le_bytes([payload[0], payload[1]]));
        }
    }
    assert_eq!(firsts, [1, 2, 3], "the kept audio comes first, in order");
    drop(peer);
    h.join().unwrap();
}

#[test]
fn req_dev_026_a_wake_word_while_the_reply_plays_or_just_after_is_ignored() {
    let core = FakeCore::listen();
    let (mic, spk, wake) = (Mic::default(), Speaker::default(), ScriptedWake::default());
    let h = device_on(&core, &mic, &spk, &wake);
    let (mut peer, _hello) = core.accept();
    welcome(&mut peer);
    until("the microphone to be opened", || mic.open());

    spk.hold.store(true, Ordering::SeqCst);
    peer.send(&Message::AudioStart(AudioFormat::PCM_16K_MONO));
    peer.send(&Message::AudioChunk {
        format: AudioFormat::PCM_16K_MONO,
        timestamp_ms: None,
        payload: vec![1, 0, 2, 0],
    });
    peer.send(&Message::AudioStop);
    until("the reply to reach the speaker", || {
        !spk.samples.lock().unwrap().is_empty()
    });
    mic.push(ScriptedWake::frame());
    peer.expect_none(Duration::from_millis(300), is_wake);
    spk.hold.store(false, Ordering::SeqCst);
    peer.expect("playback-done", |m| {
        matches!(m, Message::PlaybackDone { .. })
    });
    mic.push(ScriptedWake::frame());
    peer.expect_none(Duration::from_millis(200), is_wake);
    thread::sleep(Duration::from_millis(500));
    mic.push(ScriptedWake::frame());
    peer.expect("a wake word once the hold-off is over", is_wake);
    assert_eq!(mic.starts(), 1, "the microphone stayed open the whole time");
    drop(peer);
    h.join().unwrap();
}

#[test]
fn req_dev_028_a_missing_capture_program_is_not_retried_and_the_session_stays_up() {
    // A microphone that cannot start: the device says so once and keeps its connection to the core.
    #[derive(Debug, Clone)]
    struct Missing(Arc<AtomicUsize>);
    impl AudioIn for Missing {
        fn start(&mut self) -> Result<(), AudioError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(AudioError::NotFound("`ffmpeg` was not found".into()))
        }
        fn read_frame(&mut self, _: Duration) -> Result<Option<Vec<i16>>, AudioError> {
            Err(AudioError::NotStarted)
        }
        fn stop(&mut self) {}
    }
    let core = FakeCore::listen();
    let tries = Arc::new(AtomicUsize::new(0));
    let addr = core.addr();
    let mut dev = AudioDevice::new(
        Missing(Arc::clone(&tries)),
        Speaker::default(),
        NoTrigger,
        live_config(None),
    )
    .with_wake(ScriptedWake::default());
    let h = thread::spawn(move || {
        let r = run(TcpStream::connect(addr).unwrap(), &mut dev, &cfg());
        (r, dev.problems.clone())
    });
    let (mut peer, _hello) = core.accept();
    welcome(&mut peer);
    thread::sleep(Duration::from_millis(400));
    peer.send(&Message::Ping);
    peer.expect("a pong: the session is alive", |m| {
        matches!(m, Message::Pong)
    });
    drop(peer);
    let (r, problems) = h.join().unwrap();
    r.unwrap();
    assert_eq!(tries.load(Ordering::SeqCst), 1, "tried once");
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].contains("ffmpeg"));
}
