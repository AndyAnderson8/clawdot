//! Typed protocol messages layered on [`Frame`] (spec 0003).
//!
//! Wyoming event names are used where they fit (`audio-*`, `detection`, `error`, `ping`, `pong`);
//! ClawDot additions live under the `clawdot.` namespace (REQ-SAT-011). Unknown events are kept as
//! [`Message::Unknown`] so peers can ignore them (REQ-SAT-009).

use crate::frame::Frame;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Map, Value};
use std::fmt;

/// Event names on the wire.
pub mod kind {
    /// Satellite -> core handshake.
    pub const HELLO: &str = "clawdot.hello";
    /// Core -> satellite acceptance.
    pub const WELCOME: &str = "clawdot.welcome";
    /// Core -> satellite refusal.
    pub const REJECT: &str = "clawdot.reject";
    /// Keepalive request (Wyoming).
    pub const PING: &str = "ping";
    /// Keepalive reply (Wyoming).
    pub const PONG: &str = "pong";
    /// Start of an audio stream (Wyoming).
    pub const AUDIO_START: &str = "audio-start";
    /// A chunk of PCM audio (Wyoming).
    pub const AUDIO_CHUNK: &str = "audio-chunk";
    /// End of an audio stream (Wyoming).
    pub const AUDIO_STOP: &str = "audio-stop";
    /// A wake word was detected on the satellite (Wyoming `detection`).
    pub const WAKE: &str = "detection";
    /// A button was pressed on the satellite.
    pub const BUTTON: &str = "clawdot.button";
    /// The hardware or software mute changed.
    pub const MUTE_CHANGED: &str = "clawdot.mute-changed";
    /// A playback request finished.
    pub const PLAYBACK_DONE: &str = "clawdot.playback-done";
    /// A fault report (Wyoming).
    pub const ERROR: &str = "error";
    /// Core -> satellite: start sending microphone audio.
    pub const STREAM_START: &str = "clawdot.stream-start";
    /// Core -> satellite: stop sending microphone audio.
    pub const STREAM_STOP: &str = "clawdot.stream-stop";
    /// Core -> satellite: stop any playback now.
    pub const STOP_PLAYBACK: &str = "clawdot.stop-playback";
    /// Core -> satellite: set the speaker volume.
    pub const SET_VOLUME: &str = "clawdot.set-volume";
    /// Core -> satellite: show a state on LEDs or a display.
    pub const SET_INDICATOR: &str = "clawdot.set-indicator";
    /// Core -> satellite: mute or unmute the microphone.
    pub const SET_MUTE: &str = "clawdot.set-mute";
    /// Core to satellite: play an audio stream from a URL (radio).
    pub const PLAY_URL: &str = "clawdot.play-url";
    /// Core to satellite: pause or resume stream playback.
    pub const MEDIA_CONTROL: &str = "clawdot.media-control";
    /// The owner held the action button for 10 seconds (REQ-SAT-020).
    pub const ADMIN_RESET: &str = "clawdot.admin-reset";
    /// The core's answer to [`ADMIN_RESET`] (REQ-SAT-020).
    pub const ADMIN_RESET_WINDOW: &str = "clawdot.admin-reset-window";
}

/// Protocol version. While `major` is 0 the format is unstable and peers must match exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Version {
    /// Breaking changes bump this.
    pub major: u16,
    /// Backward-compatible additions bump this.
    pub minor: u16,
}

/// The version this crate speaks.
pub const PROTOCOL_VERSION: Version = Version { major: 0, minor: 1 };

impl Version {
    /// Whether two peers can talk (REQ-SAT-009): same major; and while the major is 0, same minor.
    #[must_use]
    pub fn compatible_with(self, peer: Version) -> bool {
        if self.major != peer.major {
            return false;
        }
        self.major != 0 || self.minor == peer.minor
    }
}

/// Raw PCM layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioFormat {
    /// Samples per second.
    pub rate: u32,
    /// Bytes per sample.
    pub width: u8,
    /// Channel count.
    pub channels: u8,
}

impl AudioFormat {
    /// The mandatory uplink format: 16 kHz, 16-bit, mono (REQ-SAT-004).
    pub const PCM_16K_MONO: AudioFormat = AudioFormat {
        rate: 16_000,
        width: 2,
        channels: 1,
    };
}

