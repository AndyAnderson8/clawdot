//! The board file: strict loading and validation (REQ-DEV-008).
//!
//! The file is JSON described by `schemas/board/board-1.schema.json`. This loader checks the same rules by hand
//! (to add no schema library) plus the few rules the schema cannot say. Every object is closed: an unknown key
//! stops loading and the error names it as a dotted path (`audio.capture.foo`). The first problem found is the
//! one reported.

use serde_json::{Map, Value};
use std::fmt;

/// The schema version this loader understands.
pub const SCHEMA_VERSION: u64 = 1;

/// Defaults the schema documents for a board file that leaves them out.
/// Press time that makes a long press, in milliseconds.
pub const DEFAULT_LONG_PRESS_MS: u32 = 700;
/// Two presses within this many milliseconds make a double press.
pub const DEFAULT_DOUBLE_PRESS_MS: u32 = 350;
/// LED brightness in normal hours, percent.
pub const DEFAULT_LED_BRIGHTNESS_PCT: u8 = 100;
/// LED brightness in quiet hours, percent.
pub const DEFAULT_LED_QUIET_BRIGHTNESS_PCT: u8 = 10;

/// Why a board file was refused. `key` is the dotted path of the offending key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardError {
    /// Dotted path, for example `audio.capture.rate`; empty for the whole file.
    pub key: String,
    /// What is wrong.
    pub message: String,
}

impl fmt::Display for BoardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.key.is_empty() {
            write!(f, "board file: {}", self.message)
        } else {
            write!(f, "board file: `{}`: {}", self.key, self.message)
        }
    }
}

impl std::error::Error for BoardError {}

type Res<T> = Result<T, BoardError>;

fn err<T>(key: &str, message: impl Into<String>) -> Res<T> {
    Err(BoardError {
        key: key.to_owned(),
        message: message.into(),
    })
}

/// Logical button names a board file may use.
pub const BUTTONS: [&str; 5] = ["action", "volume_up", "volume_down", "mute", "stop"];
/// Gesture names.
pub const GESTURES: [&str; 3] = ["press", "long_press", "double_press"];
/// Logical actions.
pub const ACTIONS: [&str; 5] = ["volume_up", "volume_down", "mute_toggle", "action", "stop"];
/// Logical LED state names (set 1).
pub const LED_STATES: [&str; 14] = [
    "idle",
    "wake",
    "listening",
    "thinking",
    "speaking",
    "muted",
    "error",
    "offline",
    "timer",
    "alarm",
    "inbox",
    "volume",
    "updating",
    "setup",
];

/// Processor architecture of the board.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    /// 32-bit ARM.
    Armv7,
    /// 64-bit ARM.
    Aarch64,
    /// x86-64.
    X86_64,
    /// 64-bit RISC-V.
    Riscv64,
}

/// The minimum-contract facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hardware {
    /// Architecture.
    pub arch: Arch,
    /// Total RAM in megabytes.
    pub ram_mb: u64,
    /// Number of microphones.
    pub microphones: u64,
    /// Number of speakers.
    pub speakers: u64,
}

/// A file that must contain a value for the board to be detected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectFile {
    /// Absolute path.
    pub path: String,
    /// Required content (the start of the file).
    pub equals: String,
}

/// How audio is reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioKind {
    /// Find a known program on `PATH`.
    Default,
    /// Run the given argument vector.
    Command,
    /// Open the ALSA device directly (reserved).
    Alsa,
}

/// PCM sample format on the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PcmFormat {
    /// Signed 16-bit little-endian.
    S16Le,
    /// Signed 24-bit little-endian in 3 bytes.
    S24_3Le,
    /// Signed 32-bit little-endian.
    S32Le,
}

/// Microphone side of the audio description.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capture {
    /// How to reach it.
    pub kind: AudioKind,
    /// Device name for the discovered program or ALSA.
    pub device: Option<String>,
    /// Program and arguments for `kind = command`.
    pub command: Vec<String>,
    /// Native sample rate.
    pub rate: u32,
    /// Native format.
    pub format: PcmFormat,
    /// Channels captured.
    pub channels: u32,
    /// Channels that are microphones.
    pub mic_channels: Vec<u32>,
    /// Channels that carry the playback reference.
    pub reference_channels: Vec<u32>,
}

/// How software lowers other audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DuckKind {
    /// Cannot duck.
    None,
    /// Scale the daemon's own audio.
    Software,
    /// Lower the whole output through the mixer.
    Mixer,
}

/// Speaker side of the audio description.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Playback {
    /// How to reach it.
    pub kind: AudioKind,
    /// Device name.
    pub device: Option<String>,
    /// Program and arguments for `kind = command`.
    pub command: Vec<String>,
    /// Native sample rate.
    pub rate: u32,
    /// Native format.
    pub format: PcmFormat,
    /// Channels played.
    pub channels: u32,
    /// Mixer control that sets the volume.
    pub volume_control: Option<String>,
    /// Ducking kind.
    pub duck: DuckKind,
    /// Ducked level, percent of normal.
    pub duck_level_pct: u8,
}

/// A mixer control set at start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MixerRoute {
    /// Control name.
    pub control: String,
    /// Value to set.
    pub value: String,
}

/// Audio section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioSpec {
    /// Microphone.
    pub capture: Capture,
    /// Speaker.
    pub playback: Playback,
    /// Mixer routes applied in order.
    pub mixer_routes: Vec<MixerRoute>,
}

/// Where a button's raw events come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ButtonSource {
    /// A Linux input device and key code.
    Evdev {
        /// Device node.
        node: String,
        /// Key code.
        code: u32,
    },
    /// A key typed on the terminal.
    Key(char),
}

