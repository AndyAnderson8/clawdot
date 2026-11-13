//! One satellite connection: the accept loop, the session's event loop, and the phases for a message from the
//! satellite, the keepalive tick, the wake word and the dialog's steps.

use super::handshake::{handshake, reset_answer};
use super::playback::{Audio, on_playback_done};
use super::realtime::{rt_flush, rt_mic, rt_next, rt_start};
use super::reply::poll_reply;
use super::settings::{Ask, RESET_ASK_EVERY, Settings};
use super::state::{Env, Pending, SessionState, poll_stop, poll_synth, poll_work, recv_progress};
use super::turn::stop_turn;
use super::wire::{TimedWrite, cue_to_wire, next_message, send};
use crate::audio::decode_pcm16;
use crate::session::is_cancel;
use crate::voice::{Phase, Step, Synthesizer, Transcriber};
use clawdot_protocol::frame::{Decoder, Limits};
use clawdot_protocol::message::{AudioFormat, Indicator, Message};
use std::future::Future;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
use tokio::sync::{Semaphore, mpsc};

/// Run one satellite connection to completion.
///
/// The session is a handshake followed by one event loop. The loop waits for the next event (a message from the
/// satellite, a finished turn, a realtime event, a progress phrase, a stop check, an announcement or the
/// keepalive tick), hands it to the function for that phase, and then runs the phases that follow every event:
/// wake handling, progress speech, delivery of a finished turn, and the dialog's steps.
///
/// # Errors
/// A message when the session ended abnormally: a framing violation, a timeout, or a write failure. A satellite
/// that is simply refused (bad token or version) ends with `Ok`.
pub async fn run_session<IO, S, T>(
    io: IO,
    stt: Arc<S>,
    tts: Arc<T>,
    asks: mpsc::Sender<Ask>,
    settings: Settings,
) -> Result<(), String>
where
    IO: AsyncRead + AsyncWrite + Unpin + Send,
    S: Transcriber + Send + Sync + 'static,
    T: Synthesizer + Send + Sync + 'static,
{
    let (mut rd, wr) = tokio::io::split(io);
    let mut wr = TimedWrite::new(wr, settings.dead_after);
    let mut dec = Decoder::new(Limits::default());

    let Some(accepted) = handshake(&mut rd, &mut wr, &mut dec, &settings).await? else {
        return Ok(());
    };
    let mut st = SessionState::new(&settings);
    let env = Env {
        settings,
        stt,
        tts,
        asks,
        speaker_id: accepted.speaker_id,
        media_url: accepted.media_url,
        started: Instant::now(),
    };

    loop {
        let mut pending = Pending::default();
        tokio::select! {
            msg = next_message(&mut rd, &mut dec) => {
                st.last_heard = Instant::now();
                let Some(msg) = msg? else { return Ok(()) };
                env.on_message(&mut st, &mut wr, msg, &mut pending).await?;
            }
            done = poll_work(&mut st.work), if st.work.is_some() => {
                st.on_turn_finished(done);
            }
            ev = rt_next(&mut st.rt), if st.rt.is_some() => {
                env.on_rt_event(&mut st, &mut wr, ev).await?;
            }
            p = recv_progress(&mut st.progress_rx), if st.progress_rx.is_some() => {
                match p {
                    Some(p) => st.progress_q.push_back(p),
                    None => st.progress_rx = None,
                }
            }
            stop = poll_stop(&mut st.stop_check), if st.stop_check.is_some() => {
                st.stop_check = None;
                if stop {
                    stop_turn(&mut st, &mut wr, &mut pending).await?;
                }
            }
            (purpose, audio) = poll_synth(&mut st.synth), if st.synth.is_some() => {
                env.on_synthesized(&mut st, &mut wr, purpose, audio).await?;
            }
            made = poll_reply(&mut st.reply), if st.reply.is_some() => {
                env.on_made(&mut st, &mut wr, made, &mut pending).await?;
            }
            Some(a) = st.announce_rx.recv() => {
                env.on_announce(&mut st, a);
            }
            _ = st.tick.tick() => {
                env.on_tick(&mut st, &mut wr, &mut pending).await?;
            }
        }

        if pending.woke {
            env.on_wake(&mut st, &mut wr, &mut pending).await?;
        }
        st.end_realtime_when_muted();
        st.drop_stale_synthesis();
        env.play_progress(&mut st);
        env.deliver_finished(&mut st, &mut wr, &mut pending).await?;
        env.pump_reply(&mut st);
        env.run_steps(&mut st, &mut wr, pending.steps).await?;
    }
}

