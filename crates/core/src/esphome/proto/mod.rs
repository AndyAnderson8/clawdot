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

/// Read a varint. `Ok(None)` means the input ended before the varint did (more bytes may complete it).
///
/// # Errors
/// [`Error::VarintTooLong`] when it runs past ten bytes.
pub fn read_varint(b: &[u8]) -> Result<Option<(u64, usize)>, Error> {
    let mut v: u64 = 0;
    for (i, &byte) in b.iter().enumerate() {
        if i >= MAX_VARINT_BYTES {
            return Err(Error::VarintTooLong);
        }
        v |= u64::from(byte & 0x7f)
            .checked_shl(7 * u32::try_from(i).unwrap_or(63))
            .unwrap_or(0);
        if byte & 0x80 == 0 {
            return Ok(Some((v, i + 1)));
        }
    }
    if b.len() >= MAX_VARINT_BYTES {
        return Err(Error::VarintTooLong);
    }
    Ok(None)
}

/// Append a varint.
pub fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v & 0x7f) as u8 | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

/// One decoded field value.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Value<'a> {
    Varint(u64),
    Fixed32(u32),
    Bytes(&'a [u8]),
    /// A fixed 64-bit value; no message here uses one, so it is read and dropped.
    Other,
}

/// Iterator over the fields of one message.
struct Fields<'a> {
    b: &'a [u8],
}

impl<'a> Iterator for Fields<'a> {
    type Item = Result<(u32, Value<'a>), Error>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.b.is_empty() {
            return None;
        }
        let r = self.read();
        if r.is_err() {
            self.b = &[];
        }
        Some(r)
    }
}

impl<'a> Fields<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        if n > self.b.len() {
            return Err(Error::Truncated);
        }
        let (head, tail) = self.b.split_at(n);
        self.b = tail;
        Ok(head)
    }

    fn varint(&mut self) -> Result<u64, Error> {
        match read_varint(self.b)? {
            Some((v, n)) => {
                self.b = &self.b[n..];
                Ok(v)
            }
            None => Err(Error::Truncated),
        }
    }

    fn read(&mut self) -> Result<(u32, Value<'a>), Error> {
        let tag = self.varint()?;
        let field = u32::try_from(tag >> 3).map_err(|_| Error::FieldNumber)?;
        if field == 0 {
            return Err(Error::FieldNumber);
        }
        let value = match tag & 7 {
            0 => Value::Varint(self.varint()?),
            1 => {
                self.take(8)?;
                Value::Other
            }
            2 => {
                let len = usize::try_from(self.varint()?).map_err(|_| Error::Truncated)?;
                Value::Bytes(self.take(len)?)
            }
            5 => {
                let raw = self.take(4)?;
                Value::Fixed32(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
            }
            w => return Err(Error::WireType(w as u8)),
        };
        Ok((field, value))
    }
}

fn text(b: &[u8]) -> Result<String, Error> {
    let s = std::str::from_utf8(b).map_err(|_| Error::Utf8)?;
    Ok(s.chars().take(MAX_TEXT_CHARS).collect())
}

fn as_u32(v: Value<'_>) -> Result<u32, Error> {
    match v {
        // A wider varint is truncated to 32 bits, as protobuf does for uint32 fields.
        Value::Varint(n) => Ok(n as u32),
        _ => Err(Error::Mismatch),
    }
}

fn as_bool(v: Value<'_>) -> Result<bool, Error> {
    match v {
        Value::Varint(n) => Ok(n != 0),
        _ => Err(Error::Mismatch),
    }
}

fn as_text(v: Value<'_>) -> Result<String, Error> {
    match v {
        Value::Bytes(b) => text(b),
        _ => Err(Error::Mismatch),
    }
}

fn as_fixed32(v: Value<'_>) -> Result<u32, Error> {
    match v {
        Value::Fixed32(n) => Ok(n),
        _ => Err(Error::Mismatch),
    }
}

fn put_tag(out: &mut Vec<u8>, field: u32, wire: u8) {
    put_varint(out, (u64::from(field) << 3) | u64::from(wire));
}

fn put_uint(out: &mut Vec<u8>, field: u32, v: u32) {
    if v != 0 {
        put_tag(out, field, 0);
        put_varint(out, u64::from(v));
    }
}

fn put_bool(out: &mut Vec<u8>, field: u32, v: bool) {
    if v {
        put_tag(out, field, 0);
        out.push(1);
    }
}