/// One physical button.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ButtonSpec {
    /// Logical name, one of [`BUTTONS`].
    pub name: String,
    /// Raw source.
    pub source: ButtonSource,
}

/// Gesture timing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gestures {
    /// A press held this long is a long press.
    pub long_press_ms: u32,
    /// Two presses within this window are a double press.
    pub double_press_ms: u32,
}

impl Default for Gestures {
    fn default() -> Self {
        Self {
            long_press_ms: DEFAULT_LONG_PRESS_MS,
            double_press_ms: DEFAULT_DOUBLE_PRESS_MS,
        }
    }
}

/// How LED states are shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedDriverKind {
    /// Print state changes.
    Console,
    /// Vendor `ledctrl` command.
    Ledctrl,
    /// Direct frame writes.
    Frame,
    /// Sysfs files.
    Sysfs,
}

/// LED section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedSpec {
    /// Driver.
    pub driver: LedDriverKind,
    /// Number of LEDs.
    pub count: Option<u32>,
    /// Normal brightness.
    pub brightness_pct: u8,
    /// Brightness in quiet hours.
    pub quiet_brightness_pct: u8,
    /// Logical state to pattern name.
    pub states: Vec<(String, String)>,
    /// Frame writer description, when `driver = frame`.
    pub frame: Option<(String, String)>,
}

/// What kind of microphone mute the board has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MuteKind {
    /// No mute control on the device.
    None,
    /// Capture is discarded.
    Software,
    /// The microphone path is proved cut.
    Hardware,
}

/// Mute section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MuteSpec {
    /// Kind.
    pub kind: MuteKind,
    /// File that shows the state.
    pub state_file: Option<String>,
}

/// Features beyond the required set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Features {
    /// Echo cancellation fits.
    pub aec: bool,
    /// A wake engine fits.
    pub on_device_wake: bool,
    /// Bluetooth sink.
    pub bluetooth_sink: bool,
    /// Bluetooth source.
    pub bluetooth_source: bool,
    /// Spotify Connect receiver allowed.
    pub spotify_connect: bool,
}

/// How the board is updated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateMode {
    /// Signed A/B with rollback.
    Ab,
    /// By hand.
    Manual,
}

/// Wi-Fi section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WifiSpec {
    /// Interface name.
    pub interface: String,
    /// Supplicant control directory.
    pub control_dir: Option<String>,
}

/// Clock section.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClockSpec {
    /// Date before which the clock counts as unset.
    pub floor: Option<String>,
    /// Verify with an HTTPS `Date` check.
    pub https_date: bool,
}

/// Takeover section.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Takeover {
    /// Services to stop.
    pub stop: Vec<String>,
    /// Paths never to touch.
    pub forbidden: Vec<String>,
}

/// A validated board file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardFile {
    /// Board id.
    pub board: String,
    /// Free text.
    pub description: String,
    /// Minimum-contract facts.
    pub hardware: Hardware,
    /// Detection fingerprints.
    pub detect: Vec<DetectFile>,
    /// Audio.
    pub audio: AudioSpec,
    /// Buttons.
    pub buttons: Vec<ButtonSpec>,
    /// Gesture timing.
    pub gestures: Gestures,
    /// `<button>.<gesture>` to logical action.
    pub actions: Vec<(String, String)>,
    /// LEDs, if any.
    pub leds: Option<LedSpec>,
    /// Mute.
    pub mute: MuteSpec,
    /// Wi-Fi, if managed.
    pub wifi: Option<WifiSpec>,
    /// Clock.
    pub clock: Option<ClockSpec>,
    /// Features.
    pub features: Features,
    /// Update mode.
    pub update: UpdateMode,
    /// Takeover.
    pub takeover: Takeover,
}

// ---- parsing helpers ----

/// A JSON object being read: remembers which keys were used so leftovers can be reported.
struct Obj<'a> {
    path: String,
    map: &'a Map<String, Value>,
    used: Vec<&'static str>,
}

impl<'a> Obj<'a> {
    fn new(v: &'a Value, path: &str) -> Res<Self> {
        match v.as_object() {
            Some(map) => Ok(Self {
                path: path.to_owned(),
                map,
                used: Vec::new(),
            }),
            None => err(path, "must be an object"),
        }
    }

    fn at(&self, key: &str) -> String {
        if self.path.is_empty() {
            key.to_owned()
        } else {
            format!("{}.{key}", self.path)
        }
    }

    fn opt(&mut self, key: &'static str) -> Option<&'a Value> {
        self.used.push(key);
        self.map.get(key)
    }

    fn req(&mut self, key: &'static str) -> Res<&'a Value> {
        self.opt(key)
            .map_or_else(|| err(&self.at(key), "is required"), Ok)
    }

    /// Fail on any key that was never asked for.
    fn finish(self) -> Res<()> {
        for k in self.map.keys() {
            if !self.used.contains(&k.as_str()) {
                return err(&self.at(k), "unknown key");
            }
        }
        Ok(())
    }
}

fn int(v: &Value, key: &str, min: u64, max: u64) -> Res<u64> {
    let Some(n) = v.as_u64() else {
        return err(key, "must be a whole number, zero or more");
    };
    if n < min || n > max {
        return err(key, format!("must be between {min} and {max} (got {n})"));
    }
    Ok(n)
}

fn boolean(v: &Value, key: &str) -> Res<bool> {
    v.as_bool()
        .map_or_else(|| err(key, "must be true or false"), Ok)
}

fn text<'a>(v: &'a Value, key: &str, max: usize) -> Res<&'a str> {
    let Some(s) = v.as_str() else {
        return err(key, "must be text");
    };
    if s.chars().count() > max {
        return err(key, format!("must be at most {max} characters"));
    }
    Ok(s)
}

