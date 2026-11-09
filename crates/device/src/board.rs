//! Boards: how the daemon is given its hardware (REQ-DEV-001, REQ-BRD-001).
//!
//! A [`Board`] turns itself into [`Parts`]: an audio input and output, LEDs, buttons, and the capability
//! declaration. There is no per-device code: [`ConfiguredBoard`] builds everything from a [`BoardFile`], and
//! [`NullBoard`] has no hardware at all. A new device is a new board file, not a new type.

use crate::board_file::{
    AudioKind, BoardFile, ButtonSource as SourceSpec, LedDriverKind, PcmFormat,
};
use crate::button::{ActionMap, ButtonSource, KeyButtons, NoButtons};
use crate::caps::{Build, Declared};
use crate::led::{ConsoleLeds, LedSink, NullLeds};
use clawdot_satellite::audio::{AudioIn, AudioOut, WavIn, WavOut};
use clawdot_satellite::command::{
    CommandIn, CommandOut, DEFAULT_STALL, custom, default_capture, default_playback,
};
use std::io::{BufRead, Write};

/// Everything the daemon needs from a board.
pub struct Parts {
    /// Board id for status.
    pub name: String,
    /// Microphone.
    pub input: Box<dyn AudioIn>,
    /// Speaker.
    pub output: Box<dyn AudioOut>,
    /// LEDs (or [`NullLeds`]).
    pub leds: Box<dyn LedSink>,
    /// Buttons (or [`NoButtons`]).
    pub buttons: Box<dyn ButtonSource>,
    /// Gesture to action mapping.
    pub actions: ActionMap,
    /// Gesture thresholds in milliseconds: long press, double press.
    pub gesture_ms: (u32, u32),
    /// What is present.
    pub declared: Declared,
    /// LED brightness in normal and quiet hours, percent.
    pub led_brightness: (u8, u8),
    /// Parts that could not be set up and were declared absent instead (REQ-BRD-015).
    pub problems: Vec<String>,
}

impl std::fmt::Debug for Parts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Parts")
            .field("name", &self.name)
            .field("problems", &self.problems)
            .finish_non_exhaustive()
    }
}

/// A source of [`Parts`].
pub trait Board {
    /// Board id.
    fn name(&self) -> &str;
    /// What this board will declare, before anything is opened.
    fn declared(&self) -> Declared;
    /// Set up the hardware.
    ///
    /// # Errors
    /// A required part (the microphone or the speaker) cannot be set up; the text names the board-file key.
    fn assemble(self: Box<Self>) -> Result<Parts, String>;
}

/// A board with no hardware: no LEDs, no buttons, silence for a microphone and a speaker that discards. It
/// exists for tests and for a daemon whose hardware is not set up yet; it is not a supported board.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullBoard;

impl Board for NullBoard {
    fn name(&self) -> &str {
        "none"
    }

    fn declared(&self) -> Declared {
        Declared::software_only()
    }

    fn assemble(self: Box<Self>) -> Result<Parts, String> {
        Ok(Parts {
            name: "none".into(),
            input: Box::new(WavIn::new(Vec::new(), 0)),
            output: Box::new(WavOut::in_memory()),
            leds: Box::new(NullLeds),
            buttons: Box::new(NoButtons),
            actions: ActionMap::default(),
            gesture_ms: (700, 350),
            declared: Declared::software_only(),
            led_brightness: (100, 10),
            problems: Vec::new(),
        })
    }
}

/// The board described by a board file. The same type serves a PC, a Pi and a Dot.
pub struct ConfiguredBoard {
    file: BoardFile,
    build: Build,
    input: Option<Box<dyn AudioIn>>,
    output: Option<Box<dyn AudioOut>>,
    key_reader: Option<Box<dyn BufRead + Send>>,
    led_out: Option<Box<dyn Write + Send>>,
}

impl std::fmt::Debug for ConfiguredBoard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfiguredBoard")
            .field("board", &self.file.board)
            .finish_non_exhaustive()
    }
}

impl ConfiguredBoard {
    /// A board for `file`, as this build implements it.
    #[must_use]
    pub fn new(file: BoardFile) -> Self {
        Self::with_build(file, Build::CURRENT)
    }

    /// A board for `file` with an explicit [`Build`] (tests use this to turn features on and off).
    #[must_use]
    pub fn with_build(file: BoardFile, build: Build) -> Self {
        Self {
            file,
            build,
            input: None,
            output: None,
            key_reader: None,
            led_out: None,
        }
    }

    /// Use these audio parts instead of opening the device (WAV files in CI and tests).
    #[must_use]
    pub fn with_audio(mut self, input: Box<dyn AudioIn>, output: Box<dyn AudioOut>) -> Self {
        self.input = Some(input);
        self.output = Some(output);
        self
    }

    /// Read terminal keys from this reader instead of standard input.
    #[must_use]
    pub fn with_key_reader(mut self, reader: Box<dyn BufRead + Send>) -> Self {
        self.key_reader = Some(reader);
        self
    }

