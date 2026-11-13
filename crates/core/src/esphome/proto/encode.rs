//! Message encoding: one small function per message type that has fields, and `Msg::encode` as the dispatch.
//! The type number and the payload are matched apart, each over every variant, so a new message cannot be
//! added to one and forgotten in the other.

use super::wire::{put_bool, put_bytes, put_fixed32, put_uint};
use super::{Msg, ty};

impl Msg {
    /// The message type number and its encoded payload.
    #[must_use]
    pub fn encode(&self) -> (u32, Vec<u8>) {
        let mut o = Vec::new();
        self.put_payload(&mut o);
        (self.type_number(), o)
    }

    /// The message type number (an unknown message keeps the number it came with).
    fn type_number(&self) -> u32 {
        match self {
            Self::HelloRequest { .. } => ty::HELLO_REQUEST,
            Self::HelloResponse { .. } => ty::HELLO_RESPONSE,
            Self::DisconnectRequest => ty::DISCONNECT_REQUEST,
            Self::DisconnectResponse => ty::DISCONNECT_RESPONSE,
            Self::PingRequest => ty::PING_REQUEST,
            Self::PingResponse => ty::PING_RESPONSE,
            Self::DeviceInfoRequest => ty::DEVICE_INFO_REQUEST,
            Self::DeviceInfoResponse { .. } => ty::DEVICE_INFO_RESPONSE,
            Self::ListEntitiesRequest => ty::LIST_ENTITIES_REQUEST,
            Self::ListEntitiesDone => ty::LIST_ENTITIES_DONE,
            Self::ListEntitiesMediaPlayer { .. } => ty::LIST_ENTITIES_MEDIA_PLAYER,
            Self::MediaPlayerCommand { .. } => ty::MEDIA_PLAYER_COMMAND,
            Self::SubscribeVoice { .. } => ty::SUBSCRIBE_VOICE,
            Self::VoiceRequest { .. } => ty::VOICE_REQUEST,
            Self::VoiceResponse { .. } => ty::VOICE_RESPONSE,
            Self::VoiceEvent { .. } => ty::VOICE_EVENT,
            Self::VoiceAudio { .. } => ty::VOICE_AUDIO,
            Self::AnnounceRequest { .. } => ty::ANNOUNCE_REQUEST,
            Self::AnnounceFinished { .. } => ty::ANNOUNCE_FINISHED,
            Self::Unknown(t) => *t,
        }
    }

    /// Append the message's fields to `o`. A message with no fields appends nothing.
    fn put_payload(&self, o: &mut Vec<u8>) {
        match self {
            Self::HelloRequest {
                client_info,
                major,
                minor,
            } => hello_request(o, client_info, *major, *minor),
            Self::HelloResponse {
                major,
                minor,
                server_info,
                name,
            } => hello_response(o, *major, *minor, server_info, name),
            Self::DeviceInfoResponse {
                name,
                esphome_version,
                friendly_name,
                voice_flags,
            } => device_info_response(o, name, esphome_version, friendly_name, *voice_flags),
            Self::ListEntitiesMediaPlayer {
                object_id,
                key,
                name,
            } => list_entities_media_player(o, object_id, *key, name),
            Self::MediaPlayerCommand {
                key,
                command,
                volume,
                media_url,
                announcement,
            } => media_player_command(
                o,
                *key,
                *command,
                *volume,
                media_url.as_deref(),
                *announcement,
            ),
            Self::SubscribeVoice { subscribe, flags } => subscribe_voice(o, *subscribe, *flags),
            Self::VoiceRequest {
                start,
                conversation_id,
                flags,
                wake_word_phrase,
            } => voice_request(o, *start, conversation_id, *flags, wake_word_phrase),
            Self::VoiceResponse { port, error } => voice_response(o, *port, *error),
            Self::VoiceEvent { kind, data } => voice_event(o, *kind, data),
            Self::VoiceAudio { data, end } => voice_audio(o, data, *end),
            Self::AnnounceRequest {
                media_id,
                text,
                start_conversation,
            } => announce_request(o, media_id, text, *start_conversation),
            Self::AnnounceFinished { success } => announce_finished(o, *success),
            Self::DisconnectRequest
            | Self::DisconnectResponse
            | Self::PingRequest
            | Self::PingResponse
            | Self::DeviceInfoRequest
            | Self::ListEntitiesRequest
            | Self::ListEntitiesDone
            | Self::Unknown(_) => {}
        }
    }
}

