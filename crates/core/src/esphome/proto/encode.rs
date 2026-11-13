//! Message encoding.

use super::wire::{put_bool, put_bytes, put_fixed32, put_uint};
use super::{Msg, ty};

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
}
