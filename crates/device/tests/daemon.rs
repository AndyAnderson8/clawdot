//! The daemon against a scripted core: capability declaration, local stop, mute, offline behaviour, tones,
//! ducking and quiet hours. Requirement IDs start each test name.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use clawdot_device::board::{Board, ConfiguredBoard, NullBoard, Parts};
use clawdot_device::button::{ButtonEvent, ScriptedButtons};
use clawdot_device::daemon::{DaemonConfig, DaemonDevice, GainOut, run_forever};
use clawdot_device::led::{LedState, NullLeds};
use clawdot_device::quiet::{QuietHours, QuietPolicy};
use clawdot_device::{BoardFile, tone};
use clawdot_protocol::message::{AudioCodec, Indicator, Message, PROTOCOL_VERSION, Welcome};
use clawdot_protocol::{Decoder, Limits, encode};
use clawdot_satellite::audio::{AudioError, AudioOut, WavIn};
use clawdot_satellite::{Config, Device};
use std::io::{Cursor, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const DESKTOP: &str = include_str!("../../../boards/desktop.json");

/// An output that records everything played.
#[derive(Clone, Default)]
struct Rec {
    played: Arc<Mutex<Vec<i16>>>,
    stops: Arc<Mutex<usize>>,
}

impl AudioOut for Rec {
    fn begin(&mut self) -> Result<(), AudioError> {
        Ok(())
    }
    fn play(&mut self, pcm: &[i16]) -> Result<(), AudioError> {
        self.played.lock().unwrap().extend_from_slice(pcm);
        Ok(())
    }
    fn finish(&mut self) {}
    fn poll_done(&mut self) -> Result<bool, AudioError> {
        Ok(true)
    }
    fn stop(&mut self) {
        *self.stops.lock().unwrap() += 1;
    }
}

fn desktop_board() -> BoardFile {
    BoardFile::parse(DESKTOP).unwrap()
}

/// A desktop board whose audio is a recorder and whose keys come from `keys`.
fn desktop_parts(rec: &Rec, keys: &str) -> Parts {
    let board = ConfiguredBoard::new(desktop_board())
        .with_audio(Box::new(WavIn::new(vec![], 0)), Box::new(rec.clone()))
        .with_key_reader(Box::new(Cursor::new(keys.as_bytes().to_vec())))
        .with_led_output(Box::new(std::io::sink()));
    Box::new(board).assemble().unwrap()
}

/// Parts with the desktop's actions and scripted button events instead of terminal keys.
fn scripted_parts(rec: &Rec, events: Vec<ButtonEvent>, leds: bool) -> Parts {
    let mut p = desktop_parts(rec, "");
    p.buttons = Box::new(ScriptedButtons(events.into()));
    if !leds {
        p.leds = Box::new(NullLeds);
        p.declared = {
            let mut f = desktop_board();
            f.leds = None;
            clawdot_device::caps::Declared::from_board(&f, clawdot_device::caps::Build::CURRENT)
        };
    }
    p
}

fn press(button: &str) -> Vec<ButtonEvent> {
    let at = Instant::now();
    vec![
        ButtonEvent::Edge {
            button: button.into(),
            down: true,
            at,
        },
        ButtonEvent::Edge {
            button: button.into(),
            down: false,
            at,
        },
    ]
}

fn send(s: &mut TcpStream, m: &Message) {
    s.write_all(&encode(&m.to_frame()).unwrap()).unwrap();
}

/// A scripted core: reads hello, answers welcome, then runs `script`.
fn fake_core(
    script: impl FnOnce(&mut TcpStream, &mut dyn FnMut() -> Message, clawdot_protocol::message::Hello)
    + Send
    + 'static,
) -> (u16, thread::JoinHandle<()>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let h = thread::spawn(move || {
        let (mut s, _) = l.accept().unwrap();
        s.set_read_timeout(Some(Duration::from_millis(20))).unwrap();
        let mut reader = s.try_clone().unwrap();
        let mut dec = Decoder::new(Limits::default());
        let mut next = move || -> Message {
            let end = Instant::now() + Duration::from_secs(10);
            loop {
                if let Some(f) = dec.next_frame().unwrap() {
                    return Message::from_frame(f).unwrap();
                }
                assert!(Instant::now() < end, "timed out waiting for the daemon");
                let mut buf = [0u8; 8192];
                match reader.read(&mut buf) {
                    Ok(0) => panic!("the daemon hung up"),
                    Ok(n) => dec.push(&buf[..n]),
                    Err(_) => {}
                }
            }
        };
        let hello = loop {
            if let Message::Hello(h) = next() {
                break h;
            }
        };
        send(
            &mut s,
            &Message::Welcome(Welcome {
                protocol: PROTOCOL_VERSION,
                session_id: "s".into(),
                audio_out: AudioCodec::Pcm,
            }),
        );
        script(&mut s, &mut next, hello);
    });
    (port, h)
}

fn cfg() -> Config {
    Config {
        device_id: "t".into(),
        token: None,
        wake_on_connect: false,
        // Pings every quarter of this, so a test that waits for one does not wait long.
        dead_after: Duration::from_secs(2),
    }
}

#[test]
fn req_dev_002_hello_declares_what_the_board_has_and_nothing_else() {
    let (port, core) = fake_core(|s, _next, hello| {
        let c = hello.capabilities;
        assert!(c.leds, "console LEDs are present on the desktop board");
        assert!(c.buttons);
        assert!(!c.wake_word, "no wake engine is built, so none is declared");
        assert!(!c.hardware_mute && !c.aec && !c.display);
        send(s, &Message::StreamStop);
    });
    let rec = Rec::default();
    let mut dev = DaemonDevice::new(
        desktop_parts(&rec, ""),
        DaemonConfig {
            max_turns: Some(0),
            ..DaemonConfig::default()
        },
    );
    let sock = TcpStream::connect(("127.0.0.1", port)).unwrap();
    clawdot_satellite::run(sock, &mut dev, &cfg()).unwrap();
    core.join().unwrap();
}

#[test]
fn req_dev_002_a_null_board_declares_no_hardware() {
    let parts = Box::new(NullBoard).assemble().unwrap();
    let dev = DaemonDevice::new(parts, DaemonConfig::default());
    let c = dev.capabilities();
    assert!(!c.leds && !c.buttons && !c.wake_word && !c.hardware_mute && !c.aec);
    assert_eq!(c.audio_out, vec![AudioCodec::Pcm]);
}

#[test]
fn req_dev_005_stop_acts_locally_with_no_core() {
    let rec = Rec::default();
    let mut dev = DaemonDevice::new(
        scripted_parts(&rec, press("stop"), true),
        DaemonConfig::default(),
    );
    assert!(!dev.status().online);
    dev.begin_playback();
    dev.pump_local();
    assert_eq!(dev.stops(), 1);
    assert!(*rec.stops.lock().unwrap() >= 1, "the speaker was silenced");
    assert_eq!(
        dev.status().led,
        LedState::Offline,
        "still offline: stop did not need the core"
    );
}

#[test]
fn req_dev_005_the_stop_button_is_a_gesture_from_the_board_file() {
    // Keys typed on the desktop: `s` is the stop button.
    let rec = Rec::default();
    let mut dev = DaemonDevice::new(desktop_parts(&rec, "s\n"), DaemonConfig::default());
    let end = Instant::now() + Duration::from_secs(5);
    while dev.stops() == 0 && Instant::now() < end {
        dev.pump_local();
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(dev.stops(), 1);
}

#[test]
fn req_dev_015_the_mute_button_toggles_and_tells_the_core() {
    let (port, core) = fake_core(|s, next, _| {
        loop {
            match next() {
                Message::MuteChanged { muted } => {
                    assert!(muted);
                    break;
                }
                Message::Ping => send(s, &Message::Pong),
                _ => {}
            }
        }
        send(s, &Message::SetMute { muted: false });
        loop {
            // The daemon obeys the core's mute change too and does not echo it back as its own.
            if matches!(next(), Message::Ping) {
                break;
            }
        }
    });
    let rec = Rec::default();
    let mut dev = DaemonDevice::new(
        scripted_parts(&rec, press("mute"), true),
        DaemonConfig::default(),
    );
    let sock = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let started = Instant::now();
    let r = clawdot_satellite::run(sock, &mut dev, &cfg());
    core.join().unwrap();
    let status = dev.status();
    assert!(r.is_ok(), "{r:?}");
    assert!(started.elapsed() < Duration::from_secs(30));
    assert!(!status.muted, "the core's set-mute is applied");
}

#[test]
fn req_dev_015_software_mute_is_never_reported_as_hardware() {
    let rec = Rec::default();
    let dev = DaemonDevice::new(desktop_parts(&rec, ""), DaemonConfig::default());
    assert_eq!(
        dev.declared().mute_kind,
        clawdot_device::board_file::MuteKind::Software
    );
    assert!(!dev.capabilities().hardware_mute);
}

#[test]
fn req_dev_020_local_functions_keep_working_while_the_core_is_away() {
    let rec = Rec::default();
    let mut events = press("mute");
    events.extend(press("stop"));
    let mut dev = DaemonDevice::new(scripted_parts(&rec, events, true), DaemonConfig::default());
    // Nothing listens on port 1: every connection fails.
    let r = run_forever(
        || Err("refused".to_owned()),
        &mut dev,
        &cfg(),
        Some(2),
        |_| {},
    );
    assert!(r.is_err());
    let s = dev.status();
    assert!(s.muted, "the mute button worked with no core");
    assert_eq!(dev.stops(), 1, "the stop button worked with no core");
    assert_eq!(s.led, LedState::Offline);
}

#[test]
fn req_dev_014_a_board_with_leds_shows_state_and_a_board_without_plays_a_tone() {
    let with = Rec::default();
    let mut dev = DaemonDevice::new(scripted_parts(&with, vec![], true), DaemonConfig::default());
    dev.set_online(true);
    dev.indicator(Indicator::Error);
    assert_eq!(dev.status().led, LedState::Error);
    assert!(
        with.played.lock().unwrap().is_empty(),
        "no tone where an LED shows the state"
    );

    let without = Rec::default();
    let mut dev = DaemonDevice::new(
        scripted_parts(&without, vec![], false),
        DaemonConfig::default(),
    );
    dev.set_online(true);
    dev.indicator(Indicator::Error);
    assert_eq!(
        *without.played.lock().unwrap(),
        tone::samples(tone::Tone::Error)
    );
    // A state with no tone equivalent stays silent.
    without.played.lock().unwrap().clear();
    dev.indicator(Indicator::Thinking);
    assert!(without.played.lock().unwrap().is_empty());
}

#[test]
fn req_dev_014_a_tone_does_not_interrupt_a_reply() {
    let rec = Rec::default();
    let mut dev = DaemonDevice::new(scripted_parts(&rec, vec![], false), DaemonConfig::default());
    dev.set_online(true);
    dev.begin_playback();
    dev.indicator(Indicator::Error);
    assert!(rec.played.lock().unwrap().is_empty());
}

#[test]
fn req_dev_016_ducking_lowers_and_restores_the_daemons_own_audio() {
    let rec = Rec::default();
    let mut out = GainOut::new(rec.clone());
    out.begin().unwrap();
    out.play(&[1000, -1000]).unwrap();
    out.duck(30);
    out.play(&[1000, -1000]).unwrap();
    out.duck(100);
    out.play(&[1000, -1000]).unwrap();
    assert_eq!(
        *rec.played.lock().unwrap(),
        vec![1000, -1000, 300, -300, 1000, -1000]
    );
    assert!(out.capabilities().duck && out.capabilities().volume);
}

#[test]
fn req_dev_006_quiet_hours_cap_the_volume_locally() {
    let rec = Rec::default();
    let quiet = QuietPolicy::new(Some(QuietHours::parse("22:00-07:00", 20, 5).unwrap()));
    let mut dev = DaemonDevice::new(
        desktop_parts(&rec, ""),
        DaemonConfig {
            quiet,
            ..DaemonConfig::default()
        },
    )
    .with_clock(|| 23 * 60);
    dev.set_volume(90);
    assert_eq!(dev.status().volume, 20);
    assert!(dev.defer_announcement());
    // The reply is played at the capped level.
    dev.begin_playback();
    dev.play(&[10_000]);
    dev.end_playback();
    assert_eq!(*rec.played.lock().unwrap(), vec![2_000]);
}

#[test]
fn req_dev_006_outside_quiet_hours_nothing_changes() {
    let rec = Rec::default();
    let quiet = QuietPolicy::new(Some(QuietHours::parse("22:00-07:00", 20, 5).unwrap()));
    let mut dev = DaemonDevice::new(
        desktop_parts(&rec, ""),
        DaemonConfig {
            quiet,
            ..DaemonConfig::default()
        },
    )
    .with_clock(|| 12 * 60);
    dev.set_volume(90);
    assert_eq!(dev.status().volume, 90);
    assert!(!dev.defer_announcement());
}

#[test]
fn req_dev_006_the_led_dims_in_quiet_hours() {
    let led_text = Arc::new(Mutex::new(Vec::<u8>::new()));
    struct W(Arc<Mutex<Vec<u8>>>);
    impl Write for W {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let board = ConfiguredBoard::new(desktop_board())
        .with_audio(Box::new(WavIn::new(vec![], 0)), Box::new(Rec::default()))
        .with_key_reader(Box::new(Cursor::new(Vec::new())))
        .with_led_output(Box::new(W(led_text.clone())));
    let parts = Box::new(board).assemble().unwrap();
    let quiet = QuietPolicy::new(Some(QuietHours::parse("22:00-07:00", 20, 5).unwrap()));
    let mut dev = DaemonDevice::new(
        parts,
        DaemonConfig {
            quiet,
            ..DaemonConfig::default()
        },
    )
    .with_clock(|| 23 * 60);
    dev.pump_local();
    let text = String::from_utf8(led_text.lock().unwrap().clone()).unwrap();
    assert!(text.contains("led: brightness 5%"), "{text}");
}

#[test]
fn req_dev_004_the_leds_follow_the_core_and_the_precedence_holds() {
    let rec = Rec::default();
    let mut dev = DaemonDevice::new(desktop_parts(&rec, ""), DaemonConfig::default());
    dev.set_online(true);
    for (i, led) in [
        (Indicator::Listening, LedState::Listening),
        (Indicator::Thinking, LedState::Thinking),
        (Indicator::Speaking, LedState::Speaking),
        (Indicator::Notification, LedState::Inbox),
        (Indicator::Idle, LedState::Idle),
    ] {
        dev.indicator(i);
        assert_eq!(dev.status().led, led);
    }
    dev.set_mute(true);
    dev.indicator(Indicator::Speaking);
    assert_eq!(dev.status().led, LedState::Muted, "muted outranks speaking");
    dev.set_mute(false);
    assert_eq!(dev.status().led, LedState::Speaking);
}

#[test]
fn req_dev_029_the_real_engine_hears_its_word_from_the_open_microphone_and_wakes_the_core() {
    use clawdot_device::wake::WordId;
    use clawdot_device::wake_mww::{MwwEngine, MwwSink, load_word};
    let golden = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../wake/tests/golden");
    let word = load_word(
        WordId(1),
        &golden.join("models/okay_nabu.tflite"),
        "Apache-2.0",
    )
    .unwrap();
    let pcm: Vec<i16> = std::fs::read(golden.join("phrase_a.s16"))
        .unwrap()
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect();
    let (port, core) = fake_core(|_s, next, _hello| {
        loop {
            if matches!(next(), Message::Wake(_)) {
                return;
            }
        }
    });
    let rec = Rec::default();
    let mut parts = desktop_parts(&rec, "");
    parts.input = Box::new(WavIn::new(pcm, 500));
    let mut dev = DaemonDevice::new(
        parts,
        DaemonConfig {
            wake: Some(Box::new(MwwSink::new(MwwEngine::new(vec![word]).unwrap()))),
            ..DaemonConfig::default()
        },
    );
    let sock = TcpStream::connect(("127.0.0.1", port)).unwrap();
    // The scripted core returns on the first wake; the daemon then sees the connection close and returns.
    let r = clawdot_satellite::run(sock, &mut dev, &cfg());
    core.join().unwrap();
    assert!(r.is_ok() || dev.inner().wakes() >= 1, "{r:?}");
    assert_eq!(dev.inner().wakes(), 1, "one utterance, one wake");
}