fn hello_request(o: &mut Vec<u8>, client_info: &str, major: u32, minor: u32) {
    put_bytes(o, 1, client_info.as_bytes());
    put_uint(o, 2, major);
    put_uint(o, 3, minor);
}

fn hello_response(o: &mut Vec<u8>, major: u32, minor: u32, server_info: &str, name: &str) {
    put_uint(o, 1, major);
    put_uint(o, 2, minor);
    put_bytes(o, 3, server_info.as_bytes());
    put_bytes(o, 4, name.as_bytes());
}

fn device_info_response(
    o: &mut Vec<u8>,
    name: &str,
    esphome_version: &str,
    friendly_name: &str,
    voice_flags: u32,
) {
    put_bytes(o, 2, name.as_bytes());
    put_bytes(o, 4, esphome_version.as_bytes());
    put_bytes(o, 13, friendly_name.as_bytes());
    put_uint(o, 17, voice_flags);
}

fn list_entities_media_player(o: &mut Vec<u8>, object_id: &str, key: u32, name: &str) {
    put_bytes(o, 1, object_id.as_bytes());
    put_fixed32(o, 2, key);
    put_bytes(o, 3, name.as_bytes());
}

fn media_player_command(
    o: &mut Vec<u8>,
    key: u32,
    command: Option<u32>,
    volume: Option<f32>,
    media_url: Option<&str>,
    announcement: Option<bool>,
) {
    put_fixed32(o, 1, key);
    if let Some(c) = command {
        put_bool(o, 2, true);
        put_uint(o, 3, c);
    }
    if let Some(v) = volume {
        put_bool(o, 4, true);
        put_fixed32(o, 5, v.to_bits());
    }
    if let Some(u) = media_url {
        put_bool(o, 6, true);
        put_bytes(o, 7, u.as_bytes());
    }
    if let Some(a) = announcement {
        put_bool(o, 8, true);
        put_bool(o, 9, a);
    }
}

fn subscribe_voice(o: &mut Vec<u8>, subscribe: bool, flags: u32) {
    put_bool(o, 1, subscribe);
    put_uint(o, 2, flags);
}

fn voice_request(
    o: &mut Vec<u8>,
    start: bool,
    conversation_id: &str,
    flags: u32,
    wake_word_phrase: &str,
) {
    put_bool(o, 1, start);
    put_bytes(o, 2, conversation_id.as_bytes());
    put_uint(o, 3, flags);
    put_bytes(o, 5, wake_word_phrase.as_bytes());
}

fn voice_response(o: &mut Vec<u8>, port: u32, error: bool) {
    put_uint(o, 1, port);
    put_bool(o, 2, error);
}

fn voice_event(o: &mut Vec<u8>, kind: u32, data: &[(String, String)]) {
    put_uint(o, 1, kind);
    for (name, value) in data {
        let mut pair = Vec::new();
        put_bytes(&mut pair, 1, name.as_bytes());
        put_bytes(&mut pair, 2, value.as_bytes());
        put_bytes(o, 2, &pair);
    }
}

fn voice_audio(o: &mut Vec<u8>, data: &[u8], end: bool) {
    put_bytes(o, 1, data);
    put_bool(o, 2, end);
}

fn announce_request(o: &mut Vec<u8>, media_id: &str, text: &str, start_conversation: bool) {
    put_bytes(o, 1, media_id.as_bytes());
    put_bytes(o, 2, text.as_bytes());
    put_bool(o, 4, start_conversation);
}

fn announce_finished(o: &mut Vec<u8>, success: bool) {
    put_bool(o, 1, success);
}