/// Downlink audio encodings a satellite can play (REQ-SAT-005).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioCodec {
    /// Raw PCM.
    Pcm,
    /// Opus packets.
    Opus,
}

/// What a satellite can do; the core supplies whatever is missing (REQ-SAT-003, REQ-SAT-013).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Capabilities {
    /// Wake-word detection runs on the device.
    pub wake_word: bool,
    /// Status LEDs.
    pub leds: bool,
    /// Physical buttons.
    pub buttons: bool,
    /// A screen.
    pub display: bool,
    /// A physical microphone mute.
    pub hardware_mute: bool,
    /// Acoustic echo cancellation.
    pub aec: bool,
    /// Noise suppression.
    pub noise_suppression: bool,
    /// Voice activity detection.
    pub vad: bool,
    /// Microphone-array beamforming.
    pub beamforming: bool,
    /// Downlink codecs; empty means PCM only.
    pub audio_out: Vec<AudioCodec>,
    /// The satellite can fetch and play an audio stream from a URL itself (internet radio), so the core may send
    /// `clawdot.play-url`.
    pub media_url: bool,
}

/// First message from a satellite.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    /// Protocol version the satellite speaks.
    pub protocol: Version,
    /// Stable identifier of the device.
    pub device_id: String,
    /// Friendly name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Pre-shared token for remote connections (REQ-SAT-010).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// What the device can do.
    #[serde(default)]
    pub capabilities: Capabilities,
}

// The token is a secret: a debug print shows only whether one is set.
impl std::fmt::Debug for Hello {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hello")
            .field("protocol", &self.protocol)
            .field("device_id", &self.device_id)
            .field("name", &self.name)
            .field("token", &self.token.as_ref().map(|_| "<set>"))
            .field("capabilities", &self.capabilities)
            .finish()
    }
}

/// Core's acceptance of a [`Hello`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Welcome {
    /// Version the core speaks.
    pub protocol: Version,
    /// Identifier of this connection.
    pub session_id: String,
    /// Codec the core will use for downlink audio.
    pub audio_out: AudioCodec,
}

/// Core's refusal of a [`Hello`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reject {
    /// Human-readable reason.
    pub reason: String,
}

/// Wake word detected on the satellite.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Wake {
    /// Which wake word.
    pub name: String,
    /// Milliseconds into the audio stream.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp_ms: Option<u64>,
}

/// A physical button event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Button {
    /// Which button (for example `action` or `mute`).
    pub name: String,
    /// A long press rather than a tap.
    #[serde(default)]
    pub long: bool,
}

/// A fault report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorReport {
    /// What happened.
    pub text: String,
    /// Optional machine-readable code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

/// States a satellite can show (REQ-VOI-006).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Indicator {
    /// Nothing happening.
    Idle,
    /// Wake word heard.
    WakeDetected,
    /// Capturing speech.
    Listening,
    /// Waiting for the assistant.
    Thinking,
    /// Playing a response.
    Speaking,
    /// Something went wrong.
    Error,
    /// Microphone muted.
    Muted,
    /// A notification is waiting.
    Notification,
}

/// What to do with a stream that is playing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MediaAction {
    /// Pause, keeping the position.
    Pause,
    /// Continue.
    Resume,
}

/// Longest stream URL accepted.
pub const MAX_URL: usize = 2048;
/// Longest stream title accepted.
pub const MAX_TITLE: usize = 200;

