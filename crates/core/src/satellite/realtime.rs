//! The optional realtime voice from this speaker's side (REQ-RTV-009, REQ-RTV-010): the open session, its audio in
//! both directions, and the session's events and end.

use super::playback::{Audio, DOWNLINK_CHUNK, Output, duck};
use super::settings::Settings;
use super::state::{Env, Finished, Purpose, SessionState, Synth};
use super::wire::send;
use crate::audio::{decode_pcm16, resample};
use crate::rtvoice::{Claim, Done, Job, Speaker};
use crate::voice::{Synthesizer, Transcriber, VoiceOutcome};
use clawdot_protocol::message::{AudioFormat, Indicator, Message};
use std::time::{Duration, Instant};
use tokio::io::AsyncWrite;
use tokio::sync::{mpsc, oneshot};

/// Model audio that stops arriving for this long ends the reply on the speaker.
pub(super) const RT_GAP: Duration = Duration::from_millis(300);
/// Microphone samples above this peak count as someone speaking, for the idle timeout.
pub(super) const RT_LOUD: u16 = 1500;

/// One open realtime session, from this speaker's side (REQ-RTV-009).
pub(super) struct RtLive {
    /// Microphone audio to the session. Dropping it ends the session.
    pub(super) mic: Option<mpsc::Sender<Vec<u8>>>,
    pub(super) speaker: mpsc::Receiver<Speaker>,
    pub(super) speaker_open: bool,
    pub(super) done: oneshot::Receiver<Done>,
    /// Frees the single session slot when this is dropped.
    pub(super) _claim: Claim,
    /// An odd byte of model audio waiting for its other half.
    pub(super) carry: Option<u8>,
    /// A reply is being streamed to the speaker (started, not yet stopped).
    pub(super) streaming: bool,
    pub(super) last_audio: Instant,
    pub(super) last_activity: Instant,
}

pub(super) enum RtEv {
    Out(Speaker),
    Gap,
    Done(Option<Done>),
}

/// Start a session for a speaker that is idle, unmuted and not in its cool-down, or `None` to use the normal
/// path. The claim makes sure only one session runs at a time.
pub(super) fn rt_start(
    settings: &Settings,
    speaker_id: &str,
    quiet: bool,
    blocked_until: Option<Instant>,
) -> Option<RtLive> {
    if !quiet || blocked_until.is_some_and(|t| Instant::now() < t) {
        return None;
    }
    let claim = settings.realtime.as_ref()?.claim()?;
    let (mic_tx, mic) = mpsc::channel(64);
    let (speaker_tx, speaker) = mpsc::channel(256);
    let (done_tx, done) = oneshot::channel();
    if !claim.submit(Job {
        speaker_id: speaker_id.to_owned(),
        mic,
        speaker: speaker_tx,
        done: done_tx,
    }) {
        return None;
    }
    let now = Instant::now();
    Some(RtLive {
        mic: Some(mic_tx),
        speaker,
        speaker_open: true,
        done,
        _claim: claim,
        carry: None,
        streaming: false,
        last_audio: now,
        last_activity: now,
    })
}

pub(super) async fn rt_next(rt: &mut Option<RtLive>) -> RtEv {
    let Some(r) = rt else {
        return std::future::pending().await;
    };
    let (streaming, deadline) = (r.streaming, r.last_audio + RT_GAP);
    let gap = async move {
        if streaming {
            tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)).await;
        } else {
            std::future::pending::<()>().await;
        }
    };
    tokio::pin!(gap);
    loop {
        tokio::select! {
            sp = r.speaker.recv(), if r.speaker_open => match sp {
                Some(s) => return RtEv::Out(s),
                // The session dropped its audio channel; its end is reported through `done`.
                None => r.speaker_open = false,
            },
            d = &mut r.done => return RtEv::Done(d.ok()),
            () = &mut gap => return RtEv::Gap,
        }
    }
}

/// Microphone audio (16 kHz) goes to the session at 24 kHz. A full queue drops audio rather than stalling the
/// connection.
pub(super) fn rt_mic(r: &mut RtLive, payload: &[u8]) {
    let pcm = decode_pcm16(payload);
    if pcm.iter().any(|s| s.unsigned_abs() > RT_LOUD) {
        r.last_activity = Instant::now();
    }
    let bytes: Vec<u8> = resample(&pcm, 16_000, 24_000)
        .iter()
        .flat_map(|s| s.to_le_bytes())
        .collect();
    if let Some(tx) = &r.mic {
        let _ = tx.try_send(bytes);
    }
}