fn choice<'a>(v: &'a Value, key: &str, allowed: &[&str]) -> Res<&'a str> {
    let s = text(v, key, 64)?;
    if allowed.contains(&s) {
        Ok(s)
    } else {
        err(
            key,
            format!("must be one of {} (got `{s}`)", allowed.join(", ")),
        )
    }
}

fn array<'a>(v: &'a Value, key: &str, min: usize, max: usize) -> Res<&'a [Value]> {
    let Some(a) = v.as_array() else {
        return err(key, "must be a list");
    };
    if a.len() < min || a.len() > max {
        return err(
            key,
            format!("must have {min} to {max} items (has {})", a.len()),
        );
    }
    Ok(a)
}

/// Fixed text: letters, digits, and `space . _ : + / @ = , -`, starting with a letter or digit, at most 128.
#[must_use]
pub fn is_token(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    s.len() <= 128
        && first.is_ascii_alphanumeric()
        && chars.all(|c| c.is_ascii_alphanumeric() || " _.:+/@=,-".contains(c))
}

/// An absolute path of safe characters with no `..` segment.
#[must_use]
pub fn is_safe_path(s: &str) -> bool {
    s.len() >= 2
        && s.len() <= 256
        && s.starts_with('/')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "_.:@+/=-".contains(c))
        && !s.split('/').any(|seg| seg == "..")
}

fn token(v: &Value, key: &str) -> Res<String> {
    let s = text(v, key, 128)?;
    if is_token(s) {
        Ok(s.to_owned())
    } else {
        err(
            key,
            "must be fixed text: letters, digits and . _ : + / @ = , - and space, starting with a letter or digit",
        )
    }
}

fn path(v: &Value, key: &str) -> Res<String> {
    let s = text(v, key, 256)?;
    if is_safe_path(s) {
        Ok(s.to_owned())
    } else {
        err(
            key,
            "must be an absolute path of letters, digits and . _ : @ + / = - with no `..` segment",
        )
    }
}

fn pattern(v: &Value, key: &str) -> Res<String> {
    let s = text(v, key, 64)?;
    let stripped = s.replacen("{nn}", "", 1);
    if stripped
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c))
    {
        Ok(s.to_owned())
    } else {
        err(
            key,
            "must be a pattern name of letters, digits and . _ - with at most one {nn}",
        )
    }
}

/// One program argument: fixed text as for a token, but it may start with `-` or `/`.
#[must_use]
pub fn is_arg(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || " _.:+/@=,-".contains(c))
}

fn argv(v: &Value, key: &str) -> Res<Vec<String>> {
    array(v, key, 1, 32)?
        .iter()
        .enumerate()
        .map(|(i, x)| {
            let k = format!("{key}[{i}]");
            let s = text(x, &k, 128)?;
            if is_arg(s) {
                Ok(s.to_owned())
            } else {
                err(&k, "must be fixed text: letters, digits and . _ : + / @ = , - and space. No shell reads it, so this keeps a board file plain; it is not a security check, because a board file is trusted and chooses the programs that run")
            }
        })
        .collect()
}

fn pcm_format(v: &Value, key: &str) -> Res<PcmFormat> {
    Ok(match choice(v, key, &["S16_LE", "S24_3LE", "S32_LE"])? {
        "S16_LE" => PcmFormat::S16Le,
        "S24_3LE" => PcmFormat::S24_3Le,
        _ => PcmFormat::S32Le,
    })
}

fn audio_kind(v: &Value, key: &str) -> Res<AudioKind> {
    Ok(match choice(v, key, &["default", "command", "alsa"])? {
        "default" => AudioKind::Default,
        "command" => AudioKind::Command,
        _ => AudioKind::Alsa,
    })
}

fn rate(v: &Value, key: &str) -> Res<u32> {
    let n = int(v, key, 0, u64::from(u32::MAX))?;
    if [16_000, 32_000, 44_100, 48_000].contains(&n) {
        Ok(n as u32)
    } else {
        err(
            key,
            format!("must be one of 16000, 32000, 44100, 48000 (got {n})"),
        )
    }
}

fn channel_list(v: &Value, key: &str, min: usize, max: usize, channels: u32) -> Res<Vec<u32>> {
    let mut out = Vec::new();
    for (i, x) in array(v, key, min, max)?.iter().enumerate() {
        let k = format!("{key}[{i}]");
        let n = int(x, &k, 0, 15)? as u32;
        if n >= channels {
            return err(
                &k,
                format!("channel {n} does not exist (channels is {channels})"),
            );
        }
        if out.contains(&n) {
            return err(&k, format!("channel {n} is listed twice"));
        }
        out.push(n);
    }
    Ok(out)
}

fn parse_capture(v: &Value) -> Res<Capture> {
    let mut o = Obj::new(v, "audio.capture")?;
    let kind = audio_kind(o.req("kind")?, "audio.capture.kind")?;
    let device = o
        .opt("device")
        .map(|d| token(d, "audio.capture.device"))
        .transpose()?;
    let command = o
        .opt("command")
        .map(|c| argv(c, "audio.capture.command"))
        .transpose()?;
    let rate = rate(o.req("rate")?, "audio.capture.rate")?;
    let format = pcm_format(o.req("format")?, "audio.capture.format")?;
    let channels = int(o.req("channels")?, "audio.capture.channels", 1, 16)? as u32;
    let mic_channels = channel_list(
        o.req("mic_channels")?,
        "audio.capture.mic_channels",
        1,
        16,
        channels,
    )?;
    let reference_channels = match o.opt("reference_channels") {
        Some(r) => channel_list(r, "audio.capture.reference_channels", 0, 4, channels)?,
        None => Vec::new(),
    };
    o.finish()?;
    if let Some(r) = reference_channels.iter().find(|r| mic_channels.contains(r)) {
        return err(
            "audio.capture.reference_channels",
            format!("channel {r} is also a microphone channel"),
        );
    }
    match (kind, &command) {
        (AudioKind::Command, None) => {
            return err(
                "audio.capture.command",
                "is required when kind is `command`",
            );
        }
        (k, Some(_)) if k != AudioKind::Command => {
            return err(
                "audio.capture.command",
                "is allowed only when kind is `command`",
            );
        }
        _ => {}
    }
    Ok(Capture {
        kind,
        device,
        command: command.unwrap_or_default(),
        rate,
        format,
        channels,
        mic_channels,
        reference_channels,
    })
}