fn put_bytes(out: &mut Vec<u8>, field: u32, v: &[u8]) {
    if !v.is_empty() {
        put_tag(out, field, 2);
        put_varint(out, v.len() as u64);
        out.extend_from_slice(v);
    }
}

fn put_fixed32(out: &mut Vec<u8>, field: u32, v: u32) {
    put_tag(out, field, 5);
    out.extend_from_slice(&v.to_le_bytes());
}

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

impl Msg {
    /// The message type number and its encoded payload.
    #[must_use]
    pub fn encode(&self) -> (u32, Vec<u8>) {
        let mut o = Vec::new();
        let t = match self {
            Self::HelloRequest {
                client_info,
                major,
                minor,
            } => {
                put_bytes(&mut o, 1, client_info.as_bytes());
                put_uint(&mut o, 2, *major);
                put_uint(&mut o, 3, *minor);
                ty::HELLO_REQUEST
            }
            Self::HelloResponse {
                major,
                minor,
                server_info,
                name,
            } => {
                put_uint(&mut o, 1, *major);
                put_uint(&mut o, 2, *minor);
                put_bytes(&mut o, 3, server_info.as_bytes());
                put_bytes(&mut o, 4, name.as_bytes());
                ty::HELLO_RESPONSE
            }
            Self::DisconnectRequest => ty::DISCONNECT_REQUEST,
            Self::DisconnectResponse => ty::DISCONNECT_RESPONSE,
            Self::PingRequest => ty::PING_REQUEST,
            Self::PingResponse => ty::PING_RESPONSE,
            Self::DeviceInfoRequest => ty::DEVICE_INFO_REQUEST,
            Self::DeviceInfoResponse {
                name,
                esphome_version,
                friendly_name,
                voice_flags,
            } => {
                put_bytes(&mut o, 2, name.as_bytes());
                put_bytes(&mut o, 4, esphome_version.as_bytes());
                put_bytes(&mut o, 13, friendly_name.as_bytes());
                put_uint(&mut o, 17, *voice_flags);
                ty::DEVICE_INFO_RESPONSE
            }
            Self::ListEntitiesRequest => ty::LIST_ENTITIES_REQUEST,
            Self::ListEntitiesDone => ty::LIST_ENTITIES_DONE,
            Self::ListEntitiesMediaPlayer {
                object_id,
                key,
                name,
            } => {
                put_bytes(&mut o, 1, object_id.as_bytes());
                put_fixed32(&mut o, 2, *key);
                put_bytes(&mut o, 3, name.as_bytes());
                ty::LIST_ENTITIES_MEDIA_PLAYER
            }
            Self::MediaPlayerCommand {
                key,
                command,
                volume,
                media_url,
                announcement,
            } => {
                put_fixed32(&mut o, 1, *key);
                if let Some(c) = command {
                    put_bool(&mut o, 2, true);
                    put_uint(&mut o, 3, *c);
                }
                if let Some(v) = volume {
                    put_bool(&mut o, 4, true);
                    put_fixed32(&mut o, 5, v.to_bits());
                }
                if let Some(u) = media_url {
                    put_bool(&mut o, 6, true);
                    put_bytes(&mut o, 7, u.as_bytes());
                }
                if let Some(a) = announcement {
                    put_bool(&mut o, 8, true);
                    put_bool(&mut o, 9, *a);
                }
                ty::MEDIA_PLAYER_COMMAND
            }
            Self::SubscribeVoice { subscribe, flags } => {
                put_bool(&mut o, 1, *subscribe);
                put_uint(&mut o, 2, *flags);
                ty::SUBSCRIBE_VOICE
            }
            Self::VoiceRequest {
                start,
                conversation_id,
                flags,
                wake_word_phrase,
            } => {
                put_bool(&mut o, 1, *start);
                put_bytes(&mut o, 2, conversation_id.as_bytes());
                put_uint(&mut o, 3, *flags);
                put_bytes(&mut o, 5, wake_word_phrase.as_bytes());
                ty::VOICE_REQUEST
            }
            Self::VoiceResponse { port, error } => {
                put_uint(&mut o, 1, *port);
                put_bool(&mut o, 2, *error);
                ty::VOICE_RESPONSE
            }
            Self::VoiceEvent { kind, data } => {
                put_uint(&mut o, 1, *kind);
                for (name, value) in data {
                    let mut pair = Vec::new();
                    put_bytes(&mut pair, 1, name.as_bytes());
                    put_bytes(&mut pair, 2, value.as_bytes());
                    put_bytes(&mut o, 2, &pair);
                }
                ty::VOICE_EVENT
            }
            Self::VoiceAudio { data, end } => {
                put_bytes(&mut o, 1, data);
                put_bool(&mut o, 2, *end);
                ty::VOICE_AUDIO
            }
            Self::AnnounceRequest {
                media_id,
                text,
                start_conversation,
            } => {
                put_bytes(&mut o, 1, media_id.as_bytes());
                put_bytes(&mut o, 2, text.as_bytes());
                put_bool(&mut o, 4, *start_conversation);
                ty::ANNOUNCE_REQUEST
            }
            Self::AnnounceFinished { success } => {
                put_bool(&mut o, 1, *success);
                ty::ANNOUNCE_FINISHED
            }
            Self::Unknown(t) => *t,
        };
        (t, o)
    }