/// Every message the protocol defines.
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    /// Satellite handshake.
    Hello(Hello),
    /// Handshake accepted.
    Welcome(Welcome),
    /// Handshake refused.
    Reject(Reject),
    /// Keepalive request.
    Ping,
    /// Keepalive reply.
    Pong,
    /// Audio stream begins.
    AudioStart(AudioFormat),
    /// Audio data.
    AudioChunk {
        /// PCM layout.
        format: AudioFormat,
        /// Milliseconds into the stream, when known.
        timestamp_ms: Option<u64>,
        /// The samples.
        payload: Vec<u8>,
    },
    /// Audio stream ends.
    AudioStop,
    /// Wake word detected.
    Wake(Wake),
    /// Button event.
    Button(Button),
    /// Mute state changed.
    MuteChanged {
        /// New state.
        muted: bool,
    },
    /// Playback request finished.
    PlaybackDone {
        /// Which request, when identified.
        id: Option<String>,
    },
    /// Fault report.
    Error(ErrorReport),
    /// Start sending microphone audio.
    StreamStart,
    /// Stop sending microphone audio.
    StreamStop,
    /// Stop playing now.
    StopPlayback,
    /// Set speaker volume, 0 to 100.
    SetVolume {
        /// Percent.
        level: u8,
    },
    /// Show a state.
    SetIndicator(Indicator),
    /// Mute or unmute the microphone.
    SetMute {
        /// Desired state.
        muted: bool,
    },
    /// Play an audio stream, replacing whatever was playing. Only sent to satellites that declared `media_url`.
    PlayUrl {
        /// `http://` or `https://` stream address.
        url: String,
        /// Name to show.
        title: String,
    },
    /// Pause or resume stream playback.
    MediaControl(MediaAction),
    /// Satellite to core: the owner asks for the local administrator reset window (REQ-SAT-020).
    AdminReset,
    /// Core to satellite: how long the reset window is open, 0 when refused (REQ-SAT-020).
    AdminResetWindow {
        /// Seconds the window stays open; 0 means the request was refused.
        open_secs: u32,
        /// Plain words for a refusal.
        reason: Option<String>,
    },
    /// An event this version does not know; kept so peers can ignore it.
    Unknown(Frame),
}

/// A known event had missing or malformed fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageError {
    /// The event name.
    pub kind: String,
    /// What was wrong.
    pub reason: String,
}

impl fmt::Display for MessageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "bad `{}` message: {}", self.kind, self.reason)
    }
}

impl std::error::Error for MessageError {}

fn to_data<T: Serialize>(value: &T) -> Map<String, Value> {
    match serde_json::to_value(value) {
        Ok(Value::Object(m)) => m,
        _ => Map::new(),
    }
}

fn from_data<T: DeserializeOwned>(kind: &str, data: Map<String, Value>) -> Result<T, MessageError> {
    serde_json::from_value(Value::Object(data)).map_err(|e| MessageError {
        kind: kind.to_owned(),
        reason: e.to_string(),
    })
}

fn one(key: &str, value: Value) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert(key.to_owned(), value);
    m
}

/// Decode the value under `key` (missing counts as null) into `T`.
fn from_field<T: serde::de::DeserializeOwned>(
    name: &str,
    data: &Map<String, Value>,
    key: &str,
) -> Result<T, MessageError> {
    let v = data.get(key).cloned().unwrap_or(Value::Null);
    serde_json::from_value(v).map_err(|e| MessageError {
        kind: name.to_owned(),
        reason: e.to_string(),
    })
}

/// `set_volume`: a level from 0 to 100.
fn set_volume(name: &str, data: &Map<String, Value>) -> Result<Message, MessageError> {
    let level = data
        .get("level")
        .and_then(Value::as_u64)
        .filter(|l| *l <= 100)
        .and_then(|l| u8::try_from(l).ok())
        .ok_or_else(|| MessageError {
            kind: name.to_owned(),
            reason: "`level` must be 0 to 100".into(),
        })?;
    Ok(Message::SetVolume { level })
}

/// `play_url`: an http or https address (no spaces, bounded) and a bounded title.
fn play_url(name: &str, data: &Map<String, Value>) -> Result<Message, MessageError> {
    let bad = |reason: &str| MessageError {
        kind: name.to_owned(),
        reason: reason.into(),
    };
    let url = data.get("url").and_then(Value::as_str).unwrap_or("");
    if !(url.starts_with("http://") || url.starts_with("https://"))
        || url.len() > MAX_URL
        || url.contains(char::is_whitespace)
    {
        return Err(bad("`url` must be an http:// or https:// address"));
    }
    let title = data.get("title").and_then(Value::as_str).unwrap_or("");
    if title.chars().count() > MAX_TITLE {
        return Err(bad("`title` is too long"));
    }
    Ok(Message::PlayUrl {
        url: url.to_owned(),
        title: title.to_owned(),
    })
}

/// `admin_reset_window`: the seconds the window stays open, and an optional reason (bounded).
fn admin_reset_window(name: &str, data: &Map<String, Value>) -> Result<Message, MessageError> {
    let open_secs = data
        .get("open_secs")
        .and_then(Value::as_u64)
        .and_then(|s| u32::try_from(s).ok())
        .ok_or_else(|| MessageError {
            kind: name.to_owned(),
            reason: "`open_secs` must be a whole number".into(),
        })?;
    let reason = data
        .get("reason")
        .and_then(Value::as_str)
        .map(|r| r.chars().take(200).collect());
    Ok(Message::AdminResetWindow { open_secs, reason })
}