    /// Print console LED lines here instead of standard error.
    #[must_use]
    pub fn with_led_output(mut self, out: Box<dyn Write + Send>) -> Self {
        self.led_out = Some(out);
        self
    }

    /// The board file.
    #[must_use]
    pub fn file(&self) -> &BoardFile {
        &self.file
    }
}

fn native_ok(rate: u32, channels: u32, format: PcmFormat, key: &str) -> Result<(), String> {
    if rate == 16_000 && channels == 1 && format == PcmFormat::S16Le {
        Ok(())
    } else {
        Err(format!(
            "board file: `{key}`: command and default backends deliver 16000 Hz mono S16_LE; \
             this section says {rate} Hz, {channels} channel(s), {format:?}"
        ))
    }
}

impl Board for ConfiguredBoard {
    fn name(&self) -> &str {
        &self.file.board
    }

    fn declared(&self) -> Declared {
        Declared::from_board(&self.file, self.build)
    }

    fn assemble(self: Box<Self>) -> Result<Parts, String> {
        let this = *self;
        let file = this.file;
        let declared = Declared::from_board(&file, this.build);
        let mut problems = Vec::new();

        let input: Box<dyn AudioIn> = match this.input {
            Some(i) => i,
            None => {
                let c = &file.audio.capture;
                let spec = match c.kind {
                    AudioKind::Default => {
                        native_ok(c.rate, c.channels, c.format, "audio.capture")?;
                        default_capture(c.device.as_deref()).map_err(|e| e.to_string())?
                    }
                    AudioKind::Command => {
                        native_ok(c.rate, c.channels, c.format, "audio.capture")?;
                        let (program, args) = c
                            .command
                            .split_first()
                            .ok_or("board file: `audio.capture.command`: is empty")?;
                        custom(program, args, false)
                    }
                    AudioKind::Alsa => {
                        return Err("board file: `audio.capture.kind`: `alsa` is reserved and not available in this build; use `default` or `command`".into());
                    }
                };
                Box::new(CommandIn::new(spec, DEFAULT_STALL))
            }
        };
        let output: Box<dyn AudioOut> = match this.output {
            Some(o) => o,
            None => {
                let p = &file.audio.playback;
                let spec = match p.kind {
                    AudioKind::Default => {
                        native_ok(p.rate, p.channels, p.format, "audio.playback")?;
                        default_playback(p.device.as_deref()).map_err(|e| e.to_string())?
                    }
                    AudioKind::Command => {
                        native_ok(p.rate, p.channels, p.format, "audio.playback")?;
                        let (program, args) = p
                            .command
                            .split_first()
                            .ok_or("board file: `audio.playback.command`: is empty")?;
                        custom(program, args, false)
                    }
                    AudioKind::Alsa => {
                        return Err("board file: `audio.playback.kind`: `alsa` is reserved and not available in this build; use `default` or `command`".into());
                    }
                };
                Box::new(CommandOut::new(spec, DEFAULT_STALL))
            }
        };

        let (leds, led_brightness): (Box<dyn LedSink>, (u8, u8)) = match &file.leds {
            Some(spec) if spec.driver == LedDriverKind::Console && this.build.console_leds => {
                let sink = match this.led_out {
                    Some(w) => ConsoleLeds::new(w, spec.states.clone()),
                    None => ConsoleLeds::stderr(spec.states.clone()),
                };
                (
                    Box::new(sink),
                    (spec.brightness_pct, spec.quiet_brightness_pct),
                )
            }
            Some(spec) => {
                problems.push(format!(
                    "leds.driver `{:?}` is not available in this build; LEDs are declared absent and tones stand in",
                    spec.driver
                ));
                (Box::new(NullLeds), (100, 10))
            }
            None => (Box::new(NullLeds), (100, 10)),
        };

        let all_keys = !file.buttons.is_empty()
            && file
                .buttons
                .iter()
                .all(|b| matches!(b.source, SourceSpec::Key(_)));
        let buttons: Box<dyn ButtonSource> = if file.buttons.is_empty() {
            Box::new(NoButtons)
        } else if all_keys && this.build.key_buttons {
            match this.key_reader {
                Some(r) => {
                    let keys = file
                        .buttons
                        .iter()
                        .filter_map(|b| match b.source {
                            SourceSpec::Key(c) => Some((c, b.name.clone())),
                            SourceSpec::Evdev { .. } => None,
                        })
                        .collect();
                    Box::new(KeyButtons::new(r, keys))
                }
                None => Box::new(KeyButtons::stdin(&file)),
            }
        } else {
            problems.push(
                "buttons read from input devices are not available in this build; buttons are declared absent"
                    .into(),
            );
            Box::new(NoButtons)
        };

        let actions = if declared.has(crate::caps::Capability::Buttons) {
            ActionMap::from_board(&file)
        } else {
            ActionMap::default()
        };
        Ok(Parts {
            name: file.board.clone(),
            input,
            output,
            leds,
            buttons,
            actions,
            gesture_ms: (file.gestures.long_press_ms, file.gestures.double_press_ms),
            declared,
            led_brightness,
            problems,
        })
    }
}