    /// Decode one message of type `t`. Types this client does not use give [`Msg::Unknown`].
    ///
    /// # Errors
    /// [`Error`] for malformed payloads of a known type.
    pub fn decode(t: u32, payload: &[u8]) -> Result<Self, Error> {
        let fields = Fields { b: payload };
        Ok(match t {
            ty::HELLO_REQUEST => {
                let (mut client_info, mut major, mut minor) = (String::new(), 0, 0);
                for f in fields {
                    match f? {
                        (1, v) => client_info = as_text(v)?,
                        (2, v) => major = as_u32(v)?,
                        (3, v) => minor = as_u32(v)?,
                        _ => {}
                    }
                }
                Self::HelloRequest {
                    client_info,
                    major,
                    minor,
                }
            }
            ty::HELLO_RESPONSE => {
                let (mut major, mut minor) = (0, 0);
                let (mut server_info, mut name) = (String::new(), String::new());
                for f in fields {
                    match f? {
                        (1, v) => major = as_u32(v)?,
                        (2, v) => minor = as_u32(v)?,
                        (3, v) => server_info = as_text(v)?,
                        (4, v) => name = as_text(v)?,
                        _ => {}
                    }
                }
                Self::HelloResponse {
                    major,
                    minor,
                    server_info,
                    name,
                }
            }
            ty::DISCONNECT_REQUEST => Self::drain(fields, Self::DisconnectRequest)?,
            ty::DISCONNECT_RESPONSE => Self::drain(fields, Self::DisconnectResponse)?,
            ty::PING_REQUEST => Self::drain(fields, Self::PingRequest)?,
            ty::PING_RESPONSE => Self::drain(fields, Self::PingResponse)?,
            ty::DEVICE_INFO_REQUEST => Self::drain(fields, Self::DeviceInfoRequest)?,
            ty::LIST_ENTITIES_REQUEST => Self::drain(fields, Self::ListEntitiesRequest)?,
            ty::LIST_ENTITIES_DONE => Self::drain(fields, Self::ListEntitiesDone)?,
            ty::DEVICE_INFO_RESPONSE => {
                let (mut name, mut esphome_version, mut friendly_name) =
                    (String::new(), String::new(), String::new());
                let mut voice_flags = 0;
                for f in fields {
                    match f? {
                        (2, v) => name = as_text(v)?,
                        (4, v) => esphome_version = as_text(v)?,
                        (13, v) => friendly_name = as_text(v)?,
                        (17, v) => voice_flags = as_u32(v)?,
                        _ => {}
                    }
                }
                Self::DeviceInfoResponse {
                    name,
                    esphome_version,
                    friendly_name,
                    voice_flags,
                }
            }
            ty::LIST_ENTITIES_MEDIA_PLAYER => {
                let (mut object_id, mut name, mut key) = (String::new(), String::new(), 0);
                for f in fields {
                    match f? {
                        (1, v) => object_id = as_text(v)?,
                        (2, v) => key = as_fixed32(v)?,
                        (3, v) => name = as_text(v)?,
                        _ => {}
                    }
                }
                Self::ListEntitiesMediaPlayer {
                    object_id,
                    key,
                    name,
                }
            }
            ty::MEDIA_PLAYER_COMMAND => {
                let mut key = 0;
                let (mut has_cmd, mut cmd, mut has_vol, mut vol) = (false, 0, false, 0.0);
                let (mut has_url, mut url, mut has_ann, mut ann) =
                    (false, String::new(), false, false);
                for f in fields {
                    match f? {
                        (1, v) => key = as_fixed32(v)?,
                        (2, v) => has_cmd = as_bool(v)?,
                        (3, v) => cmd = as_u32(v)?,
                        (4, v) => has_vol = as_bool(v)?,
                        (5, v) => vol = f32::from_bits(as_fixed32(v)?),
                        (6, v) => has_url = as_bool(v)?,
                        (7, v) => url = as_text(v)?,
                        (8, v) => has_ann = as_bool(v)?,
                        (9, v) => ann = as_bool(v)?,
                        _ => {}
                    }
                }
                Self::MediaPlayerCommand {
                    key,
                    command: has_cmd.then_some(cmd),
                    volume: has_vol.then_some(vol),
                    media_url: has_url.then_some(url),
                    announcement: has_ann.then_some(ann),
                }
            }
            ty::SUBSCRIBE_VOICE => {
                let (mut subscribe, mut flags) = (false, 0);
                for f in fields {
                    match f? {
                        (1, v) => subscribe = as_bool(v)?,
                        (2, v) => flags = as_u32(v)?,
                        _ => {}
                    }
                }
                Self::SubscribeVoice { subscribe, flags }
            }
            ty::VOICE_REQUEST => {
                let (mut start, mut flags) = (false, 0);
                let (mut conversation_id, mut wake_word_phrase) = (String::new(), String::new());
                for f in fields {
                    match f? {
                        (1, v) => start = as_bool(v)?,
                        (2, v) => conversation_id = as_text(v)?,
                        (3, v) => flags = as_u32(v)?,
                        (5, v) => wake_word_phrase = as_text(v)?,
                        _ => {}
                    }
                }
                Self::VoiceRequest {
                    start,
                    conversation_id,
                    flags,
                    wake_word_phrase,
                }
            }
            ty::VOICE_RESPONSE => {
                let (mut port, mut error) = (0, false);
                for f in fields {
                    match f? {
                        (1, v) => port = as_u32(v)?,
                        (2, v) => error = as_bool(v)?,
                        _ => {}
                    }
                }
                Self::VoiceResponse { port, error }
            }
            ty::VOICE_EVENT => {
                let mut kind = 0;
                let mut data = Vec::new();
                for f in fields {
                    match f? {
                        (1, v) => kind = as_u32(v)?,
                        (2, Value::Bytes(b)) if data.len() < MAX_EVENT_PAIRS => {
                            let (mut n, mut val) = (String::new(), String::new());
                            for p in (Fields { b }) {
                                match p? {
                                    (1, v) => n = as_text(v)?,
                                    (2, v) => val = as_text(v)?,
                                    _ => {}
                                }
                            }
                            data.push((n, val));
                        }
                        _ => {}
                    }
                }
                Self::VoiceEvent { kind, data }
            }
            ty::VOICE_AUDIO => {
                let mut data = Vec::new();
                let mut end = false;
                for f in fields {
                    match f? {
                        // `data2` (field 3) is assumed to continue `data` (open question in docs).
                        (1 | 3, Value::Bytes(b)) => data.extend_from_slice(b),
                        (2, v) => end = as_bool(v)?,
                        _ => {}
                    }
                }
                Self::VoiceAudio { data, end }
            }
            ty::ANNOUNCE_REQUEST => {
                let (mut media_id, mut text_, mut start_conversation) =
                    (String::new(), String::new(), false);
                for f in fields {
                    match f? {
                        (1, v) => media_id = as_text_full(v)?,
                        (2, v) => text_ = as_text(v)?,
                        (4, v) => start_conversation = as_bool(v)?,
                        _ => {}
                    }
                }
                Self::AnnounceRequest {
                    media_id,
                    text: text_,
                    start_conversation,
                }
            }
            ty::ANNOUNCE_FINISHED => {
                let mut success = false;
                for f in fields {
                    if let (1, v) = f? {
                        success = as_bool(v)?;
                    }
                }
                Self::AnnounceFinished { success }
            }
            other => {
                // Still walk the fields, so a malformed message of an unused type is reported the same way.
                for f in fields {
                    f?;
                }
                Self::Unknown(other)
            }
        })
    }

    /// Walk every field of an empty message (checking it is well formed) and return `empty`.
    fn drain(fields: Fields<'_>, empty: Self) -> Result<Self, Error> {
        for f in fields {
            f?;
        }
        Ok(empty)
    }
}

/// Text that must not be clipped (URLs): same checks, no character limit beyond the frame.
fn as_text_full(v: Value<'_>) -> Result<String, Error> {
    match v {
        Value::Bytes(b) => std::str::from_utf8(b)
            .map(str::to_owned)
            .map_err(|_| Error::Utf8),
        _ => Err(Error::Mismatch),
    }
}

#[cfg(test)]
mod tests {
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
