//! A minimal protobuf codec for the ESPHome native-API messages the voice-satellite client needs
//! (spec 0004, REQ-ESP-004).
//!
//! No `prost`, no generated code: about fifteen messages, written by hand from the documented wire behaviour.
//! Decoding is strict. Every length is checked against the bytes that remain, varints are bounded, groups and
//! unknown wire types are errors, unknown fields are skipped, text must be UTF-8, and repeated fields are
//! capped, so no input can panic or make the decoder allocate more than the frame it came in.

use std::fmt;

/// Longest varint (a 64-bit value).
const MAX_VARINT_BYTES: usize = 10;
/// Event data pairs kept from one event; more are ignored.
const MAX_EVENT_PAIRS: usize = 16;
/// Text fields are clipped to this many characters, because they end up in logs.
const MAX_TEXT_CHARS: usize = 256;

/// Message type numbers (the ESPHome `id` option of each message).
pub mod ty {
    /// `HelloRequest`.
    pub const HELLO_REQUEST: u32 = 1;
    /// `HelloResponse`.
    pub const HELLO_RESPONSE: u32 = 2;
    /// `DisconnectRequest`.
    pub const DISCONNECT_REQUEST: u32 = 5;
    /// `DisconnectResponse`.
    pub const DISCONNECT_RESPONSE: u32 = 6;
    /// `PingRequest`.
    pub const PING_REQUEST: u32 = 7;
    /// `PingResponse`.
    pub const PING_RESPONSE: u32 = 8;
    /// `DeviceInfoRequest`.
    pub const DEVICE_INFO_REQUEST: u32 = 9;
    /// `DeviceInfoResponse`.
    pub const DEVICE_INFO_RESPONSE: u32 = 10;
    /// `ListEntitiesRequest`.
    pub const LIST_ENTITIES_REQUEST: u32 = 11;
    /// `ListEntitiesDoneResponse`.
    pub const LIST_ENTITIES_DONE: u32 = 19;
    /// `ListEntitiesMediaPlayerResponse`.
    pub const LIST_ENTITIES_MEDIA_PLAYER: u32 = 63;
    /// `MediaPlayerCommandRequest`.
    pub const MEDIA_PLAYER_COMMAND: u32 = 65;
    /// `SubscribeVoiceAssistantRequest`.
    pub const SUBSCRIBE_VOICE: u32 = 89;
    /// `VoiceAssistantRequest`.
    pub const VOICE_REQUEST: u32 = 90;
    /// `VoiceAssistantResponse`.
    pub const VOICE_RESPONSE: u32 = 91;
    /// `VoiceAssistantEventResponse`.
    pub const VOICE_EVENT: u32 = 92;
    /// `VoiceAssistantAudio`.
    pub const VOICE_AUDIO: u32 = 106;
    /// `VoiceAssistantAnnounceRequest`.
    pub const ANNOUNCE_REQUEST: u32 = 119;
    /// `VoiceAssistantAnnounceFinished`.
    pub const ANNOUNCE_FINISHED: u32 = 120;
}

/// Bits of `voice_assistant_feature_flags` in `DeviceInfoResponse`.
pub mod flags {
    /// The device runs a voice assistant.
    pub const VOICE_ASSISTANT: u32 = 1;
    /// The device has a speaker.
    pub const SPEAKER: u32 = 2;
    /// The device can carry audio inside the API connection.
    pub const API_AUDIO: u32 = 4;
    /// The device reports timers.
    pub const TIMERS: u32 = 8;
    /// The device can play announcements.
    pub const ANNOUNCE: u32 = 16;
    /// The device can start a conversation after an announcement.
    pub const START_CONVERSATION: u32 = 32;
}

