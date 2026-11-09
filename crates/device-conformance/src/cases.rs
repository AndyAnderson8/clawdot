//! The cases. Each is named for the requirement it covers and returns an [`Outcome`].

use crate::fake::run_session;
use crate::{BoardUnderTest, Fixture, Outcome, Profile};
use clawdot_device::board::EMBEDDED;
use clawdot_device::board_file::{BoardFile, DuckKind, MuteKind, UpdateMode};
use clawdot_device::button::{ButtonEvent, KeyButtons, QUEUE_LIMIT, ScriptedButtons};
use clawdot_device::caps::Capability;
use clawdot_device::daemon::{DaemonConfig, DaemonDevice, GainOut, run_forever};
use clawdot_device::led::{LedController, LedState, NullLeds};
use clawdot_device::quiet::{QuietHours, QuietPolicy};
use clawdot_device::tone::{self, Tone};
use clawdot_device::wake::{Detection, ScriptedEngine, WakeRouter, WordId, WordInfo};
use clawdot_protocol::message::{AudioFormat, Indicator, Message};
use clawdot_satellite::audio::AudioOut;
use clawdot_satellite::{Config, DEAD_AFTER, Device};
use serde_json::Value;
use std::io::Cursor;
use std::time::{Duration, Instant};

/// How binding a case is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pri {
    /// Must.
    Must,
    /// Should.
    Should,
    /// May.
    May,
}

/// One case of the suite.
#[derive(Clone, Copy)]
pub struct Case {
    /// Test name, starting with the requirement.
    pub id: &'static str,
    /// Requirement ID.
    pub req: &'static str,
    /// Priority of the requirement.
    pub pri: Pri,
    /// Run it.
    pub run: fn(&dyn BoardUnderTest) -> Outcome,
}

