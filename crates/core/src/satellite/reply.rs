//! A turn's reply, spoken sentence by sentence as each is made.
//!
//! The reply text is complete when a turn finishes. The session then asks the voice for the first sentence only.
//! When that audio is ready the reply begins on the speaker (the dialog leaves its thinking phase, music is
//! paused, `audio-start` and the first chunks go out) and the next sentence is asked for while the first plays.
//! One `audio-start` … `audio-stop` stream carries the whole reply, so the protocol needs nothing new: the
//! satellite reports `playback-done` once, after the `audio-stop`, exactly as for a reply made all at once.
//!
//! What the session holds:
//!
//! - at most one synthesis in flight (`Reply::making`), and the audio of one sentence while it is written to the
//!   connection; nothing is queued in the core;
//! - a pace: the next sentence is not asked for until the speaker is estimated to have no more than
//!   [`Settings::reply_lead`](super::Settings::reply_lead) of audio left to play, so the speaker holds about one
//!   sentence beyond what is playing, and a stop wastes about one sentence of synthesis.
//!
//! The reply is dropped (its synthesis with it) when the listener stops it, when the satellite reports that
//! playback ended, or when the session ends.

use super::playback::{Audio, duck, finish_speech, send_chunks};
use super::state::{Env, Pending, SessionState};
use super::wire::send;
use crate::voice::Synthesizer;
use clawdot_protocol::message::{AudioFormat, Indicator, Message};
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::AsyncWrite;

/// One sentence being synthesised.
type Making = Pin<Box<dyn Future<Output = Result<Vec<i16>, String>> + Send>>;

/// A reply being made and spoken.
pub(super) struct Reply {
    /// Sentences not yet asked for.
    sentences: VecDeque<String>,
    /// The sentence being synthesised now; never more than one.
    making: Option<Making>,
    /// `audio-start` has been sent: the reply is on the speaker.
    pub(super) started: bool,
    /// The reply ends with a question, so the follow-up window after it is longer (REQ-AST-021).
    asked: bool,
    /// When the speaker will have played everything sent so far, estimated from the audio's length.
    plays_until: Instant,
    /// The earliest time the next sentence may be asked for.
    next_at: Instant,
}

impl Reply {
    pub(super) fn new(sentences: Vec<String>, asked: bool) -> Self {
        let now = Instant::now();
        Self {
            sentences: sentences.into(),
            making: None,
            started: false,
            asked,
            plays_until: now,
            next_at: now,
        }
    }
}

/// What the reply arm of the event loop woke for.
pub(super) enum Made {
    /// A sentence was synthesised (or failed).
    Audio(Result<Vec<i16>, String>),
    /// The pace allows the next sentence to be asked for.
    Due,
}

/// The reply arm of the event loop: the sentence being made, or the time the next one may be asked for.
pub(super) async fn poll_reply(reply: &mut Option<Reply>) -> Made {
    let Some(r) = reply.as_mut() else {
        return std::future::pending().await;
    };
    match r.making.as_mut() {
        Some(making) => {
            let made = making.await;
            r.making = None;
            Made::Audio(made)
        }
        None => {
            tokio::time::sleep_until(tokio::time::Instant::from_std(r.next_at)).await;
            Made::Due
        }
    }
}

/// The length of `samples` at the protocol's downlink rate.
fn playing_time(samples: usize) -> Duration {
    let rate = u64::from(AudioFormat::PCM_16K_MONO.rate);
    Duration::from_micros(
        u64::try_from(samples)
            .unwrap_or(u64::MAX)
            .saturating_mul(1_000_000)
            / rate,
    )
}