/// `VoiceAssistantEvent` numbers.
pub mod event {
    /// Something went wrong.
    pub const ERROR: u32 = 0;
    /// A run begins.
    pub const RUN_START: u32 = 1;
    /// A run ends.
    pub const RUN_END: u32 = 2;
    /// Speech-to-text begins.
    pub const STT_START: u32 = 3;
    /// Speech-to-text finished.
    pub const STT_END: u32 = 4;
    /// Intent handling begins.
    pub const INTENT_START: u32 = 5;
    /// Intent handling finished.
    pub const INTENT_END: u32 = 6;
    /// Text-to-speech begins.
    pub const TTS_START: u32 = 7;
    /// Text-to-speech finished; `url` data names the audio.
    pub const TTS_END: u32 = 8;
    /// Speech was heard to end.
    pub const STT_VAD_END: u32 = 12;
    /// The reply audio starts streaming.
    pub const TTS_STREAM_START: u32 = 98;
    /// The reply audio finished streaming.
    pub const TTS_STREAM_END: u32 = 99;
}

/// `MediaPlayerCommand` numbers.
pub mod media {
    /// Play (resume).
    pub const PLAY: u32 = 0;
    /// Pause.
    pub const PAUSE: u32 = 1;
    /// Stop.
    pub const STOP: u32 = 2;
}

/// Why a message could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// The input ended in the middle of a value.
    Truncated,
    /// A varint was longer than allowed.
    VarintTooLong,
    /// A wire type this codec does not accept (groups or reserved values).
    WireType(u8),
    /// A field number of zero or one that does not fit.
    FieldNumber,
    /// A text field was not valid UTF-8.
    Utf8,
    /// A value of the wrong wire type for its field.
    Mismatch,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Truncated => "message ends early",
            Self::VarintTooLong => "varint is too long",
            Self::WireType(_) => "unsupported wire type",
            Self::FieldNumber => "bad field number",
            Self::Utf8 => "text is not valid UTF-8",
            Self::Mismatch => "field has the wrong wire type",
        })
    }
}

impl std::error::Error for Error {}

mod decode;
mod encode;
mod wire;

pub use wire::{put_varint, read_varint};

/// Every message the client sends or understands.
#[derive(Debug, Clone, PartialEq)]
pub enum Msg {
    /// Client greeting.
    HelloRequest {
        /// Who we are, for the device's logs.
        client_info: String,
        /// API major version.
        major: u32,
        /// API minor version.
        minor: u32,
    },
    /// Device greeting.
    HelloResponse {
        /// API major version.
        major: u32,
        /// API minor version.
        minor: u32,
        /// Free text about the server.
        server_info: String,
        /// Device name.
        name: String,
    },
    /// Close the connection.
    DisconnectRequest,
    /// Acknowledge a close.
    DisconnectResponse,
    /// Keepalive.
    PingRequest,
    /// Keepalive reply.
    PingResponse,
    /// Ask for device information.
    DeviceInfoRequest,
    /// Device information.
    DeviceInfoResponse {
        /// Node name.
        name: String,
        /// Firmware version string.
        esphome_version: String,
        /// Human name.
        friendly_name: String,
        /// Voice-assistant feature bits ([`flags`]).
        voice_flags: u32,
    },
    /// Ask for the entity list.
    ListEntitiesRequest,
    /// End of the entity list.
    ListEntitiesDone,
    /// A media-player entity.
    ListEntitiesMediaPlayer {
        /// Object id.
        object_id: String,
        /// Key used to address commands.
        key: u32,
        /// Display name.
        name: String,
    },
    /// Control a media player.
    MediaPlayerCommand {
        /// Which entity.
        key: u32,
        /// A [`media`] command.
        command: Option<u32>,
        /// Volume, 0.0 to 1.0.
        volume: Option<f32>,
        /// Media to play.
        media_url: Option<String>,
        /// Play as an announcement.
        announcement: Option<bool>,
    },
    /// Subscribe to or leave voice-assistant requests.
    SubscribeVoice {
        /// Subscribe or unsubscribe.
        subscribe: bool,
        /// Subscribe flags (1 = API audio).
        flags: u32,
    },
    /// The device wants (or stops wanting) a voice run.
    VoiceRequest {
        /// Start or abort.
        start: bool,
        /// Conversation to continue, if any.
        conversation_id: String,
        /// Request flags.
        flags: u32,
        /// The phrase that woke the device.
        wake_word_phrase: String,
    },
    /// Answer to a [`Msg::VoiceRequest`].
    VoiceResponse {
        /// UDP port for audio; 0 means audio travels over the API.
        port: u32,
        /// The request was refused.
        error: bool,
    },
    /// A pipeline event for the device to show.
    VoiceEvent {
        /// A [`event`] number.
        kind: u32,
        /// Name and value pairs.
        data: Vec<(String, String)>,
    },
    /// Audio in either direction.
    VoiceAudio {
        /// 16 kHz mono 16-bit little-endian PCM.
        data: Vec<u8>,
        /// Last piece of the stream.
        end: bool,
    },
    /// Play something on the device.
    AnnounceRequest {
        /// URL of the audio.
        media_id: String,
        /// Text to show or speak.
        text: String,
        /// Listen for an answer afterwards.
        start_conversation: bool,
    },
    /// An announcement finished playing.
    AnnounceFinished {
        /// It played.
        success: bool,
    },
    /// A message type this client does not use.
    Unknown(u32),
}