fn parse_playback(v: &Value) -> Res<Playback> {
    let mut o = Obj::new(v, "audio.playback")?;
    let kind = audio_kind(o.req("kind")?, "audio.playback.kind")?;
    let device = o
        .opt("device")
        .map(|d| token(d, "audio.playback.device"))
        .transpose()?;
    let command = o
        .opt("command")
        .map(|c| argv(c, "audio.playback.command"))
        .transpose()?;
    let rate = rate(o.req("rate")?, "audio.playback.rate")?;
    let format = pcm_format(o.req("format")?, "audio.playback.format")?;
    let channels = int(o.req("channels")?, "audio.playback.channels", 1, 8)? as u32;
    let volume_control = o
        .opt("volume_control")
        .map(|d| token(d, "audio.playback.volume_control"))
        .transpose()?;
    let (duck, duck_level_pct) = match o.opt("duck") {
        None => (DuckKind::None, 30),
        Some(d) => {
            let mut d_o = Obj::new(d, "audio.playback.duck")?;
            let kind = match choice(
                d_o.req("kind")?,
                "audio.playback.duck.kind",
                &["none", "software", "mixer"],
            )? {
                "none" => DuckKind::None,
                "software" => DuckKind::Software,
                _ => DuckKind::Mixer,
            };
            let level = match d_o.opt("level_pct") {
                Some(l) => int(l, "audio.playback.duck.level_pct", 0, 100)? as u8,
                None => 30,
            };
            d_o.finish()?;
            (kind, level)
        }
    };
    o.finish()?;
    match (kind, &command) {
        (AudioKind::Command, None) => {
            return err(
                "audio.playback.command",
                "is required when kind is `command`",
            );
        }
        (k, Some(_)) if k != AudioKind::Command => {
            return err(
                "audio.playback.command",
                "is allowed only when kind is `command`",
            );
        }
        _ => {}
    }
    if duck == DuckKind::Mixer && volume_control.is_none() {
        return err(
            "audio.playback.volume_control",
            "is required when duck.kind is `mixer`",
        );
    }
    Ok(Playback {
        kind,
        device,
        command: command.unwrap_or_default(),
        rate,
        format,
        channels,
        volume_control,
        duck,
        duck_level_pct,
    })
}

fn parse_audio(v: &Value) -> Res<AudioSpec> {
    let mut o = Obj::new(v, "audio")?;
    let capture = parse_capture(o.req("capture")?)?;
    let playback = parse_playback(o.req("playback")?)?;
    let mut mixer_routes = Vec::new();
    if let Some(m) = o.opt("mixer_routes") {
        for (i, r) in array(m, "audio.mixer_routes", 0, 64)?.iter().enumerate() {
            let p = format!("audio.mixer_routes[{i}]");
            let mut ro = Obj::new(r, &p)?;
            let control = token(ro.req("control")?, &format!("{p}.control"))?;
            let value = token(ro.req("value")?, &format!("{p}.value"))?;
            ro.finish()?;
            mixer_routes.push(MixerRoute { control, value });
        }
    }
    o.finish()?;
    Ok(AudioSpec {
        capture,
        playback,
        mixer_routes,
    })
}

fn parse_hardware(v: &Value) -> Res<Hardware> {
    let mut o = Obj::new(v, "hardware")?;
    let arch = match choice(
        o.req("arch")?,
        "hardware.arch",
        &["armv7", "aarch64", "x86_64", "riscv64"],
    )? {
        "armv7" => Arch::Armv7,
        "aarch64" => Arch::Aarch64,
        "x86_64" => Arch::X86_64,
        _ => Arch::Riscv64,
    };
    let ram_mb = int(o.req("ram_mb")?, "hardware.ram_mb", 256, 1_048_576).or_else(|e| {
        match o.map.get("ram_mb").and_then(Value::as_u64) {
            Some(n) if n < 256 => err(
                "hardware.ram_mb",
                "below the 256 MB minimum: the board is unsupported (spec 0006 section 6)",
            ),
            _ => Err(e),
        }
    })?;
    let microphones = int(o.req("microphones")?, "hardware.microphones", 1, 16)?;
    let speakers = int(o.req("speakers")?, "hardware.speakers", 1, 8)?;
    o.finish()?;
    Ok(Hardware {
        arch,
        ram_mb,
        microphones,
        speakers,
    })
}

fn parse_detect(v: &Value) -> Res<Vec<DetectFile>> {
    let mut o = Obj::new(v, "detect")?;
    let mut out = Vec::new();
    for (i, f) in array(o.req("files")?, "detect.files", 1, 8)?
        .iter()
        .enumerate()
    {
        let p = format!("detect.files[{i}]");
        let mut fo = Obj::new(f, &p)?;
        let pth = path(fo.req("path")?, &format!("{p}.path"))?;
        let equals = text(fo.req("equals")?, &format!("{p}.equals"), 128)?.to_owned();
        fo.finish()?;
        out.push(DetectFile { path: pth, equals });
    }
    o.finish()?;
    Ok(out)
}

