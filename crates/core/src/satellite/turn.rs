//! A turn on the dialog's side: starting one on a captured utterance, delivering its result, and stopping it
//! (REQ-VOI-020).

use super::playback::{Audio, apply_effects, duck, finish_speech, play_audio};
use super::reply::{Reply, stop_reply};
use super::settings::Ask;
use super::state::{Env, Finished, Pending, Purpose, SessionState};
use super::wire::send;
use crate::cancel::CancelToken;
use crate::voice::{Synthesizer, Transcriber, VoiceOutcome, hear, sentences};
use clawdot_protocol::message::{Indicator, Message};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use tokio::io::AsyncWrite;
use tokio::sync::{mpsc, oneshot};

/// Stop the running turn, the progress speech that is playing for it, and its reply if that is being made or
/// spoken (REQ-VOI-020).
pub(super) async fn stop_turn<W: AsyncWrite + Unpin>(
    st: &mut SessionState,
    wr: &mut W,
    pending: &mut Pending,
) -> Result<(), String> {
    stop_reply(st, wr, pending).await?;
    if let Some(t) = &st.turn_cancel {
        t.cancel();
        st.progress_q.clear();
        if matches!(&st.synth, Some(s) if matches!(s.purpose, Purpose::Progress)) {
            st.synth = None;
        }
        if st.out.audio == Audio::Progress {
            send(wr, &Message::StopPlayback).await?;
        }
    }
    Ok(())
}

impl<S, T> Env<S, T>
where
    S: Transcriber + Send + Sync + 'static,
    T: Synthesizer + Send + Sync + 'static,
{
    /// Phase: the finished turn is delivered once nothing it said earlier is still playing.
    pub(super) async fn deliver_finished<W: AsyncWrite + Unpin>(
        &self,
        st: &mut SessionState,
        wr: &mut W,
        pending: &mut Pending,
    ) -> Result<(), String> {
        // Nothing is delivered while a progress phrase is still being made: it comes first.
        let (playing, progress_empty) = (
            st.out.audio != Audio::Idle,
            st.progress_q.is_empty() && st.synth.is_none(),
        );
        let Some(Finished {
            outcome,
            effects,
            asked,
            speak,
        }) = st.finished.take_if(|_| !playing && progress_empty)
        else {
            return Ok(());
        };
        // A reply made sentence by sentence keeps a stop capture open until its first sound.
        let streamed = outcome.audio.is_empty() && !speak.is_empty();
        if !streamed && st.dialog.is_interrupting() {
            send(wr, &Message::StreamStop).await?;
        }
        apply_effects(st, wr, self.media_url, &effects).await?;
        if streamed {
            // The reply begins when its first sentence is ready (`pump_reply` asks for it); until then the
            // dialog is still thinking, so the cue and the stop capture work as before.
            st.reply = Some(Reply::new(speak, asked));
        } else if !outcome.audio.is_empty() {
            if asked {
                st.dialog.expect_answer();
            }
            st.dialog.on_reply_started();
            duck(wr, &mut st.out.music).await?;
            play_audio(wr, &outcome.audio).await?;
            st.out.audio = Audio::Speech;
        } else if outcome.transcript.is_some() && outcome.reply.is_empty() {
            // A command with nothing to say ("pause", "volume up"): do it now and go quiet. Music that a
            // realtime session paused comes back (nothing is playing now, or this would not run).
            finish_speech(st, wr).await?;
            if !st.dialog.is_muted() {
                send(wr, &Message::SetIndicator(Indicator::Idle)).await?;
            }
            pending.steps.push(st.dialog.on_reply_silent());
        } else {
            // The reply could not be spoken. The stream commands were the assistant's answer to the request
            // and do not depend on the speech, so they run now; left queued they would start after some later,
            // unrelated speech (REQ-MUS-004 only asks that music never covers speech, and none is playing).
            finish_speech(st, wr).await?;
            pending.steps.push(st.dialog.on_reply_failed());
        }
        Ok(())
    }

    /// Start a turn on a captured utterance: transcribe and ask the assistant. The reply is spoken afterwards, a
    /// sentence at a time (see `reply`).
    pub(super) fn start_turn(&self, st: &mut SessionState, pcm: Vec<i16>) {
        let (stt, asks) = (Arc::clone(&self.stt), self.asks.clone());
        let lang = self.settings.lang;
        let speaker = self.speaker_id.clone();
        let cancel = CancelToken::new();
        st.turn_cancel = Some(cancel.clone());
        let (progress_tx, rx) = mpsc::unbounded_channel();
        st.progress_rx = Some(rx);
        st.work = Some(Box::pin(async move {
            let effects = Arc::new(Mutex::new(Vec::new()));
            let sink = Arc::clone(&effects);
            let asked = Arc::new(AtomicBool::new(false));
            let asked_sink = Arc::clone(&asked);
            let heard = hear(&*stt, &pcm, move |text| async move {
                let (tx, rx) = oneshot::channel();
                if asks
                    .send(Ask {
                        speaker,
                        text,
                        reply: tx,
                        progress: Some(progress_tx),
                        cancel,
                    })
                    .await
                    .is_err()
                {
                    return "Sorry, I can't answer right now.".to_owned();
                }
                match rx.await {
                    Ok(a) => {
                        sink.lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .extend(a.effects);
                        asked_sink.store(a.expects_reply, Ordering::Relaxed);
                        a.say
                    }
                    Err(_) => "Sorry, I can't answer right now.".to_owned(),
                }
            })
            .await;
            let effects =
                std::mem::take(&mut *effects.lock().unwrap_or_else(PoisonError::into_inner));
            let speak = sentences(lang, &heard.reply);
            Finished {
                outcome: VoiceOutcome {
                    transcript: heard.transcript,
                    reply: heard.reply,
                    audio: Vec::new(),
                    problems: heard.problems,
                },
                effects,
                asked: asked.load(Ordering::Relaxed),
                speak,
            }
        }));
    }
}
