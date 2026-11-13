//! Message decoding: one small function per message type that has fields, and `Msg::decode` as the dispatch.

use super::wire::{Fields, Value, as_bool, as_fixed32, as_text, as_text_full, as_u32};
use super::{Error, MAX_EVENT_PAIRS, Msg, ty};

impl Msg {
    /// Decode one message of type `t`. Types this client does not use give [`Msg::Unknown`].
    ///
    /// # Errors
    /// [`Error`] for malformed payloads of a known type.
    pub fn decode(t: u32, payload: &[u8]) -> Result<Self, Error> {
        let fields = Fields { b: payload };
        match t {
            ty::HELLO_REQUEST => hello_request(fields),
            ty::HELLO_RESPONSE => hello_response(fields),
            ty::DISCONNECT_REQUEST => drain(fields, Self::DisconnectRequest),
            ty::DISCONNECT_RESPONSE => drain(fields, Self::DisconnectResponse),
            ty::PING_REQUEST => drain(fields, Self::PingRequest),
            ty::PING_RESPONSE => drain(fields, Self::PingResponse),
            ty::DEVICE_INFO_REQUEST => drain(fields, Self::DeviceInfoRequest),
            ty::LIST_ENTITIES_REQUEST => drain(fields, Self::ListEntitiesRequest),
            ty::LIST_ENTITIES_DONE => drain(fields, Self::ListEntitiesDone),
            ty::DEVICE_INFO_RESPONSE => device_info_response(fields),
            ty::LIST_ENTITIES_MEDIA_PLAYER => list_entities_media_player(fields),
            ty::MEDIA_PLAYER_COMMAND => media_player_command(fields),
            ty::SUBSCRIBE_VOICE => subscribe_voice(fields),
            ty::VOICE_REQUEST => voice_request(fields),
            ty::VOICE_RESPONSE => voice_response(fields),
            ty::VOICE_EVENT => voice_event(fields),
            ty::VOICE_AUDIO => voice_audio(fields),
            ty::ANNOUNCE_REQUEST => announce_request(fields),
            ty::ANNOUNCE_FINISHED => announce_finished(fields),
            // Still walk the fields, so a malformed message of an unused type is reported the same way.
            other => drain(fields, Self::Unknown(other)),
        }
    }
}

/// Walk every field of a message with nothing to keep (checking it is well formed) and return `msg`.
fn drain(fields: Fields<'_>, msg: Msg) -> Result<Msg, Error> {
    for f in fields {
        f?;
    }
    Ok(msg)
}

fn hello_request(fields: Fields<'_>) -> Result<Msg, Error> {
    let (mut client_info, mut major, mut minor) = (String::new(), 0, 0);
    for f in fields {
        match f? {
            (1, v) => client_info = as_text(v)?,
            (2, v) => major = as_u32(v)?,
            (3, v) => minor = as_u32(v)?,
            _ => {}
        }
    }
    Ok(Msg::HelloRequest {
        client_info,
        major,
        minor,
    })
}

fn hello_response(fields: Fields<'_>) -> Result<Msg, Error> {
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
    Ok(Msg::HelloResponse {
        major,
        minor,
        server_info,
        name,
    })
}

fn device_info_response(fields: Fields<'_>) -> Result<Msg, Error> {
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
    Ok(Msg::DeviceInfoResponse {
        name,
        esphome_version,
        friendly_name,
        voice_flags,
    })
}

fn list_entities_media_player(fields: Fields<'_>) -> Result<Msg, Error> {
    let (mut object_id, mut name, mut key) = (String::new(), String::new(), 0);
    for f in fields {
        match f? {
            (1, v) => object_id = as_text(v)?,
            (2, v) => key = as_fixed32(v)?,
            (3, v) => name = as_text(v)?,
            _ => {}
        }
    }
    Ok(Msg::ListEntitiesMediaPlayer {
        object_id,
        key,
        name,
    })
}

fn media_player_command(fields: Fields<'_>) -> Result<Msg, Error> {
    let mut key = 0;
    let (mut has_cmd, mut cmd, mut has_vol, mut vol) = (false, 0, false, 0.0);
    let (mut has_url, mut url, mut has_ann, mut ann) = (false, String::new(), false, false);
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
    Ok(Msg::MediaPlayerCommand {
        key,
        command: has_cmd.then_some(cmd),
        volume: has_vol.then_some(vol),
        media_url: has_url.then_some(url),
        announcement: has_ann.then_some(ann),
    })
}

fn subscribe_voice(fields: Fields<'_>) -> Result<Msg, Error> {
    let (mut subscribe, mut flags) = (false, 0);
    for f in fields {
        match f? {
            (1, v) => subscribe = as_bool(v)?,
            (2, v) => flags = as_u32(v)?,
            _ => {}
        }
    }
    Ok(Msg::SubscribeVoice { subscribe, flags })
}

fn voice_request(fields: Fields<'_>) -> Result<Msg, Error> {
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
    Ok(Msg::VoiceRequest {
        start,
        conversation_id,
        flags,
        wake_word_phrase,
    })
}

fn voice_response(fields: Fields<'_>) -> Result<Msg, Error> {
    let (mut port, mut error) = (0, false);
    for f in fields {
        match f? {
            (1, v) => port = as_u32(v)?,
            (2, v) => error = as_bool(v)?,
            _ => {}
        }
    }
    Ok(Msg::VoiceResponse { port, error })
}

/// One name and value pair inside a voice event.
fn event_pair(b: &[u8]) -> Result<(String, String), Error> {
    let (mut n, mut val) = (String::new(), String::new());
    for p in (Fields { b }) {
        match p? {
            (1, v) => n = as_text(v)?,
            (2, v) => val = as_text(v)?,
            _ => {}
        }
    }
    Ok((n, val))
}

fn voice_event(fields: Fields<'_>) -> Result<Msg, Error> {
    let mut kind = 0;
    let mut data = Vec::new();
    for f in fields {
        match f? {
            (1, v) => kind = as_u32(v)?,
            (2, Value::Bytes(b)) if data.len() < MAX_EVENT_PAIRS => data.push(event_pair(b)?),
            _ => {}
        }
    }
    Ok(Msg::VoiceEvent { kind, data })
}

fn voice_audio(fields: Fields<'_>) -> Result<Msg, Error> {
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
    Ok(Msg::VoiceAudio { data, end })
}

fn announce_request(fields: Fields<'_>) -> Result<Msg, Error> {
    let (mut media_id, mut text, mut start_conversation) = (String::new(), String::new(), false);
    for f in fields {
        match f? {
            (1, v) => media_id = as_text_full(v)?,
            (2, v) => text = as_text(v)?,
            (4, v) => start_conversation = as_bool(v)?,
            _ => {}
        }
    }
    Ok(Msg::AnnounceRequest {
        media_id,
        text,
        start_conversation,
    })
}

fn announce_finished(fields: Fields<'_>) -> Result<Msg, Error> {
    let mut success = false;
    for f in fields {
        if let (1, v) = f? {
            success = as_bool(v)?;
        }
    }
    Ok(Msg::AnnounceFinished { success })
}