fn parse_buttons(v: &Value) -> Res<Vec<ButtonSpec>> {
    let mut out = Vec::new();
    for (i, b) in array(v, "buttons", 0, 8)?.iter().enumerate() {
        let p = format!("buttons[{i}]");
        let mut o = Obj::new(b, &p)?;
        let name = choice(o.req("name")?, &format!("{p}.name"), &BUTTONS)?.to_owned();
        let sp = format!("{p}.source");
        let sv = o.req("source")?;
        let mut so = Obj::new(sv, &sp)?;
        let source = match choice(so.req("kind")?, &format!("{sp}.kind"), &["evdev", "key"])? {
            "evdev" => {
                let node = path(so.req("node")?, &format!("{sp}.node"))?;
                let code = int(so.req("code")?, &format!("{sp}.code"), 1, 767)? as u32;
                ButtonSource::Evdev { node, code }
            }
            _ => {
                let k = text(so.req("key")?, &format!("{sp}.key"), 1)?;
                let mut cs = k.chars();
                match (cs.next(), cs.next()) {
                    (Some(c), None) if c.is_ascii_lowercase() || c.is_ascii_digit() => {
                        ButtonSource::Key(c)
                    }
                    _ => {
                        return err(
                            &format!("{sp}.key"),
                            "must be one lowercase letter or digit",
                        );
                    }
                }
            }
        };
        so.finish()?;
        o.finish()?;
        for (j, earlier) in out.iter().enumerate() {
            let earlier: &ButtonSpec = earlier;
            if earlier.name == name {
                return err(
                    &format!("{p}.name"),
                    format!("button `{name}` is already declared in buttons[{j}]"),
                );
            }
            match (&earlier.source, &source) {
                (ButtonSource::Key(a), ButtonSource::Key(b)) if a == b => {
                    return err(
                        &format!("{sp}.key"),
                        format!("key `{a}` is already used by buttons[{j}]"),
                    );
                }
                (
                    ButtonSource::Evdev { node: n1, code: c1 },
                    ButtonSource::Evdev { node: n2, code: c2 },
                ) if n1 == n2 && c1 == c2 => {
                    return err(
                        &sp,
                        format!("{n1} code {c1} is already used by buttons[{j}]"),
                    );
                }
                _ => {}
            }
        }
        out.push(ButtonSpec { name, source });
    }
    Ok(out)
}

fn parse_gestures(v: &Value) -> Res<Gestures> {
    let mut o = Obj::new(v, "gestures")?;
    let d = Gestures::default();
    let long_press_ms = match o.opt("long_press_ms") {
        Some(x) => int(x, "gestures.long_press_ms", 200, 3000)? as u32,
        None => d.long_press_ms,
    };
    let double_press_ms = match o.opt("double_press_ms") {
        Some(x) => int(x, "gestures.double_press_ms", 100, 1000)? as u32,
        None => d.double_press_ms,
    };
    o.finish()?;
    Ok(Gestures {
        long_press_ms,
        double_press_ms,
    })
}

fn parse_actions(v: &Value, buttons: &[ButtonSpec]) -> Res<Vec<(String, String)>> {
    let Some(map) = v.as_object() else {
        return err("actions", "must be an object");
    };
    let mut out = Vec::new();
    for (k, val) in map {
        let key = format!("actions.{k}");
        let Some((button, gesture)) = k.split_once('.') else {
            return err(&key, "must be `<button>.<gesture>`");
        };
        if !BUTTONS.contains(&button) || !GESTURES.contains(&gesture) {
            return err(
                &key,
                format!(
                    "must be `<button>.<gesture>` with a button from {} and a gesture from {}",
                    BUTTONS.join(", "),
                    GESTURES.join(", ")
                ),
            );
        }
        let Some(spec) = buttons.iter().find(|b| b.name == button) else {
            return err(
                &key,
                format!("button `{button}` is not declared in `buttons`"),
            );
        };
        if gesture == "long_press" && matches!(spec.source, ButtonSource::Key(_)) {
            // A typed key arrives as a press and a release at the same instant, so a hold cannot be measured.
            return err(
                &key,
                format!(
                    "button `{button}` is a terminal key, which has no release to time; a long press needs an input-device button"
                ),
            );
        }
        let action = choice(val, &key, &ACTIONS)?;
        out.push((k.clone(), action.to_owned()));
    }
    Ok(out)
}

fn parse_leds(v: &Value) -> Res<LedSpec> {
    let mut o = Obj::new(v, "leds")?;
    let driver = match choice(
        o.req("driver")?,
        "leds.driver",
        &["console", "ledctrl", "frame", "sysfs"],
    )? {
        "console" => LedDriverKind::Console,
        "ledctrl" => LedDriverKind::Ledctrl,
        "frame" => LedDriverKind::Frame,
        _ => LedDriverKind::Sysfs,
    };
    let count = o
        .opt("count")
        .map(|c| int(c, "leds.count", 1, 256).map(|n| n as u32))
        .transpose()?;
    let brightness_pct = match o.opt("brightness_pct") {
        Some(b) => int(b, "leds.brightness_pct", 0, 100)? as u8,
        None => DEFAULT_LED_BRIGHTNESS_PCT,
    };
    let quiet_brightness_pct = match o.opt("quiet_brightness_pct") {
        Some(b) => int(b, "leds.quiet_brightness_pct", 0, 100)? as u8,
        None => 10,
    };
    let mut states = Vec::new();
    if let Some(s) = o.opt("states") {
        let Some(map) = s.as_object() else {
            return err("leds.states", "must be an object");
        };
        for (k, val) in map {
            let key = format!("leds.states.{k}");
            if !LED_STATES.contains(&k.as_str()) {
                return err(
                    &key,
                    format!(
                        "unknown LED state; known states are {}",
                        LED_STATES.join(", ")
                    ),
                );
            }
            states.push((k.clone(), pattern(val, &key)?));
        }
    }
    let frame = match o.opt("frame") {
        None => None,
        Some(f) => {
            let mut fo = Obj::new(f, "leds.frame")?;
            let pth = path(fo.req("path")?, "leds.frame.path")?;
            let enc = choice(
                fo.req("encoding")?,
                "leds.frame.encoding",
                &["rgb444_text", "rgb888_text", "rgb888_binary"],
            )?
            .to_owned();
            fo.finish()?;
            Some((pth, enc))
        }
    };
    o.finish()?;
    if driver == LedDriverKind::Frame && frame.is_none() {
        return err("leds.frame", "is required when driver is `frame`");
    }
    Ok(LedSpec {
        driver,
        count,
        brightness_pct,
        quiet_brightness_pct,
        states,
        frame,
    })
}