/// Accept satellites on `listener` until `shutdown` completes. Each connection is a task, capped at
/// `settings.max_sessions`; extra connections are closed at once.
///
/// # Errors
/// None today: a connection that cannot be accepted is logged and tried again. The `Result` stays so that callers
/// need not change if a failure that cannot be survived is ever added.
pub async fn serve<S, T>(
    listener: TcpListener,
    stt: Arc<S>,
    tts: Arc<T>,
    asks: mpsc::Sender<Ask>,
    settings: Settings,
    shutdown: impl Future<Output = ()> + Send,
) -> std::io::Result<()>
where
    S: Transcriber + Send + Sync + 'static,
    T: Synthesizer + Send + Sync + 'static,
{
    let slots = Arc::new(Semaphore::new(settings.max_sessions));
    let per_ip = crate::limit::IpLimiter::new(settings.max_per_ip);
    tokio::pin!(shutdown);
    loop {
        let (stream, peer) = tokio::select! {
            accepted = crate::limit::accept_retrying("satellite", crate::limit::ACCEPT_RETRY, || listener.accept()) => accepted,
            () = &mut shutdown => return Ok(()),
        };
        let Some(ip_guard) = per_ip.try_acquire(peer.ip()) else {
            drop(stream);
            continue;
        };
        let Ok(permit) = Arc::clone(&slots).try_acquire_owned() else {
            drop(stream);
            continue;
        };
        let (stt, tts, asks, mut settings) = (
            Arc::clone(&stt),
            Arc::clone(&tts),
            asks.clone(),
            settings.clone(),
        );
        settings.peer_loopback = peer.ip().is_loopback();
        tokio::spawn(async move {
            let _permit = permit;
            let _ip_guard = ip_guard;
            if let Err(e) = run_session(stream, stt, tts, asks, settings).await {
                eprintln!("clawdot: satellite {peer}: {e}");
            }
        });
    }
}

