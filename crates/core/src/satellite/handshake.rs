//! The hello exchange (REQ-SAT-002) and the answer to an administrator-reset request (REQ-SAT-020).

use super::settings::Settings;
use super::wire::{new_session_id, next_message, send};
use crate::inbox::endpoint::verify_token;
use clawdot_protocol::frame::Decoder;
use clawdot_protocol::message::{AudioCodec, Hello, Message, PROTOCOL_VERSION, Reject, Welcome};
use tokio::io::{AsyncRead, AsyncWrite};

/// What the handshake learned about an accepted satellite.
pub(super) struct Accepted {
    pub(super) speaker_id: String,
    pub(super) media_url: bool,
}

/// The hello exchange (REQ-SAT-002): the first message must be a compatible, authorised hello, answered with
/// the welcome. `None` means the satellite was refused (or hung up) and the session is over.
pub(super) async fn handshake<R, W>(
    rd: &mut R,
    wr: &mut W,
    dec: &mut Decoder,
    settings: &Settings,
) -> Result<Option<Accepted>, String>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let first = tokio::time::timeout(settings.hello_timeout, next_message(rd, dec))
        .await
        .map_err(|_| "no hello within the allowed time".to_owned())??;
    let hello = match first {
        Some(Message::Hello(h)) => h,
        Some(_) => {
            reject(wr, "the first message must be hello".into()).await?;
            return Ok(None);
        }
        None => return Ok(None),
    };
    if let Some(reason) = refusal(&hello, settings) {
        reject(wr, reason).await?;
        return Ok(None);
    }
    // The connection's own random identifier. It names this speaker's conversation and held confirmation in the
    // core, so one speaker's "yes" can never answer another's question; a new connection starts fresh.
    let (Ok(speaker_part), Ok(session_id)) = (new_session_id(), new_session_id()) else {
        // Without a random id every speaker would share one: refuse rather than mix them up.
        reject(wr, "unavailable".into()).await?;
        return Ok(None);
    };
    let speaker_id = format!("sat:{speaker_part}");
    send(
        wr,
        &Message::Welcome(Welcome {
            protocol: PROTOCOL_VERSION,
            session_id,
            audio_out: AudioCodec::Pcm,
        }),
    )
    .await?;
    Ok(Some(Accepted {
        speaker_id,
        media_url: hello.capabilities.media_url,
    }))
}

/// Why a hello is refused, if it is: an incompatible protocol version, or a token that does not match when the
/// core requires one.
fn refusal(hello: &Hello, settings: &Settings) -> Option<String> {
    if !PROTOCOL_VERSION.compatible_with(hello.protocol) {
        return Some(format!(
            "protocol {}.{} is not supported; this core speaks {}.{}",
            hello.protocol.major,
            hello.protocol.minor,
            PROTOCOL_VERSION.major,
            PROTOCOL_VERSION.minor
        ));
    }
    if !settings.token_hashes.is_empty() {
        let ok = hello
            .token
            .as_deref()
            .is_some_and(|t| settings.token_hashes.iter().any(|h| verify_token(t, h)));
        if !ok {
            return Some("unauthorized".into());
        }
    }
    None
}

pub(super) async fn reject<W: AsyncWrite + Unpin>(w: &mut W, reason: String) -> Result<(), String> {
    send(w, &Message::Reject(Reject { reason })).await
}

/// The answer to an administrator-reset request: the seconds the window stays open, or a reason it did not.
pub(super) fn reset_answer(
    settings: &Settings,
    trusted: bool,
    slow: bool,
) -> (u32, Option<String>) {
    match (&settings.admin_reset, trusted, slow) {
        (_, _, true) => (0, Some("Too many requests. Wait a few seconds.".to_owned())),
        (Some(hook), true, false) => match (hook.0)() {
            Ok(s) => (s, None),
            Err(why) => (0, Some(why)),
        },
        (Some(_), false, false) => (
            0,
            Some("This speaker is not trusted to open the reset window.".to_owned()),
        ),
        (None, _, _) => (0, Some("Not available.".to_owned())),
    }
}