impl<S, T> Env<S, T>
where
    S: crate::voice::Transcriber + Send + Sync + 'static,
    T: Synthesizer + Send + Sync + 'static,
{
    /// Phase: ask for the next sentence when none is being made and the pace allows it.
    pub(super) fn pump_reply(&self, st: &mut SessionState) {
        let Some(r) = st.reply.as_mut() else { return };
        if r.making.is_none()
            && Instant::now() >= r.next_at
            && let Some(text) = r.sentences.pop_front()
        {
            let tts = Arc::clone(&self.tts);
            r.making = Some(Box::pin(async move { tts.synthesize(&text).await }));
        }
    }

    /// Phase: a sentence was made, or it is time to ask for the next one.
    pub(super) async fn on_made<W: AsyncWrite + Unpin>(
        &self,
        st: &mut SessionState,
        wr: &mut W,
        made: Made,
        pending: &mut Pending,
    ) -> Result<(), String> {
        match made {
            Made::Due => Ok(()),
            Made::Audio(Ok(audio)) => self.play_sentence(st, wr, &audio, pending).await,
            Made::Audio(Err(e)) => {
                // What was already sent stays played. The text of the error is the provider's own and never
                // holds audio or the key.
                eprintln!("clawdot: speech synthesis: {e}");
                end_reply(st, wr, pending).await
            }
        }
    }

    /// Send one sentence's audio, beginning the reply on the speaker first when this is its first sound.
    async fn play_sentence<W: AsyncWrite + Unpin>(
        &self,
        st: &mut SessionState,
        wr: &mut W,
        audio: &[i16],
        pending: &mut Pending,
    ) -> Result<(), String> {
        let Some(r) = st.reply.as_mut() else {
            return Ok(());
        };
        if !audio.is_empty() {
            if !r.started {
                r.started = true;
                if r.asked {
                    st.dialog.expect_answer();
                }
                if st.dialog.is_interrupting() {
                    send(wr, &Message::StreamStop).await?;
                }
                // The reply begins: the thinking cue stops, the microphone is ignored until it ends, and music
                // is paused for as long as the reply lasts.
                st.dialog.on_reply_started();
                duck(wr, &mut st.out.music).await?;
                send(wr, &Message::SetIndicator(Indicator::Speaking)).await?;
                send(wr, &Message::AudioStart(AudioFormat::PCM_16K_MONO)).await?;
                st.out.audio = Audio::Speech;
            }
            send_chunks(wr, audio).await?;
            let now = Instant::now();
            r.plays_until = r.plays_until.max(now) + playing_time(audio.len());
            r.next_at = r
                .plays_until
                .checked_sub(self.settings.reply_lead)
                .unwrap_or(now);
        }
        if r.sentences.is_empty() {
            // The last sentence is on its way: close the stream. The satellite's `playback-done` ends the reply.
            end_reply(st, wr, pending).await?;
        }
        Ok(())
    }
}

/// The reply is over, complete or cut short by a failed synthesis. A reply that began gets its `audio-stop`, so
/// what was sent plays out and the satellite reports `playback-done` as usual. A reply that never got a sound is
/// a reply that could not be spoken: the error cue, and any stream command that waited for it runs now.
async fn end_reply<W: AsyncWrite + Unpin>(
    st: &mut SessionState,
    wr: &mut W,
    pending: &mut Pending,
) -> Result<(), String> {
    let Some(r) = st.reply.take() else {
        return Ok(());
    };
    if r.started {
        send(wr, &Message::AudioStop).await
    } else {
        finish_speech(st, wr).await?;
        pending.steps.push(st.dialog.on_reply_failed());
        Ok(())
    }
}

/// The listener stopped the reply: drop the rest, and with it the synthesis in flight. A reply on the speaker is stopped there and ends like a finished one (music comes
/// back, the follow-up window opens); a reply that had not begun ends quietly.
pub(super) async fn stop_reply<W: AsyncWrite + Unpin>(
    st: &mut SessionState,
    wr: &mut W,
    pending: &mut Pending,
) -> Result<(), String> {
    let Some(r) = st.reply.take() else {
        return Ok(());
    };
    if r.started {
        send(wr, &Message::StopPlayback).await?;
        st.out.audio = Audio::Idle;
        finish_speech(st, wr).await?;
        pending.steps.push(st.dialog.on_playback_done());
    } else {
        finish_speech(st, wr).await?;
        if !st.dialog.is_muted() {
            send(wr, &Message::SetIndicator(Indicator::Idle)).await?;
        }
        pending.steps.push(st.dialog.on_reply_silent());
    }
    Ok(())
}
