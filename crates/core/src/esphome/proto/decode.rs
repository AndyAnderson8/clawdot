//! Message decoding.

use super::wire::{Fields, Value, as_bool, as_fixed32, as_text, as_text_full, as_u32};
use super::{Error, MAX_EVENT_PAIRS, Msg, ty};

impl Msg {
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