fn parse_mute(v: &Value) -> Res<MuteSpec> {
    let mut o = Obj::new(v, "mute")?;
    let kind = match choice(
        o.req("kind")?,
        "mute.kind",
        &["none", "software", "hardware"],
    )? {
        "none" => MuteKind::None,
        "software" => MuteKind::Software,
        _ => MuteKind::Hardware,
    };
    let state_file = o
        .opt("state_file")
        .map(|p| path(p, "mute.state_file"))
        .transpose()?;
    o.finish()?;
    Ok(MuteSpec { kind, state_file })
}

fn parse_features(v: &Value) -> Res<Features> {
    let mut o = Obj::new(v, "features")?;
    let mut flag = |k: &'static str| -> Res<bool> {
        o.opt(k)
            .map_or(Ok(false), |x| boolean(x, &format!("features.{k}")))
    };
    let f = Features {
        aec: flag("aec")?,
        on_device_wake: flag("on_device_wake")?,
        bluetooth_sink: flag("bluetooth_sink")?,
        bluetooth_source: flag("bluetooth_source")?,
        spotify_connect: flag("spotify_connect")?,
    };
    o.finish()?;
    Ok(f)
}

fn parse_takeover(v: &Value) -> Res<Takeover> {
    let mut o = Obj::new(v, "takeover")?;
    let mut t = Takeover::default();
    if let Some(s) = o.opt("stop") {
        for (i, x) in array(s, "takeover.stop", 0, 32)?.iter().enumerate() {
            t.stop.push(token(x, &format!("takeover.stop[{i}]"))?);
        }
    }
    if let Some(s) = o.opt("forbidden") {
        for (i, x) in array(s, "takeover.forbidden", 0, 64)?.iter().enumerate() {
            t.forbidden
                .push(path(x, &format!("takeover.forbidden[{i}]"))?);
        }
    }
    o.finish()?;
    Ok(t)
}

fn parse_wifi(v: &Value) -> Res<WifiSpec> {
    let mut o = Obj::new(v, "wifi")?;
    let interface = token(o.req("interface")?, "wifi.interface")?;
    let control_dir = o
        .opt("control_dir")
        .map(|p| path(p, "wifi.control_dir"))
        .transpose()?;
    o.finish()?;
    Ok(WifiSpec {
        interface,
        control_dir,
    })
}

fn parse_clock(v: &Value) -> Res<ClockSpec> {
    let mut o = Obj::new(v, "clock")?;
    let floor = match o.opt("floor") {
        None => None,
        Some(f) => {
            let s = text(f, "clock.floor", 10)?;
            let b = s.as_bytes();
            let ok = b.len() == 10
                && b.iter().enumerate().all(|(i, c)| {
                    if i == 4 || i == 7 {
                        *c == b'-'
                    } else {
                        c.is_ascii_digit()
                    }
                });
            if !ok {
                return err("clock.floor", "must be a date like 2023-11-14");
            }
            Some(s.to_owned())
        }
    };
    let https_date = match o.opt("prefer_verified_by") {
        None => false,
        Some(x) => choice(x, "clock.prefer_verified_by", &["none", "https-date"])? == "https-date",
    };
    o.finish()?;
    Ok(ClockSpec { floor, https_date })
}

fn parse_update(v: &Value) -> Res<UpdateMode> {
    let mut o = Obj::new(v, "update")?;
    let m = choice(o.req("mode")?, "update.mode", &["ab", "manual"])?;
    let mode = if m == "ab" {
        UpdateMode::Ab
    } else {
        UpdateMode::Manual
    };
    o.finish()?;
    Ok(mode)
}

impl BoardFile {
    /// Parse and validate a board file from JSON text.
    ///
    /// # Errors
    /// [`BoardError`] naming the first offending key.
    pub fn parse(json: &str) -> Result<Self, BoardError> {
        let value: Value = serde_json::from_str(json).map_err(|e| BoardError {
            key: String::new(),
            message: format!("not valid JSON: {e}"),
        })?;
        Self::from_value(&value)
    }