impl Message {
    /// Convert to a wire frame.
    #[must_use]
    pub fn to_frame(&self) -> Frame {
        match self {
            Self::Hello(m) => Frame::new(kind::HELLO).with_data(to_data(m)),
            Self::Welcome(m) => Frame::new(kind::WELCOME).with_data(to_data(m)),
            Self::Reject(m) => Frame::new(kind::REJECT).with_data(to_data(m)),
            Self::Ping => Frame::new(kind::PING),
            Self::Pong => Frame::new(kind::PONG),
            Self::AudioStart(f) => Frame::new(kind::AUDIO_START).with_data(to_data(f)),
            Self::AudioChunk {
                format,
                timestamp_ms,
                payload,
            } => {
                let mut data = to_data(format);
                if let Some(t) = timestamp_ms {
                    data.insert("timestamp".into(), Value::from(*t));
                }
                Frame::new(kind::AUDIO_CHUNK)
                    .with_data(data)
                    .with_payload(payload.clone())
            }
            Self::AudioStop => Frame::new(kind::AUDIO_STOP),
            Self::Wake(m) => Frame::new(kind::WAKE).with_data(to_data(m)),
            Self::Button(m) => Frame::new(kind::BUTTON).with_data(to_data(m)),
            Self::MuteChanged { muted } => {
                Frame::new(kind::MUTE_CHANGED).with_data(one("muted", Value::Bool(*muted)))
            }
            Self::PlaybackDone { id } => {
                let data = id
                    .as_ref()
                    .map(|i| one("id", Value::String(i.clone())))
                    .unwrap_or_default();
                Frame::new(kind::PLAYBACK_DONE).with_data(data)
            }
            Self::Error(m) => Frame::new(kind::ERROR).with_data(to_data(m)),
            Self::StreamStart => Frame::new(kind::STREAM_START),
            Self::StreamStop => Frame::new(kind::STREAM_STOP),
            Self::StopPlayback => Frame::new(kind::STOP_PLAYBACK),
            Self::SetVolume { level } => {
                Frame::new(kind::SET_VOLUME).with_data(one("level", Value::from(*level)))
            }
            Self::SetIndicator(i) => Frame::new(kind::SET_INDICATOR)
                .with_data(one("state", serde_json::to_value(i).unwrap_or(Value::Null))),
            Self::SetMute { muted } => {
                Frame::new(kind::SET_MUTE).with_data(one("muted", Value::Bool(*muted)))
            }
            Self::PlayUrl { url, title } => {
                let mut d = one("url", Value::String(url.clone()));
                d.insert("title".into(), Value::String(title.clone()));
                Frame::new(kind::PLAY_URL).with_data(d)
            }
            Self::MediaControl(a) => Frame::new(kind::MEDIA_CONTROL).with_data(one(
                "action",
                serde_json::to_value(a).unwrap_or(Value::Null),
            )),
            Self::AdminReset => Frame::new(kind::ADMIN_RESET),
            Self::AdminResetWindow { open_secs, reason } => {
                let mut d = one("open_secs", Value::from(*open_secs));
                if let Some(r) = reason {
                    d.insert("reason".into(), Value::String(r.clone()));
                }
                Frame::new(kind::ADMIN_RESET_WINDOW).with_data(d)
            }
            Self::Unknown(f) => f.clone(),
        }
    }