#[cfg(test)]
mod tests {
    use super::wire::{put_bytes, put_uint};
    use super::*;

    fn round(m: &Msg) {
        let (t, p) = m.encode();
        assert_eq!(&Msg::decode(t, &p).unwrap(), m, "type {t}");
    }

    // REQ-ESP-004
    #[test]
    fn req_esp_004_every_message_round_trips() {
        for m in [
            Msg::HelloRequest {
                client_info: "ClawDot".into(),
                major: 1,
                minor: 10,
            },
            Msg::HelloResponse {
                major: 1,
                minor: 12,
                server_info: "2026.1".into(),
                name: "dot".into(),
            },
            Msg::DisconnectRequest,
            Msg::DisconnectResponse,
            Msg::PingRequest,
            Msg::PingResponse,
            Msg::DeviceInfoRequest,
            Msg::DeviceInfoResponse {
                name: "dot".into(),
                esphome_version: "2026.1".into(),
                friendly_name: "Kitchen Dot".into(),
                voice_flags: 63,
            },
            Msg::ListEntitiesRequest,
            Msg::ListEntitiesDone,
            Msg::ListEntitiesMediaPlayer {
                object_id: "speaker".into(),
                key: 0xdead_beef,
                name: "Speaker".into(),
            },
            Msg::MediaPlayerCommand {
                key: 7,
                command: Some(media::STOP),
                volume: Some(0.5),
                media_url: Some("http://x/a.wav".into()),
                announcement: Some(true),
            },
            Msg::MediaPlayerCommand {
                key: 7,
                command: None,
                volume: None,
                media_url: None,
                announcement: None,
            },
            Msg::SubscribeVoice {
                subscribe: true,
                flags: 1,
            },
            Msg::VoiceRequest {
                start: true,
                conversation_id: "c1".into(),
                flags: 3,
                wake_word_phrase: "okay nabu".into(),
            },
            Msg::VoiceResponse {
                port: 0,
                error: false,
            },
            Msg::VoiceEvent {
                kind: event::TTS_END,
                data: vec![("url".into(), "http://x/a.wav".into())],
            },
            Msg::VoiceAudio {
                data: vec![1, 2, 3, 4],
                end: true,
            },
            Msg::AnnounceRequest {
                media_id: "http://x/a.wav".into(),
                text: String::new(),
                start_conversation: true,
            },
            Msg::AnnounceFinished { success: true },
        ] {
            round(&m);
        }
    }

    #[test]
    fn unknown_types_and_fields_are_skipped() {
        assert_eq!(Msg::decode(9999, &[8, 1]).unwrap(), Msg::Unknown(9999));
        // An unknown field 15 (varint) inside a Hello response is ignored.
        let mut p = Msg::HelloResponse {
            major: 1,
            minor: 2,
            server_info: String::new(),
            name: "n".into(),
        }
        .encode()
        .1;
        p.extend_from_slice(&[0x78, 0x05]);
        assert!(matches!(
            Msg::decode(ty::HELLO_RESPONSE, &p).unwrap(),
            Msg::HelloResponse { major: 1, .. }
        ));
    }