    /// Validate a parsed JSON value.
    ///
    /// # Errors
    /// [`BoardError`] naming the first offending key.
    pub fn from_value(value: &Value) -> Result<Self, BoardError> {
        let mut o = Obj::new(value, "")?;
        let version = o.req("schema")?;
        match version.as_u64() {
            Some(SCHEMA_VERSION) => {}
            Some(n) => {
                return err(
                    "schema",
                    format!(
                        "version {n} is not supported; this build reads version {SCHEMA_VERSION}"
                    ),
                );
            }
            None => return err("schema", "must be a whole number"),
        }
        let board = {
            let b = text(o.req("board")?, "board", 32)?;
            let ok = b.len() >= 2
                && b.chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
                && b.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
            if !ok {
                return err(
                    "board",
                    "must be 2 to 32 characters: lowercase letters, digits and -",
                );
            }
            b.to_owned()
        };
        let description = match o.opt("description") {
            Some(d) => text(d, "description", 300)?.to_owned(),
            None => String::new(),
        };
        let hardware = parse_hardware(o.req("hardware")?)?;
        let detect = match o.opt("detect") {
            Some(d) => parse_detect(d)?,
            None => Vec::new(),
        };
        let audio = parse_audio(o.req("audio")?)?;
        let buttons = match o.opt("buttons") {
            Some(b) => parse_buttons(b)?,
            None => Vec::new(),
        };
        let gestures = match o.opt("gestures") {
            Some(g) => parse_gestures(g)?,
            None => Gestures::default(),
        };
        let actions = match o.opt("actions") {
            Some(a) => parse_actions(a, &buttons)?,
            None => Vec::new(),
        };
        let leds = o.opt("leds").map(parse_leds).transpose()?;
        let mute = match o.opt("mute") {
            Some(m) => parse_mute(m)?,
            None => MuteSpec {
                kind: MuteKind::None,
                state_file: None,
            },
        };
        let wifi = o.opt("wifi").map(parse_wifi).transpose()?;
        let clock = o.opt("clock").map(parse_clock).transpose()?;
        let features = match o.opt("features") {
            Some(f) => parse_features(f)?,
            None => Features::default(),
        };
        let update = parse_update(o.req("update")?)?;
        let takeover = match o.opt("takeover") {
            Some(t) => parse_takeover(t)?,
            None => Takeover::default(),
        };
        o.finish()?;
        Ok(Self {
            board,
            description,
            hardware,
            detect,
            audio,
            buttons,
            gestures,
            actions,
            leds,
            mute,
            wifi,
            clock,
            features,
            update,
            takeover,
        })
    }