/// Board files shipped inside the binary, by name.
pub const EMBEDDED: [(&str, &str); 3] = [
    ("desktop", include_str!("../../../boards/desktop.json")),
    ("echo-dot2", include_str!("../../../boards/echo-dot2.json")),
    (
        "raspberry-pi",
        include_str!("../../../boards/raspberry-pi.json"),
    ),
];

/// Load a board by name (`none`, or an embedded board) or from a file path.
///
/// # Errors
/// The name is unknown, the file cannot be read, or the board file is invalid (the text names the key).
pub fn load(name_or_path: &str) -> Result<Box<dyn Board>, String> {
    load_tuned(name_or_path, |_| {}, Build::CURRENT)
}

/// Like [`load`], but lets the caller adjust the board file before it is used (a microphone name given on the
/// command line) and say what this build implements (a wake engine was loaded).
///
/// # Errors
/// As [`load`].
pub fn load_tuned(
    name_or_path: &str,
    tune: impl FnOnce(&mut BoardFile),
    build: Build,
) -> Result<Box<dyn Board>, String> {
    if name_or_path == "none" {
        return Ok(Box::new(NullBoard));
    }
    if let Some((_, json)) = EMBEDDED.iter().find(|(n, _)| *n == name_or_path) {
        let mut file =
            BoardFile::parse(json).map_err(|e| format!("embedded board `{name_or_path}`: {e}"))?;
        tune(&mut file);
        return Ok(Box::new(ConfiguredBoard::with_build(file, build)));
    }
    if name_or_path.contains('/') || name_or_path.contains('\\') || name_or_path.ends_with(".json")
    {
        let json = std::fs::read_to_string(name_or_path)
            .map_err(|e| format!("cannot read board file {name_or_path}: {e}"))?;
        let mut file = BoardFile::parse(&json).map_err(|e| format!("{name_or_path}: {e}"))?;
        tune(&mut file);
        return Ok(Box::new(ConfiguredBoard::with_build(file, build)));
    }
    Err(format!(
        "unknown board `{name_or_path}`; known boards are none, {}; or give the path of a board file ending in .json",
        EMBEDDED
            .iter()
            .map(|(n, _)| *n)
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caps::Capability;

    #[test]
    fn req_dev_001_every_embedded_board_loads_through_the_same_type() {
        for (name, _) in EMBEDDED {
            let b = load(name).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(b.name(), name);
        }
    }

    #[test]
    fn req_dev_008_an_unknown_board_name_lists_the_known_ones() {
        let e = load("toaster").err().unwrap();
        assert!(e.contains("toaster") && e.contains("desktop") && e.contains("echo-dot2"));
        let e = load("/no/such/board.json").err().unwrap();
        assert!(e.contains("cannot read"));
    }

    #[test]
    fn req_brd_002_a_null_board_declares_no_hardware_and_still_assembles() {
        let b = NullBoard;
        assert!(!b.declared().has(Capability::Leds));
        assert!(!b.declared().has(Capability::Buttons));
        assert!(!b.declared().has(Capability::AudioIn));
        let parts = Box::new(b).assemble().unwrap();
        assert!(parts.problems.is_empty());
    }

    #[test]
    fn req_brd_015_unsupported_parts_are_reported_not_fatal() {
        let file = BoardFile::parse(include_str!("../../../boards/echo-dot2.json")).unwrap();
        // The Dot's audio is ALSA, which this build does not open: that part is required, so it is an error that
        // names the key. With audio supplied, LEDs and buttons degrade to absent with a problem for each.
        let board = ConfiguredBoard::new(file.clone());
        let e = Box::new(board).assemble().err().unwrap();
        assert!(e.contains("audio.capture.kind"), "{e}");
        let board = ConfiguredBoard::new(file).with_audio(
            Box::new(WavIn::new(vec![], 0)),
            Box::new(WavOut::in_memory()),
        );
        let parts = Box::new(board).assemble().unwrap();
        assert_eq!(parts.problems.len(), 2, "{:?}", parts.problems);
        assert!(!parts.declared.has(Capability::Leds));
        assert!(!parts.declared.has(Capability::Buttons));
        assert!(parts.actions == ActionMap::default());
    }

    #[test]
    fn command_backends_must_deliver_the_daemon_format() {
        let mut v: serde_json::Value =
            serde_json::from_str(include_str!("../../../boards/desktop.json")).unwrap();
        v["audio"]["capture"]["rate"] = serde_json::Value::from(48_000);
        let file = BoardFile::from_value(&v).unwrap();
        let e = Box::new(ConfiguredBoard::new(file))
            .assemble()
            .err()
            .unwrap();
        assert!(e.contains("audio.capture") && e.contains("16000"), "{e}");
    }
}