    // REQ-ESP-004
    #[test]
    fn req_esp_004_malformed_input_is_an_error_not_a_panic() {
        let bad: &[&[u8]] = &[
            &[0x0a],                   // length missing
            &[0x0a, 0x05, b'a'],       // length past the end
            &[0x08],                   // varint value missing
            &[0x08, 0x80],             // varint never ends
            &[0x0b, 0x00],             // group start (wire type 3)
            &[0x0f],                   // wire type 7
            &[0x00, 0x01],             // field number zero
            &[0x0d, 0x01, 0x02],       // fixed32 truncated
            &[0x0a, 0x02, 0xff, 0xfe], // invalid UTF-8
            &[
                0x08, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01,
            ], // 11-byte varint
            &[
                0x0a, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01,
            ], // huge length
        ];
        for t in [
            ty::HELLO_RESPONSE,
            ty::DEVICE_INFO_RESPONSE,
            ty::LIST_ENTITIES_MEDIA_PLAYER,
            ty::VOICE_REQUEST,
            ty::VOICE_AUDIO,
            ty::VOICE_EVENT,
            ty::ANNOUNCE_FINISHED,
            ty::PING_REQUEST,
            12345,
        ] {
            for b in bad {
                // Some of these happen to be valid for some types (a wrong field type is only an error
                // when the field is used); the point is that nothing panics.
                let _ = Msg::decode(t, b);
            }
        }
        assert_eq!(Msg::decode(ty::VOICE_AUDIO, &[0x0a]), Err(Error::Truncated));
        assert_eq!(
            Msg::decode(ty::PING_REQUEST, &[0x0b]),
            Err(Error::WireType(3))
        );
        assert_eq!(
            Msg::decode(ty::HELLO_RESPONSE, &[0x22, 2, 0xff, 0xfe]),
            Err(Error::Utf8)
        );
    }

    // REQ-ESP-004: a fuzz-style sweep with a fixed generator, so it is repeatable.
    #[test]
    fn req_esp_004_random_payloads_never_panic() {
        let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        let types = [
            1, 2, 5, 7, 10, 19, 63, 65, 89, 90, 91, 92, 106, 119, 120, 1000,
        ];
        for _ in 0..20_000 {
            let len = (next() % 48) as usize;
            let buf: Vec<u8> = (0..len).map(|_| next() as u8).collect();
            let t = types[(next() % types.len() as u64) as usize];
            let _ = Msg::decode(t, &buf);
        }
    }

    #[test]
    fn varints_round_trip() {
        for v in [0u64, 1, 127, 128, 300, u32::MAX.into(), u64::MAX] {
            let mut b = Vec::new();
            put_varint(&mut b, v);
            assert_eq!(read_varint(&b).unwrap(), Some((v, b.len())));
        }
        assert_eq!(read_varint(&[0x80]).unwrap(), None);
        assert_eq!(read_varint(&[0x80; 10]), Err(Error::VarintTooLong));
    }

    #[test]
    fn event_pairs_and_text_are_capped() {
        let mut o = Vec::new();
        put_uint(&mut o, 1, 8);
        for i in 0..40 {
            let mut pair = Vec::new();
            put_bytes(&mut pair, 1, format!("k{i}").as_bytes());
            put_bytes(&mut pair, 2, "x".repeat(1000).as_bytes());
            put_bytes(&mut o, 2, &pair);
        }
        let Msg::VoiceEvent { data, .. } = Msg::decode(ty::VOICE_EVENT, &o).unwrap() else {
            panic!("wrong message");
        };
        assert_eq!(data.len(), MAX_EVENT_PAIRS);
        assert_eq!(data[0].1.chars().count(), MAX_TEXT_CHARS);
    }
}