impl std::fmt::Debug for Case {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Case")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

macro_rules! case {
    ($id:ident, $req:literal, $pri:ident) => {
        Case {
            id: stringify!($id),
            req: $req,
            pri: Pri::$pri,
            run: $id,
        }
    };
}

/// Every case, in requirement order.
#[must_use]
pub fn cases() -> Vec<Case> {
    vec![
        case!(req_dev_001_one_daemon_for_every_board, "REQ-DEV-001", Must),
        case!(
            req_dev_002_hello_declares_exactly_the_present_capabilities,
            "REQ-DEV-002",
            Must
        ),
        case!(req_dev_003_audio_in_and_out, "REQ-DEV-003", Must),
        case!(req_dev_004_led_states, "REQ-DEV-004", Must),
        case!(req_dev_005_stop_without_the_wake_word, "REQ-DEV-005", Must),
        case!(req_dev_006_quiet_hours_apply_locally, "REQ-DEV-006", Must),
        case!(req_dev_007_updates_are_declared, "REQ-DEV-007", Must),
        case!(req_dev_008_board_file_validation, "REQ-DEV-008", Must),
        case!(req_dev_009_follow_up_listening, "REQ-DEV-009", Should),
        case!(req_dev_010_wifi_setup, "REQ-DEV-010", Should),
        case!(req_dev_012_minimum_hardware, "REQ-DEV-012", Must),
        case!(
            req_dev_013_absent_capabilities_have_equivalents,
            "REQ-DEV-013",
            Must
        ),
        case!(req_dev_014_tones_play_locally, "REQ-DEV-014", Must),
        case!(req_dev_015_mute_is_honest, "REQ-DEV-015", Must),
        case!(req_dev_016_ducking, "REQ-DEV-016", Must),
        case!(req_dev_017_push_to_talk_and_wake_seam, "REQ-DEV-017", Must),
        case!(req_dev_018_clock_trust, "REQ-DEV-018", Should),
        case!(req_dev_019_forbidden_paths, "REQ-DEV-019", Must),
        case!(
            req_dev_020_local_functions_survive_a_lost_core,
            "REQ-DEV-020",
            Must
        ),
        case!(req_dev_021_default_build_links_no_c, "REQ-DEV-021", Must),
        case!(req_dev_022_resource_profile, "REQ-DEV-022", Must),
        case!(req_dev_023_bluetooth_speaker_mode, "REQ-DEV-023", May),
        case!(req_dev_024_spotify_connect, "REQ-DEV-024", May),
    ]
}

// ---- helpers ----

const WAIT: Duration = Duration::from_secs(5);

fn file_of(b: &dyn BoardUnderTest) -> Result<BoardFile, String> {
    BoardFile::parse(&b.board_json()).map_err(|e| e.to_string())
}

fn daemon(
    b: &dyn BoardUnderTest,
    profile: Profile,
    fx: &Fixture,
    cfg: DaemonConfig,
) -> Result<DaemonDevice, String> {
    let parts = b.build(profile, fx)?;
    let mut d = DaemonDevice::new(parts, cfg);
    d.set_online(true);
    Ok(d)
}

fn cfg() -> Config {
    Config {
        device_id: "conformance".into(),
        token: None,
        wake_on_connect: false,
        dead_after: DEAD_AFTER,
    }
}

fn fail(msg: impl Into<String>) -> Outcome {
    Outcome::Fail(msg.into())
}

/// Run a check body that returns `Result<(), String>` as an outcome.
fn check(f: impl FnOnce() -> Result<(), String>) -> Outcome {
    match f() {
        Ok(()) => Outcome::Pass,
        Err(m) => Outcome::Fail(m),
    }
}

/// Scripted raw events for a gesture on a button, with times that make the gesture unambiguous.
fn gesture_events(file: &BoardFile, button: &str, gesture: &str) -> Vec<ButtonEvent> {
    let t0 = Instant::now();
    let at = |ms: u64| t0 + Duration::from_millis(ms);
    let edge = |down: bool, ms: u64| ButtonEvent::Edge {
        button: button.to_owned(),
        down,
        at: at(ms),
    };
    match gesture {
        "long_press" => vec![
            edge(true, 0),
            edge(false, u64::from(file.gestures.long_press_ms) + 100),
        ],
        "double_press" => vec![
            edge(true, 0),
            edge(false, 30),
            edge(true, 80),
            edge(false, 110),
        ],
        _ => vec![edge(true, 0), edge(false, 40)],
    }
}

/// The first `<button>.<gesture>` that maps to `action`, if the board file has one.
fn mapped(file: &BoardFile, action: &str) -> Option<(String, String)> {
    file.actions
        .iter()
        .find(|(_, a)| a == action)
        .and_then(|(k, _)| k.split_once('.'))
        .map(|(b, g)| (b.to_owned(), g.to_owned()))
}

fn has_button_driver(d: &DaemonDevice) -> bool {
    d.declared().has(Capability::Buttons)
}

fn replace_buttons(
    b: &dyn BoardUnderTest,
    fx: &Fixture,
    cfg: DaemonConfig,
    events: Vec<ButtonEvent>,
) -> Result<DaemonDevice, String> {
    let mut parts = b.build(Profile::Full, fx)?;
    parts.buttons = Box::new(ScriptedButtons(events.into()));
    let mut d = DaemonDevice::new(parts, cfg);
    d.set_online(true);
    Ok(d)
}

fn mutate(json: &str, edit: impl FnOnce(&mut Value)) -> String {
    let mut v: Value = serde_json::from_str(json).unwrap_or(Value::Null);
    edit(&mut v);
    v.to_string()
}

fn expect_key(json: &str, key: &str, what: &str) -> Result<(), String> {
    match BoardFile::parse(json) {
        Ok(_) => Err(format!("{what}: the board file was accepted")),
        Err(e) if e.key == key => Ok(()),
        Err(e) => Err(format!(
            "{what}: the error named `{}` instead of `{key}`: {e}",
            e.key
        )),
    }
}

// ---- cases ----

fn req_dev_001_one_daemon_for_every_board(b: &dyn BoardUnderTest) -> Outcome {
    check(|| {
        // The daemon's own modules, apart from the board loader, must not branch on a board name.
        let sources = [
            ("daemon.rs", include_str!("../../device/src/daemon.rs")),
            ("caps.rs", include_str!("../../device/src/caps.rs")),
            ("led.rs", include_str!("../../device/src/led.rs")),
            ("button.rs", include_str!("../../device/src/button.rs")),
            ("quiet.rs", include_str!("../../device/src/quiet.rs")),
            ("tone.rs", include_str!("../../device/src/tone.rs")),
            ("wake.rs", include_str!("../../device/src/wake.rs")),
        ];
        let mut names: Vec<String> = EMBEDDED.iter().map(|(n, _)| (*n).to_owned()).collect();
        names.push(b.name());
        for (file, text) in sources {
            for n in &names {
                if text.contains(&format!("\"{n}\"")) {
                    return Err(format!(
                        "{file} mentions the board name \"{n}\": per-device code in the daemon"
                    ));
                }
            }
        }
        // And the board runs through the one daemon type.
        daemon(b, Profile::Full, &Fixture::new(), DaemonConfig::default())?;
        Ok(())
    })
}

fn req_dev_002_hello_declares_exactly_the_present_capabilities(b: &dyn BoardUnderTest) -> Outcome {
    check(|| {
        let file = file_of(b)?;
        let fx = Fixture::new();
        let dev = daemon(b, Profile::Full, &fx, DaemonConfig::default())?;
        let want = dev.declared().to_protocol();
        let s = run_session(dev, move |core| {
            let c = &core.hello.capabilities;
            if *c != want {
                return Err(format!("hello declares {c:?}, the daemon has {want:?}"));
            }
            if c.leds && file.leds.is_none() {
                return Err("hello claims LEDs the board file does not have".into());
            }
            if c.buttons && file.buttons.is_empty() {
                return Err("hello claims buttons the board file does not have".into());
            }
            if c.hardware_mute && file.mute.kind != MuteKind::Hardware {
                return Err("hello claims a hardware mute the board file does not declare".into());
            }
            if c.wake_word && !file.features.on_device_wake {
                return Err(
                    "hello claims an on-device wake word the board file does not allow".into(),
                );
            }
            if c.aec && !file.features.aec {
                return Err("hello claims echo cancellation the board file does not allow".into());
            }
            if !c
                .audio_out
                .contains(&clawdot_protocol::message::AudioCodec::Pcm)
            {
                return Err("hello does not offer PCM downlink".into());
            }
            Ok(())
        });
        s.script
    })
}

fn req_dev_003_audio_in_and_out(b: &dyn BoardUnderTest) -> Outcome {
    if b.on_hardware() {
        return Outcome::Skip("needs a loopback or a person on the unit".into());
    }
    check(|| {
        let mut fx = Fixture::new();
        fx.mic = vec![1000; 320 * 12];
        let dev = daemon(b, Profile::Full, &fx, DaemonConfig::default())?;
        let rec = fx.rec.clone();
        let reply = vec![100i16; 320];
        let payload: Vec<u8> = reply.iter().flat_map(|s| s.to_le_bytes()).collect();
        let s = run_session(dev, move |core| {
            core.send(&Message::StreamStart)?;
            let msgs = core.collect(Duration::from_millis(700))?;
            let chunks: Vec<_> = msgs
                .iter()
                .filter_map(|m| match m {
                    Message::AudioChunk {
                        format, payload, ..
                    } => Some((*format, payload.clone())),
                    _ => None,
                })
                .collect();
            if chunks.len() < 3 {
                return Err(format!("only {} microphone frames arrived", chunks.len()));
            }
            for (f, p) in &chunks {
                if *f != AudioFormat::PCM_16K_MONO {
                    return Err(format!("uplink format {f:?} is not 16 kHz 16-bit mono"));
                }
                if p.len() != 640 {
                    return Err(format!("a frame has {} bytes, not 640 (20 ms)", p.len()));
                }
            }
            if chunks[0].1[..2] != 1000i16.to_le_bytes() {
                return Err("the first frame does not carry the microphone samples".into());
            }
            core.send(&Message::StreamStop)?;
            core.send(&Message::AudioStart(AudioFormat::PCM_16K_MONO))?;
            core.send(&Message::AudioChunk {
                format: AudioFormat::PCM_16K_MONO,
                timestamp_ms: None,
                payload,
            })?;
            core.send(&Message::AudioStop)?;
            core.expect(WAIT, |m| matches!(m, Message::PlaybackDone { .. }))?;
            Ok(())
        });
        s.script?;
        let played = rec.samples();
        if played != reply {
            return Err(format!(
                "the speaker got {} samples, expected the {} sent",
                played.len(),
                reply.len()
            ));
        }
        Ok(())
    })
}

fn req_dev_004_led_states(b: &dyn BoardUnderTest) -> Outcome {
    check(|| {
        for s in LedState::all() {
            if LedState::parse(s.name()) != Some(s) {
                return Err(format!("state {} does not round-trip", s.name()));
            }
        }
        if LedState::parse("sparkle").is_some() {
            return Err("an unknown state was accepted".into());
        }
        // A board file naming an unknown state is refused with the key (the loader), so the set is closed.
        let json = mutate(&b.board_json(), |v| {
            v["leds"] = serde_json::json!({"driver": "console", "states": {"sparkle": "x"}});
        });
        expect_key(&json, "leds.states.sparkle", "unknown LED state")?;
        // Precedence: one winner, fixed order.
        let mut c = LedController::new(Box::new(NullLeds), true);
        c.turn(LedState::Speaking);
        c.set(LedState::Muted, true);
        if c.current() != LedState::Muted {
            return Err("muted must outrank speaking".into());
        }
        c.set(LedState::Updating, true);
        if c.current() != LedState::Updating {
            return Err("updating must outrank everything".into());
        }
        // On the board: the core's indicators drive the shown state.
        let fx = Fixture::new();
        let mut d = daemon(b, Profile::Full, &fx, DaemonConfig::default())?;
        for (i, want) in [
            (Indicator::Listening, LedState::Listening),
            (Indicator::Thinking, LedState::Thinking),
            (Indicator::Speaking, LedState::Speaking),
            (Indicator::Error, LedState::Error),
            (Indicator::Notification, LedState::Inbox),
            (Indicator::Idle, LedState::Idle),
        ] {
            d.indicator(i);
            let got = d.status().led;
            if got != want {
                return Err(format!(
                    "indicator {i:?} should show {}, shows {}",
                    want.name(),
                    got.name()
                ));
            }
        }
        Ok(())
    })
}

fn req_dev_005_stop_without_the_wake_word(b: &dyn BoardUnderTest) -> Outcome {
    check(|| {
        let file = file_of(b)?;
        // 1. By the board's own stop gesture, with no core at all.
        if let Some((button, gesture)) = mapped(&file, "stop") {
            let fx = Fixture::new();
            let d = replace_buttons(
                b,
                &fx,
                DaemonConfig::default(),
                gesture_events(&file, &button, &gesture),
            )?;
            if has_button_driver(&d) {
                let mut d = d;
                d.begin_playback();
                d.pump_local();
                if d.stops() != 1 {
                    return Err(format!(
                        "the {button}.{gesture} gesture did not stop locally"
                    ));
                }
                if !b.on_hardware() && fx.rec.stop_count() == 0 {
                    return Err("the speaker was not silenced".into());
                }
            }
        }
        // 2. By the protocol, always: silenced at once, with the core saying nothing more.
        if !b.on_hardware() {
            let fx = Fixture::new();
            let dev = daemon(b, Profile::Full, &fx, DaemonConfig::default())?;
            let rec = fx.rec.clone();
            let s = run_session(dev, move |core| {
                core.send(&Message::AudioStart(AudioFormat::PCM_16K_MONO))?;
                core.send(&Message::StopPlayback)?;
                core.collect(Duration::from_millis(400))?;
                Ok(())
            });
            s.script?;
            if rec.stop_count() == 0 {
                return Err("clawdot.stop-playback did not silence the speaker".into());
            }
        }
        Ok(())
    })
}

fn req_dev_006_quiet_hours_apply_locally(b: &dyn BoardUnderTest) -> Outcome {
    check(|| {
        let window = || QuietPolicy::new(QuietHours::parse("22:00-07:00", 20, 5).ok());
        // Night, no core.
        let fx = Fixture::new();
        let mut d = daemon(
            b,
            Profile::Full,
            &fx,
            DaemonConfig {
                quiet: window(),
                ..DaemonConfig::default()
            },
        )?
        .with_clock(|| 23 * 60);
        d.set_online(false);
        d.pump_local();
        d.set_volume(90);
        if d.status().volume != 20 {
            return Err(format!(
                "volume 90 in quiet hours is {}, not capped at 20",
                d.status().volume
            ));
        }
        if !d.defer_announcement() {
            return Err("announcements are not deferred in quiet hours".into());
        }
        if !b.on_hardware() {
            d.begin_playback();
            d.play(&[10_000]);
            d.end_playback();
            if fx.rec.samples() != vec![2_000] {
                return Err(format!(
                    "reply audio was {:?}, not scaled to the ceiling",
                    fx.rec.samples()
                ));
            }
            if d.declared().has(Capability::Leds) && !fx.led_out.text().contains("brightness") {
                return Err("the LEDs were not dimmed".into());
            }
        }
        // Day.
        let fx = Fixture::new();
        let mut d = daemon(
            b,
            Profile::Full,
            &fx,
            DaemonConfig {
                quiet: window(),
                ..DaemonConfig::default()
            },
        )?
        .with_clock(|| 12 * 60);
        d.set_volume(90);
        if d.status().volume != 90 || d.defer_announcement() {
            return Err("quiet hours applied outside their window".into());
        }
        // No settings, no quiet hours.
        let fx = Fixture::new();
        let mut d = daemon(b, Profile::Full, &fx, DaemonConfig::default())?.with_clock(|| 23 * 60);
        d.set_volume(90);
        if d.status().volume != 90 {
            return Err("quiet hours applied with no settings".into());
        }
        Ok(())
    })
}

fn req_dev_007_updates_are_declared(b: &dyn BoardUnderTest) -> Outcome {
    match file_of(b) {
        Err(e) => fail(e),
        Ok(file) => match file.update {
            UpdateMode::Manual => Outcome::Pass,
            UpdateMode::Ab => Outcome::Skip(
                "declared `ab`; signature checks and rollback are shown on the unit with the image (B5)".into(),
            ),
        },
    }
}

fn req_dev_008_board_file_validation(b: &dyn BoardUnderTest) -> Outcome {
    check(|| {
        let json = b.board_json();
        BoardFile::parse(&json).map_err(|e| format!("the board's own file is invalid: {e}"))?;
        expect_key(
            &mutate(&json, |v| v["zzz"] = Value::from(1)),
            "zzz",
            "unknown top-level key",
        )?;
        expect_key(
            &mutate(&json, |v| v["audio"]["capture"]["bogus"] = Value::from(1)),
            "audio.capture.bogus",
            "unknown nested key",
        )?;
        expect_key(
            &mutate(&json, |v| v["audio"]["capture"]["rate"] = Value::from(8000)),
            "audio.capture.rate",
            "impossible rate",
        )?;
        expect_key(
            &mutate(&json, |v| v["schema"] = Value::from(2)),
            "schema",
            "future schema version",
        )?;
        expect_key(
            &mutate(&json, |v| {
                v["audio"]["capture"]["kind"] = Value::from("command");
                v["audio"]["capture"]["command"] = serde_json::json!(["arecord", "$(reboot)"]);
            }),
            "audio.capture.command[1]",
            "a command assembled from text",
        )?;
        expect_key(
            &mutate(&json, |v| {
                v["takeover"] = serde_json::json!({"forbidden": ["/sys/../etc"]})
            }),
            "takeover.forbidden[0]",
            "a path with ..",
        )?;
        Ok(())
    })
}

fn req_dev_009_follow_up_listening(_b: &dyn BoardUnderTest) -> Outcome {
    Outcome::Skip(
        "driven by the core (REQ-VOI-007); the daemon has no feature of its own to test".into(),
    )
}

fn req_dev_010_wifi_setup(_b: &dyn BoardUnderTest) -> Outcome {
    Outcome::Skip("Wi-Fi setup belongs to the image (roadmap B5); not built".into())
}

fn req_dev_012_minimum_hardware(b: &dyn BoardUnderTest) -> Outcome {
    check(|| {
        let file = file_of(b)?;
        if file.hardware.ram_mb < 256 || file.hardware.microphones < 1 || file.hardware.speakers < 1
        {
            return Err("the board file claims less than the minimum hardware".into());
        }
        let json = b.board_json();
        expect_key(
            &mutate(&json, |v| v["hardware"]["ram_mb"] = Value::from(128)),
            "hardware.ram_mb",
            "128 MB",
        )?;
        expect_key(
            &mutate(&json, |v| v["hardware"]["microphones"] = Value::from(0)),
            "hardware.microphones",
            "no microphone",
        )?;
        expect_key(
            &mutate(&json, |v| v["hardware"]["speakers"] = Value::from(0)),
            "hardware.speakers",
            "no speaker",
        )?;
        Ok(())
    })
}

fn req_dev_013_absent_capabilities_have_equivalents(b: &dyn BoardUnderTest) -> Outcome {
    check(|| {
        let fx = Fixture::new();
        let mut d = daemon(b, Profile::Stripped, &fx, DaemonConfig::default())?;
        for cap in [
            Capability::Leds,
            Capability::Buttons,
            Capability::HardwareMute,
        ] {
            if d.declared().has(cap) {
                return Err(format!(
                    "the stripped profile still declares {}",
                    cap.name()
                ));
            }
        }
        let c = d.capabilities();
        if c.leds || c.buttons || c.hardware_mute {
            return Err("hello claims hardware the stripped board lacks".into());
        }
        for cap in Capability::ALL {
            if cap.equivalent().is_empty() {
                return Err(format!("{} has no documented equivalent", cap.name()));
            }
            if !d.declared().has(cap)
                && !d.declared().report().contains(&format!("{} ", cap.name()))
            {
                return Err(format!("the report does not list absent {}", cap.name()));
            }
        }
        if !d.declared().report().contains("instead:") {
            return Err("the report gives no equivalent".into());
        }
        // Every Required capability is still delivered.
        for cap in Capability::ALL {
            if cap.class() == clawdot_device::caps::Class::Required
                && !matches!(cap, Capability::AudioIn | Capability::AudioOut)
                && !d.declared().has(cap)
            {
                return Err(format!(
                    "required capability {} is missing on the stripped board",
                    cap.name()
                ));
            }
        }
        // The error state is not silent without LEDs.
        d.indicator(Indicator::Error);
        if !b.on_hardware() && fx.rec.samples().is_empty() {
            return Err("the error state made no sound on a board with no LEDs".into());
        }
        Ok(())
    })
}

fn req_dev_014_tones_play_locally(b: &dyn BoardUnderTest) -> Outcome {
    if b.on_hardware() {
        return Outcome::Skip("needs a listener on the unit".into());
    }
    check(|| {
        // No core: the daemon is not connected to anything.
        for (ind, t) in [
            (Indicator::WakeDetected, Tone::Wake),
            (Indicator::Error, Tone::Error),
            (Indicator::Notification, Tone::Inbox),
        ] {
            let fx = Fixture::new();
            let mut d = daemon(b, Profile::Stripped, &fx, DaemonConfig::default())?;
            d.indicator(ind);
            if fx.rec.samples() != tone::samples(t) {
                return Err(format!("{ind:?} did not play the built-in {t:?} tone"));
            }
        }
        // A board with LEDs shows the state instead of playing a tone for it.
        let fx = Fixture::new();
        let mut d = daemon(b, Profile::Full, &fx, DaemonConfig::default())?;
        if d.declared().has(Capability::Leds) {
            d.indicator(Indicator::Error);
            if !fx.rec.samples().is_empty() {
                return Err("a tone played for a state the LEDs already show".into());
            }
        }
        Ok(())
    })
}

fn req_dev_015_mute_is_honest(b: &dyn BoardUnderTest) -> Outcome {
    check(|| {
        let file = file_of(b)?;
        let mut fx = Fixture::new();
        fx.mic = vec![1000; 320 * 12];
        let dev = daemon(b, Profile::Full, &fx, DaemonConfig::default())?;
        let honest = !dev.capabilities().hardware_mute || file.mute.kind == MuteKind::Hardware;
        if !honest {
            return Err("hardware mute claimed without a hardware declaration".into());
        }
        let can_observe = !b.on_hardware();
        let s = run_session(dev, move |core| {
            if file.mute.kind != MuteKind::Hardware && core.hello.capabilities.hardware_mute {
                return Err("hello claims a hardware mute".into());
            }
            core.send(&Message::SetMute { muted: true })?;
            core.send(&Message::StreamStart)?;
            let msgs = core.collect(Duration::from_millis(600))?;
            if can_observe && msgs.iter().any(|m| matches!(m, Message::AudioChunk { .. })) {
                return Err("microphone audio was streamed while muted".into());
            }
            Ok(())
        });
        s.script?;
        if !s.device.status().muted {
            return Err("the daemon did not apply the core's set-mute".into());
        }
        if s.device.declared().mute_kind == MuteKind::Software
            && s.device.capabilities().hardware_mute
        {
            return Err("software mute reported as hardware".into());
        }
        Ok(())
    })
}

fn req_dev_016_ducking(b: &dyn BoardUnderTest) -> Outcome {
    match file_of(b) {
        Err(e) => fail(e),
        Ok(f) => match f.audio.playback.duck {
            DuckKind::None => Outcome::Skip(
                "the board declares duck none; the daemon has no other audio source to pause yet"
                    .into(),
            ),
            DuckKind::Mixer => {
                Outcome::Skip("mixer ducking needs the ALSA backend, which is not built".into())
            }
            DuckKind::Software => check(|| {
                let rec = crate::fake::Rec::default();
                let mut out = GainOut::new(rec.clone());
                out.begin().map_err(|e| e.to_string())?;
                out.play(&[1000]).map_err(|e| e.to_string())?;
                out.duck(f.audio.playback.duck_level_pct);
                out.play(&[1000]).map_err(|e| e.to_string())?;
                out.duck(100);
                out.play(&[1000]).map_err(|e| e.to_string())?;
                let got = rec.samples();
                let ducked =
                    i16::try_from(i32::from(f.audio.playback.duck_level_pct) * 10).unwrap_or(0);
                if got != vec![1000, ducked, 1000] {
                    return Err(format!("ducking and restoring gave {got:?}"));
                }
                Ok(())
            }),
        },
    }
}

fn req_dev_017_push_to_talk_and_wake_seam(b: &dyn BoardUnderTest) -> Outcome {
    check(|| {
        // The seam: frame in, detection out; muted means not fed.
        let word = WordInfo {
            id: WordId(1),
            phrase: "test".into(),
            model_sha256: [0; 32],
            licence: "test".into(),
        };
        let mut r = WakeRouter::new(ScriptedEngine::new(word, vec![500]));
        r.set_muted(true);
        if r.feed(&[0; 1000]).is_some() || r.engine().fed != 0 {
            return Err("a muted engine was fed".into());
        }
        r.set_muted(false);
        let d: Option<Detection> = r.feed(&[0; 1000]);
        if d.is_none_or(|d| d.word == WordId::PTT) {
            return Err("the seam produced no detection from a frame".into());
        }
        // Push-to-talk on the board's own talk button, when it has one this build can read.
        let file = file_of(b)?;
        let Some((button, gesture)) = mapped(&file, "action") else {
            return Ok(());
        };
        let mut fx = Fixture::new();
        fx.mic = vec![0; 320 * 4];
        let d = replace_buttons(
            b,
            &fx,
            DaemonConfig::default(),
            gesture_events(&file, &button, &gesture),
        )?;
        if !has_button_driver(&d) {
            return Ok(());
        }
        let s = run_session(d, |core| {
            core.expect(WAIT, |m| matches!(m, Message::Wake(_)))
                .map(|_| ())
                .map_err(|e| format!("the talk button sent no wake event: {e}"))
        });
        s.script
    })
}

fn req_dev_018_clock_trust(_b: &dyn BoardUnderTest) -> Outcome {
    Outcome::Skip("clock trust reporting is not built yet".into())
}

fn req_dev_019_forbidden_paths(b: &dyn BoardUnderTest) -> Outcome {
    check(|| {
        let f = file_of(b)?;
        for p in &f.takeover.forbidden {
            if f.path_allowed(p) {
                return Err(format!("forbidden path {p} is allowed"));
            }
        }
        if f.path_allowed("/sys/../etc/shadow") {
            return Err("a path with .. is allowed".into());
        }
        if !f.path_allowed("/dev/null") {
            return Err("an ordinary path is refused".into());
        }
        Ok(())
    })
}

fn req_dev_020_local_functions_survive_a_lost_core(b: &dyn BoardUnderTest) -> Outcome {
    check(|| {
        let file = file_of(b)?;
        // Bounded queues keep the newest and never grow.
        let mut lines = "t\n".repeat(QUEUE_LIMIT * 4);
        lines.push_str("s\n");
        let mut k = KeyButtons::new(
            Cursor::new(lines.into_bytes()),
            vec![('t', "action".into()), ('s', "stop".into())],
        );
        // The reader thread works through the whole input at once; give it a moment, then drain.
        std::thread::sleep(Duration::from_millis(300));
        let mut seen = 0usize;
        let mut last = String::new();
        while let Some(e) = clawdot_device::button::ButtonSource::poll(&mut k) {
            if let ButtonEvent::Edge { button, .. } = e {
                seen += 1;
                last = button;
            }
        }
        if seen > QUEUE_LIMIT {
            return Err(format!("a flood of keys left {seen} events queued"));
        }
        if last != "stop" {
            return Err("the newest key press was dropped instead of the oldest".into());
        }
        // The core is unreachable: the LED says offline, and the board's mute button still works.
        let fx = Fixture::new();
        let events = match mapped(&file, "mute_toggle") {
            Some((button, gesture)) => gesture_events(&file, &button, &gesture),
            None => Vec::new(),
        };
        let mut d = replace_buttons(b, &fx, DaemonConfig::default(), events.clone())?;
        let r = run_forever(
            || Err("no core".to_owned()),
            &mut d,
            &cfg(),
            Some(2),
            |_| {},
        );
        if r.is_ok() {
            return Err("run_forever reported success with no core".into());
        }
        if d.status().led != LedState::Offline {
            return Err(format!(
                "with no core the LED shows {}, not offline",
                d.status().led.name()
            ));
        }
        if !events.is_empty() && has_button_driver(&d) && !d.status().muted {
            return Err("the mute button did not work with no core".into());
        }
        Ok(())
    })
}

fn req_dev_021_default_build_links_no_c(_b: &dyn BoardUnderTest) -> Outcome {
    check(|| {
        let manifest = include_str!("../../device/Cargo.toml");
        for bad in ["build =", "cc =", "-sys", "bindgen", "cmake", "pkg-config"] {
            if manifest.contains(bad) {
                return Err(format!(
                    "crates/device/Cargo.toml mentions `{bad}`: C code in the default build"
                ));
            }
        }
        Ok(())
    })
}

fn req_dev_022_resource_profile(_b: &dyn BoardUnderTest) -> Outcome {
    Outcome::Skip(
        "RSS and CPU are measured on the reference board at B3 (spec 0006 section 6.2)".into(),
    )
}

fn req_dev_023_bluetooth_speaker_mode(_b: &dyn BoardUnderTest) -> Outcome {
    Outcome::Skip("optional (May); not built".into())
}

fn req_dev_024_spotify_connect(_b: &dyn BoardUnderTest) -> Outcome {
    Outcome::Skip("optional (May); not built; off by default".into())
}