    /// Interpret a wire frame. Unknown event names become [`Message::Unknown`]; known events with bad
    /// fields are an error.
    ///
    /// # Errors
    /// [`MessageError`] when a known event's fields are missing or the wrong type.
    pub fn from_frame(frame: Frame) -> Result<Self, MessageError> {
        let Frame {
            kind: name,
            data,
            payload,
        } = frame;
        let tag = name.clone();
        let simple_bool = |data: &Map<String, Value>, key: &str| -> Result<bool, MessageError> {
            data.get(key)
                .and_then(Value::as_bool)
                .ok_or_else(|| MessageError {
                    kind: tag.clone(),
                    reason: format!("`{key}` must be a boolean"),
                })
        };
        Ok(match name.as_str() {
            kind::HELLO => Self::Hello(from_data(&name, data)?),
            kind::WELCOME => Self::Welcome(from_data(&name, data)?),
            kind::REJECT => Self::Reject(from_data(&name, data)?),
            kind::PING => Self::Ping,
            kind::PONG => Self::Pong,
            kind::AUDIO_START => Self::AudioStart(from_data(&name, data)?),
            kind::AUDIO_CHUNK => {
                let timestamp_ms = data.get("timestamp").and_then(Value::as_u64);
                Self::AudioChunk {
                    format: from_data(&name, data)?,
                    timestamp_ms,
                    payload,
                }
            }
            kind::AUDIO_STOP => Self::AudioStop,
            kind::WAKE => Self::Wake(from_data(&name, data)?),
            kind::BUTTON => Self::Button(from_data(&name, data)?),
            kind::MUTE_CHANGED => Self::MuteChanged {
                muted: simple_bool(&data, "muted")?,
            },
            kind::PLAYBACK_DONE => Self::PlaybackDone {
                id: data.get("id").and_then(Value::as_str).map(str::to_owned),
            },
            kind::ERROR => Self::Error(from_data(&name, data)?),
            kind::STREAM_START => Self::StreamStart,
            kind::STREAM_STOP => Self::StreamStop,
            kind::STOP_PLAYBACK => Self::StopPlayback,
            kind::SET_VOLUME => set_volume(&name, &data)?,
            kind::SET_INDICATOR => Self::SetIndicator(from_field(&name, &data, "state")?),
            kind::SET_MUTE => Self::SetMute {
                muted: simple_bool(&data, "muted")?,
            },
            kind::PLAY_URL => play_url(&name, &data)?,
            kind::MEDIA_CONTROL => Self::MediaControl(from_field(&name, &data, "action")?),
            kind::ADMIN_RESET => Self::AdminReset,
            kind::ADMIN_RESET_WINDOW => admin_reset_window(&name, &data)?,
            _ => Self::Unknown(Frame {
                kind: name,
                data,
                payload,
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{Decoder, Limits, encode};

    #[test]
    fn hello_debug_does_not_print_the_token() {
        let Message::Hello(h) = all_messages().remove(0) else {
            panic!("first message is a hello")
        };
        let shown = format!("{h:?} {:?}", Message::Hello(h.clone()));
        assert!(!shown.contains("secret"), "{shown}");
        assert!(shown.contains("<set>"), "{shown}");
    }

    fn round_trip(msg: &Message) -> Message {
        let bytes = encode(&msg.to_frame()).unwrap();
        let mut d = Decoder::new(Limits::default());
        d.push(&bytes);
        let frame = d.next_frame().unwrap().unwrap();
        Message::from_frame(frame).unwrap()
    }

    fn all_messages() -> Vec<Message> {
        vec![
            Message::Hello(Hello {
                protocol: PROTOCOL_VERSION,
                device_id: "dot-1".into(),
                name: Some("Kitchen".into()),
                token: Some("secret".into()),
                capabilities: Capabilities {
                    wake_word: true,
                    leds: true,
                    audio_out: vec![AudioCodec::Pcm, AudioCodec::Opus],
                    ..Capabilities::default()
                },
            }),
            Message::Welcome(Welcome {
                protocol: PROTOCOL_VERSION,
                session_id: "s1".into(),
                audio_out: AudioCodec::Opus,
            }),
            Message::Reject(Reject {
                reason: "bad token".into(),
            }),
            Message::Ping,
            Message::Pong,
            Message::AudioStart(AudioFormat::PCM_16K_MONO),
            Message::AudioChunk {
                format: AudioFormat::PCM_16K_MONO,
                timestamp_ms: Some(40),
                payload: vec![0, 1, 2, 3],
            },
            Message::AudioStop,
            Message::Wake(Wake {
                name: "hey_clawdot".into(),
                timestamp_ms: None,
            }),
            Message::Button(Button {
                name: "mute".into(),
                long: true,
            }),
            Message::MuteChanged { muted: true },
            Message::PlaybackDone {
                id: Some("tts-7".into()),
            },
            Message::Error(ErrorReport {
                text: "mic failed".into(),
                code: Some("mic".into()),
            }),
            Message::StreamStart,
            Message::StreamStop,
            Message::StopPlayback,
            Message::SetVolume { level: 55 },
            Message::PlayUrl {
                url: "http://radio.example/jazz".into(),
                title: "Jazz".into(),
            },
            Message::MediaControl(MediaAction::Pause),
            Message::MediaControl(MediaAction::Resume),
            Message::AdminReset,
            Message::AdminResetWindow {
                open_secs: 300,
                reason: None,
            },
            Message::AdminResetWindow {
                open_secs: 0,
                reason: Some("try again in a minute".into()),
            },
            Message::SetIndicator(Indicator::WakeDetected),
            Message::SetMute { muted: false },
        ]
    }

    #[test]
    fn every_message_round_trips_through_the_wire() {
        for msg in all_messages() {
            assert_eq!(round_trip(&msg), msg);
        }
    }

    #[test]
    fn unknown_events_are_preserved_not_rejected() {
        let frame = Frame::new("clawdot.from-the-future");
        assert_eq!(
            Message::from_frame(frame.clone()).unwrap(),
            Message::Unknown(frame)
        );
    }

    #[test]
    fn extra_fields_are_ignored_for_forward_compatibility() {
        let mut data = Map::new();
        data.insert("name".into(), Value::String("hey".into()));
        data.insert("new_field".into(), Value::Bool(true));
        let msg = Message::from_frame(Frame::new(kind::WAKE).with_data(data)).unwrap();
        assert!(matches!(msg, Message::Wake(w) if w.name == "hey"));
    }

    #[test]
    fn known_events_with_bad_fields_are_errors() {
        assert!(Message::from_frame(Frame::new(kind::HELLO)).is_err());
        assert!(Message::from_frame(Frame::new(kind::SET_MUTE)).is_err());
        let mut data = Map::new();
        data.insert("level".into(), Value::from(101));
        assert!(Message::from_frame(Frame::new(kind::SET_VOLUME).with_data(data)).is_err());
    }

    #[test]
    fn capabilities_default_to_none_so_a_minimal_satellite_works() {
        // REQ-SAT-013: only the mandatory set; everything else absent.
        let mut data = Map::new();
        data.insert(
            "protocol".into(),
            serde_json::json!({"major": 0, "minor": 1}),
        );
        data.insert("device_id".into(), Value::String("bare".into()));
        let Message::Hello(h) =
            Message::from_frame(Frame::new(kind::HELLO).with_data(data)).unwrap()
        else {
            panic!("expected hello");
        };
        assert_eq!(h.capabilities, Capabilities::default());
    }

    #[test]
    fn version_compatibility_follows_the_spec() {
        let v = |major, minor| Version { major, minor };
        assert!(v(0, 1).compatible_with(v(0, 1)));
        assert!(!v(0, 1).compatible_with(v(0, 2)), "0.x is exact-match");
        assert!(
            v(1, 0).compatible_with(v(1, 5)),
            "minor additions are compatible"
        );
        assert!(!v(1, 0).compatible_with(v(2, 0)), "major is breaking");
    }

    #[test]
    fn stream_urls_are_validated() {
        let frame = |url: &str, title: &str| {
            let mut d = Map::new();
            d.insert("url".into(), Value::String(url.into()));
            d.insert("title".into(), Value::String(title.into()));
            Frame::new(kind::PLAY_URL).with_data(d)
        };
        assert!(Message::from_frame(frame("https://r.example/a", "A")).is_ok());
        for bad in [
            "",
            "file:///etc/passwd",
            "ftp://x/y",
            "http://a b/c",
            "javascript:alert(1)",
        ] {
            assert!(Message::from_frame(frame(bad, "x")).is_err(), "{bad}");
        }
        let long = format!("http://x/{}", "a".repeat(MAX_URL));
        assert!(Message::from_frame(frame(&long, "x")).is_err());
        assert!(Message::from_frame(frame("http://x/", &"t".repeat(MAX_TITLE + 1))).is_err());
        assert!(
            Message::from_frame(Frame::new(kind::PLAY_URL)).is_err(),
            "missing fields"
        );
    }

    #[test]
    fn media_control_rejects_unknown_actions() {
        let mut d = Map::new();
        d.insert("action".into(), Value::String("explode".into()));
        assert!(Message::from_frame(Frame::new(kind::MEDIA_CONTROL).with_data(d)).is_err());
    }

    #[test]
    fn media_url_is_off_unless_declared() {
        assert!(!Capabilities::default().media_url);
        let h: Hello =
            serde_json::from_str(r#"{"protocol":{"major":0,"minor":1},"device_id":"d"}"#).unwrap();
        assert!(
            !h.capabilities.media_url,
            "older satellites that never heard of it stay off"
        );
    }
}