impl<S, T> Env<S, T>
where
    S: Transcriber + Send + Sync + 'static,
    T: Synthesizer + Send + Sync + 'static,
{
    pub(super) fn now_ms(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /// Phase: a message from the satellite.
    pub(super) async fn on_message<W: AsyncWrite + Unpin>(
        &self,
        st: &mut SessionState,
        wr: &mut W,
        msg: Message,
        pending: &mut Pending,
    ) -> Result<(), String> {
        match msg {
            Message::Ping => send(wr, &Message::Pong).await?,
            Message::Wake(_) => pending.woke = true,
            Message::Button(b) if b.name == "action" => {
                if st.rt.is_some() {
                    // The action button ends a realtime session.
                    rt_flush(wr, st.rt.as_mut(), &mut st.out.audio).await?;
                    if let Some(r) = st.rt.as_mut() {
                        r.mic = None;
                    }
                } else if (st.turn_cancel.is_some() && st.dialog.phase() == Phase::Thinking)
                    || st.reply.is_some()
                {
                    // While a turn is running, or its reply is being made or spoken, the action button stops
                    // it (REQ-VOI-020).
                    stop_turn(st, wr, pending).await?;
                } else {
                    pending.woke = true;
                }
            }
            Message::Button(b) if b.name == "mute" => {
                let muted = !st.dialog.is_muted();
                pending.steps.push(st.dialog.set_muted(muted));
                send(wr, &Message::SetMute { muted }).await?;
            }
            Message::MuteChanged { muted } => pending.steps.push(st.dialog.set_muted(muted)),
            Message::AdminReset => {
                let slow = st
                    .last_reset_ask
                    .is_some_and(|t| t.elapsed() < RESET_ASK_EVERY);
                st.last_reset_ask = Some(Instant::now());
                let (open_secs, reason) = reset_answer(&self.settings, st.reset_trusted, slow);
                send(wr, &Message::AdminResetWindow { open_secs, reason }).await?;
            }
            Message::AudioChunk {
                format, payload, ..
            } => {
                self.on_uplink_audio(st, wr, format, &payload, pending)
                    .await?
            }
            Message::PlaybackDone { .. } => on_playback_done(st, wr, pending).await?,
            // Pong, button names we do not use, errors, unknown events, and anything that only
            // the core should send are all ignored, so newer satellites keep working (REQ-SAT-009).
            _ => {}
        }
        Ok(())
    }

    /// Microphone audio goes to the realtime session if there is one, else to the dialog's endpointer.
    pub(super) async fn on_uplink_audio<W: AsyncWrite + Unpin>(
        &self,
        st: &mut SessionState,
        wr: &mut W,
        format: AudioFormat,
        payload: &[u8],
        pending: &mut Pending,
    ) -> Result<(), String> {
        if format != AudioFormat::PCM_16K_MONO {
            if !st.warned_format {
                st.warned_format = true;
                send(
                    wr,
                    &Message::Error(clawdot_protocol::message::ErrorReport {
                        text: "uplink audio must be 16 kHz 16-bit mono".into(),
                        code: Some("bad-audio-format".into()),
                    }),
                )
                .await?;
            }
        } else if let Some(r) = st.rt.as_mut() {
            rt_mic(r, payload);
        } else {
            pending
                .steps
                .push(st.dialog.on_audio(self.now_ms(), &decode_pcm16(payload)));
        }
        Ok(())
    }

    /// Phase: the keepalive tick (liveness, ping, dialog timers, idle realtime sessions).
    pub(super) async fn on_tick<W: AsyncWrite + Unpin>(
        &self,
        st: &mut SessionState,
        wr: &mut W,
        pending: &mut Pending,
    ) -> Result<(), String> {
        if st.last_heard.elapsed() >= self.settings.dead_after {
            return Err("the satellite stopped responding".into());
        }
        if st.last_ping.elapsed() >= self.settings.keepalive {
            st.last_ping = Instant::now();
            send(wr, &Message::Ping).await?;
        }
        pending.steps.push(st.dialog.tick(self.now_ms()));
        // A session nobody is talking in ends, so the microphone does not stream (and cost) for nothing.
        if let Some(r) = st.rt.as_mut()
            && st.out.audio == Audio::Idle
            && !r.streaming
            && r.last_activity.elapsed() >= self.settings.rt_idle
        {
            r.mic = None;
        }
        Ok(())
    }

    /// Phase: the wake word (or the action button). In a realtime session it cuts in; otherwise a realtime
    /// session starts if one is available and the speaker is quiet, else the dialog handles it.
    pub(super) async fn on_wake<W: AsyncWrite + Unpin>(
        &self,
        st: &mut SessionState,
        wr: &mut W,
        pending: &mut Pending,
    ) -> Result<(), String> {
        if st.rt.is_some() {
            // The wake word during a session is the user cutting in.
            rt_flush(wr, st.rt.as_mut(), &mut st.out.audio).await?;
        } else {
            let quiet = st.is_quiet();
            match rt_start(&self.settings, &self.speaker_id, quiet, st.rt_blocked_until) {
                Some(live) => {
                    send(wr, &Message::SetIndicator(Indicator::Listening)).await?;
                    send(wr, &Message::StreamStart).await?;
                    st.rt = Some(live);
                }
                None => pending.steps.push(st.dialog.on_wake()),
            }
        }
        Ok(())
    }

    /// Phase: carry out what the dialog asked for (cues, the interrupt check, starting a turn).
    pub(super) async fn run_steps<W: AsyncWrite + Unpin>(
        &self,
        st: &mut SessionState,
        wr: &mut W,
        steps: Vec<Step>,
    ) -> Result<(), String> {
        for step in steps {
            match step {
                Step::Nothing => {}
                Step::Cue(cue) => cue_to_wire(cue, wr).await?,
                Step::Interrupt(pcm) => {
                    // The capture opened by a wake word while the turn runs: close it, and check what was
                    // said against the stop phrases only (REQ-VOI-020).
                    send(wr, &Message::StreamStop).await?;
                    send(wr, &Message::SetIndicator(Indicator::Thinking)).await?;
                    if !pcm.is_empty() && st.stop_check.is_none() {
                        let stt = Arc::clone(&self.stt);
                        let lang = self.settings.lang;
                        st.stop_check = Some(Box::pin(async move {
                            stt.transcribe(&pcm)
                                .await
                                .is_ok_and(|t| is_cancel(lang, &t))
                        }));
                    }
                }
                Step::Utterance(pcm) => {
                    send(wr, &Message::StreamStop).await?;
                    send(wr, &Message::SetIndicator(Indicator::Thinking)).await?;
                    self.start_turn(st, pcm);
                }
            }
        }
        Ok(())
    }
}
