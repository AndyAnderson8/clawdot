//! What the speaker is doing (speech and music) and how audio and media commands reach it: replies in pieces, music
//! ducked for speech, stream commands held until the speech ends, and the effects of a turn.

use super::state::{Pending, SessionState};
use super::wire::send;
use crate::assistant::{Effect, MediaCmd};
use clawdot_protocol::message::{AudioFormat, Indicator, MediaAction, Message};
use tokio::io::AsyncWrite;

/// Samples per downlink audio frame (about 0.26 s at 16 kHz).
pub(super) const DOWNLINK_CHUNK: usize = 4096;
/// Volume change for "louder" and "quieter", in percent.
pub(super) const VOLUME_STEP: u8 = 10;

/// What the satellite's speaker is doing for this session. One value, because the kinds exclude each other: a
/// realtime session starts only when the speaker is quiet, and progress speech and announcements start only
/// when it is idle, so no two are ever on at once.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum Audio {
    /// Nothing is playing.
    #[default]
    Idle,
    /// A reply or an announcement.
    Speech,
    /// A phrase spoken while the turn works; the dialog does not move on when it ends.
    Progress,
    /// A reply from the realtime session; its end must not open the follow-up window.
    Realtime,
}

/// What the music on the satellite is doing. Paused-for-speech is a state of its own, so music cannot be both
/// playing and waiting to be resumed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum Music {
    /// No music, or it was stopped.
    #[default]
    Off,
    /// Music is playing.
    Playing,
    /// Music was paused for speech and is to be resumed afterwards.
    Ducked,
}

/// What the satellite is playing: speech and music.
#[derive(Debug, Default)]
pub(super) struct Output {
    pub(super) audio: Audio,
    pub(super) music: Music,
}

/// Resume music that was paused for speech (unless a stream command is waiting), then send the waiting commands.
pub(super) async fn finish_speech<W: AsyncWrite + Unpin>(
    st: &mut SessionState,
    wr: &mut W,
) -> Result<(), String> {
    if st.out.music == Music::Ducked {
        // Paused music stays paused when a stream command is waiting; that command decides what plays.
        st.out.music = Music::Off;
        if st.media_after_speech.is_empty() {
            send(wr, &Message::MediaControl(MediaAction::Resume)).await?;
            st.out.music = Music::Playing;
        }
    }
    flush_media(wr, &mut st.media_after_speech, &mut st.out.music).await
}

/// The satellite finished playing what it was sent. Which kind of audio it was decides what happens next.
pub(super) async fn on_playback_done<W: AsyncWrite + Unpin>(
    st: &mut SessionState,
    wr: &mut W,
    pending: &mut Pending,
) -> Result<(), String> {
    match std::mem::take(&mut st.out.audio) {
        Audio::Idle => {}
        Audio::Realtime => {
            // A reply from the session ended. The session itself goes on listening.
            if st.rt.is_some() {
                send(wr, &Message::SetIndicator(Indicator::Listening)).await?;
            }
        }
        Audio::Progress => {
            // Progress speech ended; the turn is still running, so the dialog does not move on.
            st.dialog.set_output_playing(false);
        }
        Audio::Speech => {
            // A reply the satellite stopped on its own ends here too: nobody hears the rest, so it is not made.
            st.reply = None;
            finish_speech(st, wr).await?;
            pending.steps.push(st.dialog.on_playback_done());
        }
    }
    Ok(())
}

/// Volume and stop take effect now; stream commands wait until the satellite has finished speaking, so music
/// never covers the reply.
pub(super) async fn apply_effects<W: AsyncWrite + Unpin>(
    st: &mut SessionState,
    wr: &mut W,
    media_url: bool,
    effects: &[Effect],
) -> Result<(), String> {
    for e in effects {
        match e {
            Effect::StopPlayback => {
                send(wr, &Message::StopPlayback).await?;
                // Music paused for speech is stopped too, or the end of the reply would resume it.
                st.out.music = Music::Off;
                st.media_after_speech.clear();
            }
            Effect::VolumeSet(l) => {
                st.volume = (*l).min(100);
                send(wr, &Message::SetVolume { level: st.volume }).await?;
            }
            Effect::VolumeUp => {
                st.volume = st.volume.saturating_add(VOLUME_STEP).min(100);
                send(wr, &Message::SetVolume { level: st.volume }).await?;
            }
            Effect::VolumeDown => {
                st.volume = st.volume.saturating_sub(VOLUME_STEP);
                send(wr, &Message::SetVolume { level: st.volume }).await?;
            }
            Effect::PlayStream { url, title } if media_url => {
                st.media_after_speech.push(Message::PlayUrl {
                    url: url.clone(),
                    title: title.clone(),
                });
            }
            Effect::PlayStream { .. } => {
                send(
                    wr,
                    &Message::Error(clawdot_protocol::message::ErrorReport {
                        text: "this satellite did not declare media_url, so it cannot play streams"
                            .into(),
                        code: Some("media-unsupported".into()),
                    }),
                )
                .await?;
            }
            Effect::Media(MediaCmd::Pause) if media_url => {
                st.media_after_speech
                    .push(Message::MediaControl(MediaAction::Pause));
            }
            Effect::Media(MediaCmd::Resume) if media_url => {
                st.media_after_speech
                    .push(Message::MediaControl(MediaAction::Resume));
            }
            Effect::Media(_) => {}
        }
    }
    Ok(())
}

/// Show the speaking state and send a spoken reply: start, audio in pieces, stop.
pub(super) async fn play_audio<W: AsyncWrite + Unpin>(
    w: &mut W,
    audio: &[Vec<i16>],
) -> Result<(), String> {
    send(w, &Message::SetIndicator(Indicator::Speaking)).await?;
    send(w, &Message::AudioStart(AudioFormat::PCM_16K_MONO)).await?;
    for sentence in audio {
        send_chunks(w, sentence).await?;
    }
    send(w, &Message::AudioStop).await
}

/// Send audio to the speaker in pieces, inside a stream that is already open.
pub(super) async fn send_chunks<W: AsyncWrite + Unpin>(
    w: &mut W,
    audio: &[i16],
) -> Result<(), String> {
    for chunk in audio.chunks(DOWNLINK_CHUNK) {
        let payload: Vec<u8> = chunk.iter().flat_map(|s| s.to_le_bytes()).collect();
        send(
            w,
            &Message::AudioChunk {
                format: AudioFormat::PCM_16K_MONO,
                timestamp_ms: None,
                payload,
            },
        )
        .await?;
    }
    Ok(())
}

/// Pause music for the duration of speech (REQ-MUS-004); resumed on playback-done.
pub(super) async fn duck<W: AsyncWrite + Unpin>(
    w: &mut W,
    music: &mut Music,
) -> Result<(), String> {
    if *music == Music::Playing {
        send(w, &Message::MediaControl(MediaAction::Pause)).await?;
        *music = Music::Ducked;
    }
    Ok(())
}

/// Send the stream commands that were waiting for speech to finish, tracking whether music is now playing.
pub(super) async fn flush_media<W: AsyncWrite + Unpin>(
    w: &mut W,
    queued: &mut Vec<Message>,
    music: &mut Music,
) -> Result<(), String> {
    for m in queued.drain(..) {
        match &m {
            Message::PlayUrl { .. } | Message::MediaControl(MediaAction::Resume) => {
                *music = Music::Playing;
            }
            Message::MediaControl(MediaAction::Pause) => *music = Music::Off,
            _ => {}
        }
        send(w, &m).await?;
    }
    Ok(())
}