    /// Whether a path may be touched: it must be safe and not in the board's forbidden list (REQ-DEV-019).
    #[must_use]
    pub fn path_allowed(&self, p: &str) -> bool {
        is_safe_path(p) && !self.takeover.forbidden.iter().any(|f| f == p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOT: &str = include_str!("../../../boards/echo-dot2.json");
    const DESKTOP: &str = include_str!("../../../boards/desktop.json");
    const PI: &str = include_str!("../../../boards/raspberry-pi.json");

    fn with(json: &str, edit: impl FnOnce(&mut Value)) -> String {
        let mut v: Value = serde_json::from_str(json).unwrap();
        edit(&mut v);
        v.to_string()
    }

    fn expect_key(json: &str, key: &str) {
        let e = BoardFile::parse(json).unwrap_err();
        assert_eq!(e.key, key, "{e}");
        assert!(e.to_string().contains(key) || key.is_empty());
    }

    #[test]
    fn req_dev_008_shipped_examples_validate() {
        for (name, json) in [
            ("echo-dot2", DOT),
            ("desktop", DESKTOP),
            ("raspberry-pi", PI),
        ] {
            let b = BoardFile::parse(json).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(b.board, name);
        }
    }

    #[test]
    fn req_dev_008_dot_values_are_read() {
        let b = BoardFile::parse(DOT).unwrap();
        assert_eq!(b.hardware.arch, Arch::Armv7);
        assert_eq!(b.audio.capture.channels, 9);
        assert_eq!(b.audio.capture.format, PcmFormat::S24_3Le);
        assert_eq!(b.buttons.len(), 4);
        assert_eq!(b.mute.kind, MuteKind::Software);
        assert_eq!(b.update, UpdateMode::Ab);
        assert!(!b.path_allowed("/sys/devices/soc/10010000.keypad/amz_privacy/power_button_state"));
        assert!(b.path_allowed("/dev/input/event1"));
    }

    #[test]
    fn req_dev_008_an_unknown_key_stops_loading_and_names_it() {
        expect_key(
            &with(DESKTOP, |v| v["colour"] = Value::from("red")),
            "colour",
        );
        expect_key(
            &with(DESKTOP, |v| v["audio"]["capture"]["gain"] = Value::from(3)),
            "audio.capture.gain",
        );
        expect_key(
            &with(DOT, |v| v["buttons"][0]["source"]["extra"] = Value::from(1)),
            "buttons[0].source.extra",
        );
    }

    #[test]
    fn req_dev_008_impossible_values_stop_loading_and_name_the_key() {
        expect_key(
            &with(DESKTOP, |v| {
                v["audio"]["capture"]["rate"] = Value::from(8000)
            }),
            "audio.capture.rate",
        );
        expect_key(
            &with(DESKTOP, |v| v["leds"]["driver"] = Value::from("neon")),
            "leds.driver",
        );
        expect_key(
            &with(DESKTOP, |v| {
                v["gestures"] = serde_json::json!({"long_press_ms": 5})
            }),
            "gestures.long_press_ms",
        );
        expect_key(
            &with(DOT, |v| {
                v["audio"]["capture"]["mic_channels"] = serde_json::json!([9])
            }),
            "audio.capture.mic_channels[0]",
        );
        expect_key(
            &with(DOT, |v| v["leds"]["states"]["sparkle"] = Value::from("x")),
            "leds.states.sparkle",
        );
        expect_key(
            &with(DOT, |v| {
                v["takeover"]["forbidden"][0] = Value::from("/sys/../etc/passwd")
            }),
            "takeover.forbidden[0]",
        );
    }

    #[test]
    fn req_dev_008_a_future_schema_version_is_refused() {
        expect_key(&with(DESKTOP, |v| v["schema"] = Value::from(2)), "schema");
        expect_key(
            &with(DESKTOP, |v| {
                v.as_object_mut().unwrap().remove("schema");
            }),
            "schema",
        );
    }

    #[test]
    fn req_dev_012_a_claim_below_the_minimum_hardware_is_refused() {
        expect_key(
            &with(DESKTOP, |v| v["hardware"]["ram_mb"] = Value::from(128)),
            "hardware.ram_mb",
        );
        expect_key(
            &with(DESKTOP, |v| v["hardware"]["microphones"] = Value::from(0)),
            "hardware.microphones",
        );
        expect_key(
            &with(DESKTOP, |v| v["hardware"]["speakers"] = Value::from(0)),
            "hardware.speakers",
        );
    }

    #[test]
    fn req_dev_008_commands_are_data_only() {
        // Shell characters and placeholders other than {nn} are refused.
        let cmd = |arg: &str| {
            with(DESKTOP, |v| {
                v["audio"]["capture"]["kind"] = Value::from("command");
                v["audio"]["capture"]["command"] = serde_json::json!(["arecord", arg]);
            })
        };
        assert!(BoardFile::parse(&cmd("-q")).is_ok());
        assert!(BoardFile::parse(&cmd("--rate=16000")).is_ok());
        expect_key(&cmd("$(reboot)"), "audio.capture.command[1]");
        expect_key(&cmd(""), "audio.capture.command[1]");
        expect_key(&cmd("a;b"), "audio.capture.command[1]");
        expect_key(&cmd("`id`"), "audio.capture.command[1]");
        expect_key(
            &with(DOT, |v| v["leds"]["states"]["idle"] = Value::from("a;b")),
            "leds.states.idle",
        );
        expect_key(
            &with(DOT, |v| {
                v["leds"]["states"]["idle"] = Value::from("{nn}{nn}")
            }),
            "leds.states.idle",
        );
        // A command without kind = command, and kind = command without one, are both refused.
        expect_key(
            &with(DESKTOP, |v| {
                v["audio"]["capture"]["command"] = serde_json::json!(["arecord"])
            }),
            "audio.capture.command",
        );
        expect_key(
            &with(DESKTOP, |v| {
                v["audio"]["capture"]["kind"] = Value::from("command")
            }),
            "audio.capture.command",
        );
    }

    #[test]
    fn req_dev_008_buttons_are_distinct_and_key_buttons_have_no_long_press() {
        // The same logical button twice.
        expect_key(
            &with(DESKTOP, |v| v["buttons"][1]["name"] = Value::from("action")),
            "buttons[1].name",
        );
        // Two buttons on the same terminal key.
        expect_key(
            &with(DESKTOP, |v| {
                v["buttons"][1]["source"]["key"] = Value::from("t")
            }),
            "buttons[1].source.key",
        );
        // Two buttons on the same input code of the same device.
        expect_key(
            &with(DOT, |v| {
                let first = v["buttons"][0]["source"].clone();
                v["buttons"][1]["source"] = first;
            }),
            "buttons[1].source",
        );
        // A key press has no release to time, so a long press could never fire.
        expect_key(
            &with(DESKTOP, |v| {
                v["actions"]["action.long_press"] = Value::from("stop")
            }),
            "actions.action.long_press",
        );
        // The same mapping is fine for a button read from an input device (the Dot file has one).
        assert!(BoardFile::parse(DOT).is_ok());
    }

    #[test]
    fn req_dev_012_the_ram_message_matches_the_direction_of_the_error() {
        let low = BoardFile::parse(&with(DESKTOP, |v| {
            v["hardware"]["ram_mb"] = Value::from(128)
        }))
        .unwrap_err();
        assert!(
            low.to_string().contains("below the 256 MB minimum"),
            "{low}"
        );
        let high = BoardFile::parse(&with(DESKTOP, |v| {
            v["hardware"]["ram_mb"] = Value::from(5_000_000)
        }))
        .unwrap_err();
        assert_eq!(high.key, "hardware.ram_mb");
        assert!(!high.to_string().contains("below"), "{high}");
    }

    #[test]
    fn req_dev_008_cross_checks_name_the_key() {
        expect_key(
            &with(DESKTOP, |v| {
                v["actions"]["volume_up.press"] = Value::from("volume_up")
            }),
            "actions.volume_up.press",
        );
        expect_key(
            &with(DOT, |v| v["actions"]["mute.tap"] = Value::from("stop")),
            "actions.mute.tap",
        );
        expect_key(
            &with(DOT, |v| {
                v["audio"]["capture"]["reference_channels"] = serde_json::json!([0])
            }),
            "audio.capture.reference_channels",
        );
        expect_key(
            &with(DOT, |v| v["leds"]["driver"] = Value::from("frame"))
                .replace("\"frame\":{", "\"fram\":{"),
            "leds.fram",
        );
    }

    #[test]
    fn req_dev_008_not_json_or_not_an_object() {
        expect_key("{", "");
        expect_key("[]", "");
    }

    #[test]
    fn schema_property_names_match_the_loader() {
        // Keeps schemas/board/board-1.schema.json and this loader in step: every top-level property in the
        // schema is accepted by the loader, and the loader accepts nothing else at the top level.
        let schema: Value =
            serde_json::from_str(include_str!("../../../schemas/board/board-1.schema.json"))
                .unwrap();
        let mut props: Vec<String> = schema["properties"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        props.sort();
        let mut known = vec![
            "schema",
            "board",
            "description",
            "hardware",
            "detect",
            "audio",
            "buttons",
            "gestures",
            "actions",
            "leds",
            "mute",
            "wifi",
            "clock",
            "features",
            "update",
            "takeover",
        ];
        known.sort_unstable();
        assert_eq!(props, known);
        // And the LED states and actions agree.
        let states: Vec<&str> = schema["$defs"]["led_state"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_str().unwrap())
            .collect();
        assert_eq!(states, LED_STATES);
        let actions: Vec<&str> = schema["properties"]["actions"]["additionalProperties"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_str().unwrap())
            .collect();
        assert_eq!(actions, ACTIONS);
    }
}
