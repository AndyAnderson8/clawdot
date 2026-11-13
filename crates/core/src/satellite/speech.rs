//! Speech made outside a turn: announcements from elsewhere in the program, progress phrases while a turn works, and
//! the closing line of a realtime session.

use super::announcer::{Announce, Request};
use super::playback::{Audio, duck, play_audio};
use super::state::{Env, Purpose, SessionState, Synth};
use crate::voice::{Synthesizer, Transcriber, speak_text_as};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::io::AsyncWrite;

impl<S, T> Env<S, T>
where
    S: Transcriber + Send + Sync + 'static,
    T: Synthesizer + Send + Sync + 'static,
{
    /// Phase: someone elsewhere in the program wants this speaker to say something.
    pub(super) fn on_announce(&self, st: &mut SessionState, a: Announce) {
        let Announce { request, answer } = a;
        let idle = st.is_quiet() && st.rt.is_none() && st.synth.is_none();
        match request {
            Request::Probe => {
                let _ = answer.send(idle);
            }
            // The speech is made while the session goes on reading; `on_synthesized` plays it and answers.
            Request::Speak { text, voice } if idle => {
                st.synth = Some(Synth {
                    purpose: Purpose::Announcement(answer),
                    work: self.synthesize(text, voice),
                });
            }
            Request::Speak { .. } => {
                let _ = answer.send(false);
            }
        }
    }

    /// Phase: speech made outside a turn is ready. It plays only if the speaker is still in the state it was
    /// wanted in, since the user may have spoken, muted or stopped the turn while it was being made.
    pub(super) async fn on_synthesized<W: AsyncWrite + Unpin>(
        &self,
        st: &mut SessionState,
        wr: &mut W,
        purpose: Purpose,
        audio: Vec<Vec<i16>>,
    ) -> Result<(), String> {
        match purpose {
            Purpose::Announcement(answer) => {
                let accepted = !audio.is_empty() && st.is_quiet() && st.rt.is_none();
                if accepted {
                    st.dialog.on_reply_started();
                    duck(wr, &mut st.out.music).await?;
                    play_audio(wr, &audio).await?;
                    st.out.audio = Audio::Speech;
                }
                let _ = answer.send(accepted);
            }
            Purpose::Closing => {
                if let Some(f) = st.finished.as_mut() {
                    f.outcome.audio = audio;
                }
            }
            Purpose::Progress => {
                if !audio.is_empty() && st.wants_progress_speech() {
                    st.dialog.set_output_playing(true);
                    duck(wr, &mut st.out.music).await?;
                    play_audio(wr, &audio).await?;
                    st.out.audio = Audio::Progress;
                }
            }
        }
        Ok(())
    }

    /// Make speech for `text` as a future the event loop polls, in `voice` when one is given.
    pub(super) fn synthesize(
        &self,
        text: String,
        voice: Option<String>,
    ) -> Pin<Box<dyn Future<Output = Vec<Vec<i16>>> + Send>> {
        let tts = Arc::clone(&self.tts);
        let lang = self.settings.lang;
        Box::pin(async move {
            let mut problems = Vec::new();
            speak_text_as(lang, &*tts, &text, voice.as_deref(), &mut problems).await
        })
    }

    /// Phase: progress speech is played one phrase at a time while the turn goes on (REQ-AST-020).
    pub(super) fn play_progress(&self, st: &mut SessionState) {
        if st.synth.is_none()
            && st.wants_progress_speech()
            && let Some(text) = st.progress_q.pop_front()
        {
            st.synth = Some(Synth {
                purpose: Purpose::Progress,
                work: self.synthesize(text, None),
            });
        }
    }
}