/// Model audio (24 kHz) goes to the speaker at 16 kHz through the same start, chunks and stop as any reply, with
/// music ducked first (REQ-RTV-009).
pub(super) async fn rt_play<W: AsyncWrite + Unpin>(
    w: &mut W,
    r: &mut RtLive,
    bytes: &[u8],
    out: &mut Output,
) -> Result<(), String> {
    r.last_audio = Instant::now();
    r.last_activity = r.last_audio;
    let mut b: Vec<u8> = r.carry.take().into_iter().collect();
    b.extend_from_slice(bytes);
    if b.len() % 2 == 1 {
        r.carry = b.pop();
    }
    let pcm = resample(&decode_pcm16(&b), 24_000, 16_000);
    if pcm.is_empty() {
        return Ok(());
    }
    if !r.streaming {
        duck(w, &mut out.music).await?;
        send(w, &Message::SetIndicator(Indicator::Speaking)).await?;
        send(w, &Message::AudioStart(AudioFormat::PCM_16K_MONO)).await?;
        r.streaming = true;
        out.audio = Audio::Realtime;
    }
    for chunk in pcm.chunks(DOWNLINK_CHUNK) {
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

/// Barge-in: stop what the speaker is playing from the session and forget what was queued (REQ-RTV-009).
pub(super) async fn rt_flush<W: AsyncWrite + Unpin>(
    w: &mut W,
    r: Option<&mut RtLive>,
    audio: &mut Audio,
) -> Result<(), String> {
    if *audio == Audio::Realtime {
        send(w, &Message::StopPlayback).await?;
        *audio = Audio::Idle;
    }
    if let Some(r) = r {
        r.streaming = false;
        r.carry = None;
    }
    Ok(())
}

impl<S, T> Env<S, T>
where
    S: Transcriber + Send + Sync + 'static,
    T: Synthesizer + Send + Sync + 'static,
{
    /// Phase: an event from the realtime session.
    pub(super) async fn on_rt_event<W: AsyncWrite + Unpin>(
        &self,
        st: &mut SessionState,
        wr: &mut W,
        ev: RtEv,
    ) -> Result<(), String> {
        match ev {
            RtEv::Out(Speaker::Audio(bytes)) => {
                if let Some(r) = st.rt.as_mut() {
                    rt_play(wr, r, &bytes, &mut st.out).await?;
                }
            }
            RtEv::Out(Speaker::Flush) => {
                rt_flush(wr, st.rt.as_mut(), &mut st.out.audio).await?;
            }
            RtEv::Gap => {
                if let Some(r) = st.rt.as_mut() {
                    r.streaming = false;
                }
                send(wr, &Message::AudioStop).await?;
            }
            RtEv::Done(done) => self.on_rt_done(st, wr, done).await?,
        }
        Ok(())
    }

    /// The realtime session ended: play what is left, then deliver its end like a finished turn.
    pub(super) async fn on_rt_done<W: AsyncWrite + Unpin>(
        &self,
        st: &mut SessionState,
        wr: &mut W,
        done: Option<Done>,
    ) -> Result<(), String> {
        if let Some(mut r) = st.rt.take() {
            // Whatever the model said before it ended still gets played.
            while let Ok(Speaker::Audio(bytes)) = r.speaker.try_recv() {
                rt_play(wr, &mut r, &bytes, &mut st.out).await?;
            }
            if r.streaming {
                send(wr, &Message::AudioStop).await?;
            }
        }
        send(wr, &Message::StreamStop).await?;
        let done = done.unwrap_or(Done {
            speak: Some("I couldn't run the realtime voice.".into()),
            failed: true,
            effects: Vec::new(),
        });
        if done.failed {
            // Use the normal path for a while, so a broken service is not tried on every wake.
            st.rt_blocked_until = Some(Instant::now() + self.settings.rt_cooldown);
        }
        // The end of a session is delivered like a finished turn: after any audio still playing,
        // with its effects, and then either silence or the explanation in the normal voice.
        let mut outcome = VoiceOutcome {
            transcript: Some(String::new()),
            ..VoiceOutcome::default()
        };
        let mut closing = None;
        if let Some(text) = done.speak {
            outcome.transcript = None;
            outcome.reply = text.clone();
            closing = Some(text);
        }
        st.finished = Some(Finished {
            outcome,
            effects: done.effects,
            asked: false,
            speak: Vec::new(),
        });
        if let Some(text) = closing {
            // The speech is made while the session goes on reading (network synthesis can take long).
            // `deliver_finished` waits for it, and `on_synthesized` hands it to the waiting session end.
            st.synth = Some(Synth {
                purpose: Purpose::Closing,
                work: self.synthesize(text, None),
            });
        }
        Ok(())
    }
}
