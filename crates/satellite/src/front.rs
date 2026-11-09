//! The audio front end: a [`crate::Device`] built from an [`AudioIn`], an [`AudioOut`] and a [`Trigger`].
//!
//! With it the reference satellite runs a real spoken turn: a key press (or, later, a wake word) starts it, the
//! microphone streams until the core finds the end of the utterance, and the reply plays on the speaker. The
//! dialog rules (endpointing, mute, half-duplex, follow-up window, speech-to-text, text-to-speech) all live in
//! the core; this module only moves samples (REQ-VOI-013, 017, 018).

use crate::Device;
use crate::audio::{AudioError, AudioIn, AudioOut};
use crate::hw::{Button, Buttons, LedState, Leds, NullButtons, NullLeds};
use clawdot_protocol::message::Indicator;
use std::collections::VecDeque;
use std::io::BufRead;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::thread;
use std::time::{Duration, Instant};

/// What the user did since the last look.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Press {
    /// Nothing.
    Nothing,
    /// Start a turn (push-to-talk key, or a wake word was heard).
    Talk,
    /// Leave.
    Quit,
}

/// A source of turn triggers. A wake-word detector implements this too (REQ-VOI-017), with a different
/// way of deciding.
pub trait Trigger {
    /// Poll without blocking.
    fn poll(&mut self) -> Press;
}

impl<T: Trigger + ?Sized> Trigger for Box<T> {
    fn poll(&mut self) -> Press {
        (**self).poll()
    }
}

/// Never triggers: for devices where the core opens listening itself.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoTrigger;

impl Trigger for NoTrigger {
    fn poll(&mut self) -> Press {
        Press::Nothing
    }
}

/// Triggers once, at the first poll: for one-shot and headless runs.
#[derive(Debug, Default, Clone, Copy)]
pub struct Immediate {
    done: bool,
}

impl Trigger for Immediate {
    fn poll(&mut self) -> Press {
        if std::mem::replace(&mut self.done, true) {
            Press::Nothing
        } else {
            Press::Talk
        }
    }
}

/// A scripted trigger for tests: returns the presses in order, then nothing.
#[derive(Debug, Default, Clone)]
pub struct Scripted(pub VecDeque<Press>);

impl Trigger for Scripted {
    fn poll(&mut self) -> Press {
        self.0.pop_front().unwrap_or(Press::Nothing)
    }
}

/// Push-to-talk from lines of text: Enter (or any text) talks; `q`, `quit` or `exit` leaves; the end of the
/// input leaves too. A thread does the blocking read, so no terminal library is needed.
#[derive(Debug)]
pub struct LineTrigger {
    rx: Receiver<Press>,
}

impl LineTrigger {
    /// Read from any line source.
    pub fn new(reader: impl BufRead + Send + 'static) -> Self {
        let (tx, rx) = channel();
        thread::spawn(move || {
            for line in reader.lines() {
                let Ok(line) = line else { break };
                let press = match line.trim().to_ascii_lowercase().as_str() {
                    "q" | "quit" | "exit" => Press::Quit,
                    _ => Press::Talk,
                };
                if tx.send(press).is_err() {
                    return;
                }
            }
            let _ = tx.send(Press::Quit);
        });
        Self { rx }
    }

    /// Read from the terminal.
    #[must_use]
    pub fn stdin() -> Self {
        Self::new(std::io::BufReader::new(std::io::stdin()))
    }
}

impl Trigger for LineTrigger {
    fn poll(&mut self) -> Press {
        match self.rx.try_recv() {
            Ok(p) => p,
            Err(TryRecvError::Empty) => Press::Nothing,
            Err(TryRecvError::Disconnected) => Press::Quit,
        }
    }
}

/// Listens to the microphone while the device idles and says when a wake word was heard (REQ-DEV-025). The
/// reference satellite knows nothing about any engine; `clawdot-device` plugs one in behind this trait.
pub trait WakeSink: Send + std::fmt::Debug {
    /// What to tell the user to say, for status lines, for example `okay nabu`.
    fn name(&self) -> String;
    /// Offer 20 ms of audio. `Some(phrase)` when a wake word ended in it. The sink keeps no audio.
    fn feed(&mut self, pcm: &[i16]) -> Option<String>;
    /// Drop all streaming state: after a turn, after speech, after mute.
    fn reset(&mut self);
}

/// Counters of a [`ScriptedWake`], shared so a test can read them while the device owns the sink.
#[derive(Debug, Default, Clone)]
pub struct WakeStats {
    /// Frames offered to the sink.
    pub fed: usize,
    /// Wake words reported.
    pub heard: usize,
    /// Resets requested.
    pub resets: usize,
}

/// A test sink: reports a wake word for every frame whose samples all equal [`ScriptedWake::MARK`]. Any other
/// audio is "not the wake word". Not an engine for real use.
#[derive(Debug, Clone, Default)]
pub struct ScriptedWake {
    /// What the counters say.
    pub stats: std::sync::Arc<std::sync::Mutex<WakeStats>>,
}

impl ScriptedWake {
    /// The sample value that makes up a frame of "wake word".
    pub const MARK: i16 = 12_345;

    /// Twenty milliseconds of the wake word.
    #[must_use]
    pub fn frame() -> Vec<i16> {
        vec![Self::MARK; crate::audio::FRAME_SAMPLES]
    }

    fn with_stats<R>(&self, f: impl FnOnce(&mut WakeStats) -> R) -> R {
        let mut g = self
            .stats
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f(&mut g)
    }
}

impl WakeSink for ScriptedWake {
    fn name(&self) -> String {
        "scripted".into()
    }

    fn feed(&mut self, pcm: &[i16]) -> Option<String> {
        let hit = !pcm.is_empty() && pcm.iter().all(|&s| s == Self::MARK);
        self.with_stats(|s| {
            s.fed += 1;
            if hit {
                s.heard += 1;
            }
        });
        hit.then(|| "scripted".to_owned())
    }

    fn reset(&mut self) {
        self.with_stats(|s| s.resets += 1);
    }
}

/// Settings for [`AudioDevice`].
#[derive(Debug, Clone, Copy)]
pub struct DeviceConfig {
    /// Leave after this many replies; `None` runs until the trigger says quit.
    pub max_turns: Option<usize>,
    /// Stop capturing if the core does not ask for audio this long after a press.
    pub arm_timeout: Duration,
    /// Print what is happening (one short line per state) to standard error.
    pub verbose: bool,
    /// Keep the microphone open between turns (REQ-DEV-025): wake word detection and no lost first words.
    pub always_on: bool,
    /// After a reply or the end of speech, ignore the wake word for this long (REQ-DEV-026).
    pub holdoff: Duration,
    /// Frames kept between a wake or press and the core asking for audio (2 s at 20 ms).
    pub queue_frames: usize,
}

impl Default for DeviceConfig {
    fn default() -> Self {
        Self {
            max_turns: None,
            arm_timeout: Duration::from_secs(3),
            verbose: false,
            always_on: false,
            holdoff: Duration::from_millis(500),
            queue_frames: 100,
        }
    }
}

/// The time source, replaceable in tests.
struct Clock(Box<dyn Fn() -> Instant + Send>);

impl std::fmt::Debug for Clock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Clock")
    }
}

/// First and longest wait before reopening a failed microphone (REQ-DEV-028).
const RESTART_MIN: Duration = Duration::from_secs(1);
const RESTART_MAX: Duration = Duration::from_secs(10);

/// Most problems kept in [`AudioDevice::problems`].
const MAX_PROBLEMS: usize = 32;
/// Frames read per pass of the satellite loop when catching up (REQ-VOI-013): 80 ms.
const BURST: usize = 4;

/// A satellite device made of an audio input, an audio output and a trigger.
#[derive(Debug)]
pub struct AudioDevice<I, O, T> {
    input: I,
    output: O,
    trigger: T,
    cfg: DeviceConfig,
    capturing: bool,
    armed_at: Option<Instant>,
    listening: bool,
    playing: bool,
    muted: bool,
    quit: bool,
    turns: usize,
    leds: Box<dyn Leds>,
    buttons: Box<dyn Buttons>,
    wake: Option<Box<dyn WakeSink>>,
    clock: Clock,
    /// Frames captured since a wake or press, waiting for the core to ask for audio.
    queue: VecDeque<Vec<i16>>,
    holdoff_until: Option<Instant>,
    restart_at: Option<Instant>,
    restart_delay: Duration,
    recovering: bool,
    mic_unavailable: bool,
    wakes: usize,
    /// Problems met so far (a missing program, a stalled device), oldest first. Never contains audio.
    pub problems: Vec<String>,
}

impl<I: AudioIn, O: AudioOut, T: Trigger> AudioDevice<I, O, T> {
    /// A device over the given parts.
    #[must_use]
    pub fn new(input: I, output: O, trigger: T, cfg: DeviceConfig) -> Self {
        Self {
            input,
            output,
            trigger,
            cfg,
            capturing: false,
            armed_at: None,
            listening: false,
            playing: false,
            muted: false,
            quit: false,
            turns: 0,
            leds: Box::new(NullLeds),
            buttons: Box::new(NullButtons),
            wake: None,
            clock: Clock(Box::new(Instant::now)),
            queue: VecDeque::new(),
            holdoff_until: None,
            restart_at: None,
            restart_delay: RESTART_MIN,
            recovering: false,
            mic_unavailable: false,
            wakes: 0,
            problems: Vec::new(),
        }
    }

    /// Listen for a wake word with this sink. Needs `always_on` in the [`DeviceConfig`] to do anything.
    #[must_use]
    pub fn with_wake(mut self, sink: impl WakeSink + 'static) -> Self {
        self.wake = Some(Box::new(sink));
        self
    }

    /// Like [`AudioDevice::with_wake`] for a sink that is already boxed.
    #[must_use]
    pub fn with_wake_boxed(mut self, sink: Box<dyn WakeSink>) -> Self {
        self.wake = Some(sink);
        self
    }

    /// Read time from this clock instead of the system's (tests).
    #[must_use]
    pub fn with_clock(mut self, clock: impl Fn() -> Instant + Send + 'static) -> Self {
        self.clock = Clock(Box::new(clock));
        self
    }

    /// Wake words heard so far.
    #[must_use]
    pub fn wakes(&self) -> usize {
        self.wakes
    }

    /// Whether the microphone is open right now.
    #[must_use]
    pub fn mic_open(&self) -> bool {
        self.capturing
    }

    fn now(&self) -> Instant {
        (self.clock.0)()
    }

    /// Use these lights (optional; the default shows nothing).
    #[must_use]
    pub fn with_leds(mut self, leds: impl Leds + 'static) -> Self {
        self.leds = Box::new(leds);
        self
    }

    /// Use these buttons (optional; `Talk` starts a turn, `Stop` silences playback).
    #[must_use]
    pub fn with_buttons(mut self, buttons: impl Buttons + 'static) -> Self {
        self.buttons = Box::new(buttons);
        self
    }

    /// The audio output, for inspection after a run.
    #[must_use]
    pub fn output(&self) -> &O {
        &self.output
    }

    /// The audio output, to play a local sound (a tone) between replies.
    pub fn output_mut(&mut self) -> &mut O {
        &mut self.output
    }

    /// Replies received so far.
    #[must_use]
    pub fn turns(&self) -> usize {
        self.turns
    }

    fn say(&self, text: &str) {
        if self.cfg.verbose {
            eprintln!("{text}");
        }
    }

    fn problem(&mut self, what: &str, e: &AudioError) {
        let msg = format!("{what}: {e}");
        eprintln!("clawdot-satellite: {msg}");
        if self.problems.len() < MAX_PROBLEMS {
            self.problems.push(msg);
        }
    }

    fn start_capture(&mut self) -> bool {
        if self.capturing {
            return true;
        }
        if self.mic_unavailable {
            return false;
        }
        match self.input.start() {
            Ok(()) => {
                self.capturing = true;
                true
            }
            Err(e) => {
                self.mic_failed("microphone", &e);
                false
            }
        }
    }

    fn stop_capture(&mut self) {
        self.capturing = false;
        self.armed_at = None;
        self.input.stop();
    }

    /// The microphone failed: say so, release it, and (always-on) plan to reopen it (REQ-DEV-028).
    fn mic_failed(&mut self, what: &str, e: &AudioError) {
        self.problem(what, e);
        self.stop_capture();
        self.queue.clear();
        if !self.cfg.always_on {
            return;
        }
        if matches!(e, AudioError::NotFound(_) | AudioError::Format(_)) {
            // Retrying cannot install a program.
            self.mic_unavailable = true;
            self.say("microphone: not available; the wake word is off (push-to-talk may not work either)");
            return;
        }
        let delay = self.restart_delay;
        self.restart_at = Some(self.now() + delay);
        self.restart_delay = (delay * 2).min(RESTART_MAX);
        self.recovering = true;
        self.say(&format!(
            "microphone: stopped; reopening in {} s",
            delay.as_secs()
        ));
    }

    /// A frame arrived: the microphone is healthy.
    fn mic_ok(&mut self) {
        self.restart_delay = RESTART_MIN;
        if std::mem::take(&mut self.recovering) {
            self.say("microphone: back");
        }
    }

    fn reset_wake(&mut self) {
        if let Some(w) = self.wake.as_mut() {
            w.reset();
        }
    }

    /// Quiet period after speech or a reply, with the engine starting afresh (REQ-DEV-026).
    fn hold_off(&mut self) {
        if self.cfg.always_on {
            self.reset_wake();
            self.queue.clear();
            self.holdoff_until = Some(self.now() + self.cfg.holdoff);
        }
    }

    /// Reply playback is over (finished, failed or stopped).
    fn playback_over(&mut self) {
        if std::mem::replace(&mut self.playing, false) {
            self.hold_off();
        }
    }

    /// Keep the microphone open between turns, reopening it after a fault.
    fn keep_microphone_open(&mut self, now: Instant) {
        if self.capturing || self.muted || self.mic_unavailable {
            return;
        }
        if self.restart_at.is_some_and(|t| now < t) {
            return;
        }
        self.restart_at = None;
        if self.start_capture() && self.queue.is_empty() {
            self.say("microphone: open");
        }
    }

    /// Read what the microphone heard while idle: listen for the wake word, or keep the audio that follows one.
    /// Returns true when a wake word was heard.
    fn pump_idle(&mut self, now: Instant) -> bool {
        if !self.capturing || self.listening {
            return false;
        }
        for _ in 0..2 * BURST {
            let frame = match self.input.read_frame(Duration::ZERO) {
                Ok(Some(f)) => f,
                Ok(None) => break,
                Err(e) => {
                    let what = if e == AudioError::Eof {
                        "microphone source ended"
                    } else {
                        "microphone"
                    };
                    self.mic_failed(what, &e);
                    break;
                }
            };
            self.mic_ok();
            let deaf = self.playing || self.holdoff_until.is_some_and(|t| now < t);
            if deaf {
                continue;
            }
            if self.armed_at.is_some() {
                if self.queue.len() >= self.cfg.queue_frames.max(1) {
                    self.queue.pop_front();
                }
                self.queue.push_back(frame);
                continue;
            }
            if let Some(phrase) = self.wake.as_mut().and_then(|w| w.feed(&frame)) {
                self.wakes += 1;
                self.say(&format!("wake word heard: \"{phrase}\""));
                self.armed_at = Some(now);
                self.queue.clear();
                return true;
            }
        }
        false
    }
}

impl<I: AudioIn, O: AudioOut, T: Trigger> Device for AudioDevice<I, O, T> {
    fn read_mic(&mut self) -> Option<Vec<i16>> {
        if !self.capturing {
            return None;
        }
        if let Some(frame) = self.queue.pop_front() {
            return Some(frame);
        }
        match self.input.read_frame(Duration::ZERO) {
            Ok(frame) => {
                if frame.is_some() {
                    self.mic_ok();
                }
                frame
            }
            Err(e) => {
                let what = if e == AudioError::Eof {
                    "microphone source ended"
                } else {
                    "microphone"
                };
                // Release it so the next turn starts afresh instead of reading a dead source.
                self.mic_failed(what, &e);
                None
            }
        }
    }

    fn mic_active(&mut self, on: bool) {
        self.listening = on;
        if on {
            self.armed_at = None;
            if !self.muted {
                self.start_capture();
            }
        } else if self.cfg.always_on {
            // Keep the microphone; forget the turn's audio and ignore the tail of the speech.
            self.armed_at = None;
            self.hold_off();
        } else {
            self.stop_capture();
        }
    }

    fn mic_burst(&self) -> usize {
        BURST
    }

    fn poll_wake(&mut self) -> bool {
        let now = self.now();
        if let Some(at) = self.armed_at
            && !self.listening
            && now.saturating_duration_since(at) >= self.cfg.arm_timeout
        {
            // The core never asked for audio (busy, or muted from its side): do not keep the audio, and
            // (outside always-on mode) do not keep the microphone open.
            self.armed_at = None;
            self.queue.clear();
            if !self.cfg.always_on {
                self.stop_capture();
            }
        }
        if self.cfg.always_on {
            self.keep_microphone_open(now);
        }
        let press = match self.buttons.poll() {
            Some(Button::Talk) => Press::Talk,
            Some(Button::Stop) => {
                self.stop_playback();
                Press::Nothing
            }
            Some(Button::Mute) | None => self.trigger.poll(),
        };
        match press {
            Press::Nothing => self.cfg.always_on && self.pump_idle(now),
            Press::Quit => {
                self.quit = true;
                false
            }
            Press::Talk => {
                if self.playing || self.muted {
                    self.say("(not now: the assistant is speaking or the microphone is muted)");
                    return false;
                }
                if self.start_capture() {
                    // Capture starts at the press, so the first words are not lost while the core answers.
                    self.armed_at = Some(now);
                    self.queue.clear();
                    if self.cfg.always_on {
                        // Whatever the microphone heard before the press is not part of the question.
                        for _ in 0..self.cfg.queue_frames.max(1) + 50 {
                            if !matches!(self.input.read_frame(Duration::ZERO), Ok(Some(_))) {
                                break;
                            }
                        }
                    }
                    true
                } else {
                    false
                }
            }
        }
    }

    fn begin_playback(&mut self) {
        // Half-duplex (REQ-VOI-012): the microphone is closed while the assistant speaks, or (always-on) kept
        // open and its audio thrown away so the engine never hears the reply.
        if self.cfg.always_on {
            self.armed_at = None;
            self.queue.clear();
            self.reset_wake();
        } else {
            self.stop_capture();
        }
        match self.output.begin() {
            Ok(()) => self.playing = true,
            Err(e) => {
                self.problem("speaker", &e);
                self.playing = false;
            }
        }
    }

    fn play(&mut self, pcm: &[i16]) {
        if !self.playing {
            return;
        }
        if let Err(e) = self.output.play(pcm) {
            self.problem("speaker", &e);
            self.output.stop();
            self.playback_over();
        }
    }

    fn end_playback(&mut self) {
        self.turns += 1;
        if self.playing {
            self.output.finish();
        }
    }

    fn playback_idle(&mut self) -> bool {
        if !self.playing {
            return true;
        }
        match self.output.poll_done() {
            Ok(false) => false,
            Ok(true) => {
                self.playback_over();
                true
            }
            Err(e) => {
                self.problem("speaker", &e);
                self.output.stop();
                self.playback_over();
                true
            }
        }
    }

    fn stop_playback(&mut self) {
        self.output.stop();
        self.playback_over();
    }

    fn indicator(&mut self, state: Indicator) {
        let led = match state {
            Indicator::Idle => Some(LedState::Idle),
            Indicator::Listening => Some(LedState::Listening),
            Indicator::Thinking => Some(LedState::Thinking),
            Indicator::Speaking => Some(LedState::Speaking),
            Indicator::Error => Some(LedState::Error),
            Indicator::Muted => Some(LedState::Muted),
            _ => None,
        };
        if let Some(l) = led {
            self.leds.show(l);
        }
        match state {
            Indicator::Idle => {
                let how = self.wake.as_ref().map_or_else(
                    || "press Enter to talk".to_owned(),
                    |w| format!("say \"{}\" or press Enter to talk", w.name()),
                );
                self.say(&format!("ready: {how}; q then Enter to quit"));
            }
            Indicator::Listening => self.say("listening: speak now"),
            Indicator::Thinking => self.say("thinking..."),
            Indicator::Speaking => self.say("speaking"),
            Indicator::Error => self.say("(nothing heard, or something went wrong)"),
            _ => {}
        }
    }

    fn set_volume(&mut self, level: u8) {
        self.output.set_volume(level);
    }

    fn set_mute(&mut self, muted: bool) {
        self.muted = muted;
        if muted {
            self.stop_capture();
            self.queue.clear();
            self.reset_wake();
            self.say("muted: the microphone is released");
        } else {
            // The engine starts afresh before any frame reaches it; the microphone reopens on the next pass.
            self.reset_wake();
            self.restart_at = None;
            self.say("unmuted");
        }
    }

    fn finished(&self) -> bool {
        self.quit || (self.cfg.max_turns.is_some_and(|m| self.turns >= m) && !self.playing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::{FRAME_SAMPLES, WavIn, WavOut};
    use crate::{Config, DEAD_AFTER, run};
    use clawdot_protocol::message::{AudioCodec, AudioFormat, Message, PROTOCOL_VERSION, Welcome};
    use clawdot_protocol::{Decoder, Limits, encode};
    use std::io::{Cursor, Read, Write};
    use std::net::{TcpListener, TcpStream};

    fn send(s: &mut TcpStream, m: &Message) {
        s.write_all(&encode(&m.to_frame()).unwrap()).unwrap();
    }

    /// A scripted core on its own thread. `script` gets the connection, a message reader, and runs after the
    /// satellite's hello was read and answered with a welcome.
    fn fake_core(
        script: impl FnOnce(&mut TcpStream, &mut dyn FnMut() -> Message) + Send + 'static,
    ) -> (u16, thread::JoinHandle<()>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let h = thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            s.set_read_timeout(Some(Duration::from_millis(20))).unwrap();
            let mut dec = Decoder::new(Limits::default());
            let mut reader = s.try_clone().unwrap();
            let mut next = move || -> Message {
                let end = Instant::now() + Duration::from_secs(10);
                loop {
                    if let Some(f) = dec.next_frame().unwrap() {
                        return Message::from_frame(f).unwrap();
                    }
                    assert!(Instant::now() < end, "timed out waiting for the satellite");
                    let mut buf = [0u8; 8192];
                    match reader.read(&mut buf) {
                        Ok(0) => panic!("the satellite hung up"),
                        Ok(n) => dec.push(&buf[..n]),
                        Err(_) => {}
                    }
                }
            };
            assert!(matches!(next(), Message::Hello(_)));
            send(
                &mut s,
                &Message::Welcome(Welcome {
                    protocol: PROTOCOL_VERSION,
                    session_id: "s".into(),
                    audio_out: AudioCodec::Pcm,
                }),
            );
            script(&mut s, &mut next);
        });
        (port, h)
    }

    fn cfg() -> Config {
        Config {
            device_id: "t".into(),
            token: None,
            wake_on_connect: false,
            dead_after: DEAD_AFTER,
        }
    }

    fn reply(s: &mut TcpStream, chunks: usize) {
        send(s, &Message::AudioStart(AudioFormat::PCM_16K_MONO));
        for _ in 0..chunks {
            send(
                s,
                &Message::AudioChunk {
                    format: AudioFormat::PCM_16K_MONO,
                    timestamp_ms: None,
                    payload: vec![3, 0, 4, 0],
                },
            );
        }
        send(s, &Message::AudioStop);
    }

    #[test]
    fn req_voi_017_a_press_wakes_the_core_and_the_turn_runs_from_a_recording_to_a_recording() {
        let (port, h) = fake_core(|s, next| {
            assert!(
                matches!(next(), Message::Wake(_)),
                "the press is a wake event"
            );
            send(s, &Message::StreamStart);
            let mut chunks = 0;
            while chunks < 5 {
                if let Message::AudioChunk {
                    payload, format, ..
                } = next()
                {
                    assert_eq!(payload.len(), 640, "20 ms frames");
                    assert_eq!(format, AudioFormat::PCM_16K_MONO);
                    if chunks == 0 {
                        assert_eq!(
                            &payload[..2],
                            &500i16.to_le_bytes(),
                            "the recording, from its start"
                        );
                    }
                    chunks += 1;
                }
            }
            send(s, &Message::StreamStop);
            reply(s, 2);
            while !matches!(next(), Message::PlaybackDone { .. }) {}
        });
        let mut dev = AudioDevice::new(
            WavIn::new(vec![500; 320_000], 0),
            WavOut::in_memory(),
            Immediate::default(),
            DeviceConfig {
                max_turns: Some(1),
                ..DeviceConfig::default()
            },
        );
        run(
            TcpStream::connect(("127.0.0.1", port)).unwrap(),
            &mut dev,
            &cfg(),
        )
        .unwrap();
        h.join().unwrap();
        assert_eq!(dev.output().samples, vec![3, 4, 3, 4]);
        assert_eq!(dev.turns(), 1);
        assert!(dev.problems.is_empty(), "{:?}", dev.problems);
    }

    /// An output that stays busy for a number of polls, to model a long reply.
    #[derive(Debug)]
    struct Slow {
        inner: WavOut,
        busy_polls: usize,
    }
    impl AudioOut for Slow {
        fn begin(&mut self) -> Result<(), AudioError> {
            self.inner.begin()
        }
        fn play(&mut self, pcm: &[i16]) -> Result<(), AudioError> {
            self.inner.play(pcm)
        }
        fn finish(&mut self) {}
        fn poll_done(&mut self) -> Result<bool, AudioError> {
            if self.busy_polls > 0 {
                self.busy_polls -= 1;
                thread::sleep(Duration::from_millis(5));
                return Ok(false);
            }
            self.inner.finish();
            self.inner.poll_done()
        }
        fn stop(&mut self) {
            self.busy_polls = 0;
            self.inner.stop();
        }
    }

    #[test]
    fn req_voi_018_pings_are_answered_and_playback_done_waits_while_a_reply_plays() {
        let (port, h) = fake_core(|s, next| {
            reply(s, 1);
            send(s, &Message::Ping);
            // The pong must arrive while the reply is still "playing", before playback-done.
            let mut saw_pong = false;
            loop {
                match next() {
                    Message::Pong => saw_pong = true,
                    Message::PlaybackDone { .. } => break,
                    _ => {}
                }
            }
            assert!(
                saw_pong,
                "playback-done came before the keepalive was answered"
            );
        });
        let mut dev = AudioDevice::new(
            WavIn::new(vec![], 0),
            Slow {
                inner: WavOut::in_memory(),
                busy_polls: 40,
            },
            crate::front::NoTrigger,
            DeviceConfig {
                max_turns: Some(1),
                ..DeviceConfig::default()
            },
        );
        run(
            TcpStream::connect(("127.0.0.1", port)).unwrap(),
            &mut dev,
            &cfg(),
        )
        .unwrap();
        h.join().unwrap();
        assert_eq!(dev.output().inner.samples, vec![3, 4]);
    }

    #[test]
    fn req_voi_017_a_press_while_speaking_or_muted_is_ignored() {
        let mut dev = AudioDevice::new(
            WavIn::new(vec![1; 640], 0),
            WavOut::in_memory(),
            Scripted(VecDeque::from([Press::Talk, Press::Talk, Press::Talk])),
            DeviceConfig::default(),
        );
        dev.begin_playback();
        assert!(!dev.poll_wake(), "speaking");
        dev.stop_playback();
        dev.set_mute(true);
        assert!(!dev.poll_wake(), "muted");
        dev.set_mute(false);
        assert!(dev.poll_wake(), "idle: the press wakes the core");
        assert!(dev.read_mic().is_some(), "capture began at the press");
    }

    #[test]
    fn req_voi_017_an_unanswered_press_does_not_leave_the_microphone_open() {
        let mut dev = AudioDevice::new(
            WavIn::new(vec![1; 6400], 0),
            WavOut::in_memory(),
            Scripted(VecDeque::from([Press::Talk])),
            DeviceConfig {
                arm_timeout: Duration::from_millis(30),
                ..DeviceConfig::default()
            },
        );
        assert!(dev.poll_wake());
        thread::sleep(Duration::from_millis(60));
        dev.poll_wake();
        assert!(dev.read_mic().is_none(), "capture was released");
    }

    #[test]
    fn req_voi_008_muting_closes_the_microphone_and_stream_start_does_not_reopen_it() {
        let mut dev = AudioDevice::new(
            WavIn::new(vec![1; 6400], 0),
            WavOut::in_memory(),
            Scripted::default(),
            DeviceConfig::default(),
        );
        dev.mic_active(true);
        assert!(dev.read_mic().is_some());
        dev.set_mute(true);
        assert!(dev.read_mic().is_none());
        dev.mic_active(true);
        assert!(dev.read_mic().is_none(), "muted stays closed");
    }

    #[test]
    fn req_voi_013_a_dead_microphone_is_reported_not_taken_for_silence() {
        let mut dev = AudioDevice::new(
            WavIn::new(vec![1; 320], 0),
            WavOut::in_memory(),
            NoTrigger,
            DeviceConfig::default(),
        );
        dev.mic_active(true);
        assert!(dev.read_mic().is_some());
        assert!(dev.read_mic().is_none());
        assert_eq!(dev.problems.len(), 1);
        assert!(dev.problems[0].contains("ended"), "{:?}", dev.problems);
    }

    #[test]
    fn req_voi_013_a_missing_microphone_program_is_reported_on_the_press() {
        use crate::command::{CommandIn, custom};
        let mic = CommandIn::new(
            custom("clawdot-no-such-recorder", &[], false),
            Duration::from_secs(1),
        );
        let mut dev = AudioDevice::new(
            mic,
            WavOut::in_memory(),
            Scripted(VecDeque::from([Press::Talk])),
            DeviceConfig::default(),
        );
        assert!(
            !dev.poll_wake(),
            "no wake is sent for a microphone that cannot start"
        );
        assert!(
            dev.problems[0].contains("clawdot-no-such-recorder"),
            "{:?}",
            dev.problems
        );
    }

    #[test]
    fn req_voi_017_line_triggers_talk_on_enter_and_quit_on_q_or_end_of_input() {
        let mut t = LineTrigger::new(Cursor::new("\nhello\nq\n"));
        let mut seen = Vec::new();
        let end = Instant::now() + Duration::from_secs(2);
        while seen.len() < 3 && Instant::now() < end {
            match t.poll() {
                Press::Nothing => thread::sleep(Duration::from_millis(5)),
                p => seen.push(p),
            }
        }
        assert_eq!(seen, [Press::Talk, Press::Talk, Press::Quit]);
        let mut eof = LineTrigger::new(Cursor::new(""));
        let end = Instant::now() + Duration::from_secs(2);
        let mut p = Press::Nothing;
        while p == Press::Nothing && Instant::now() < end {
            p = eof.poll();
        }
        assert_eq!(p, Press::Quit, "no terminal means leave, not spin");
    }

    #[derive(Debug, Clone, Default)]
    struct Recorder(std::sync::Arc<std::sync::Mutex<Vec<LedState>>>);
    impl Leds for Recorder {
        fn show(&mut self, s: LedState) {
            self.0.lock().unwrap().push(s);
        }
    }
    #[derive(Debug)]
    struct Pad(VecDeque<Button>);
    impl Buttons for Pad {
        fn poll(&mut self) -> Option<Button> {
            self.0.pop_front()
        }
    }

    #[test]
    fn req_voi_019_optional_lights_and_buttons_are_driven_when_present() {
        let rec = Recorder::default();
        let mut dev = AudioDevice::new(
            WavIn::new(vec![1; 640], 0),
            WavOut::in_memory(),
            NoTrigger,
            DeviceConfig::default(),
        )
        .with_leds(rec.clone())
        .with_buttons(Pad(VecDeque::from([Button::Talk, Button::Stop])));
        dev.indicator(Indicator::Listening);
        dev.indicator(Indicator::Thinking);
        dev.indicator(Indicator::Error);
        assert_eq!(
            *rec.0.lock().unwrap(),
            [LedState::Listening, LedState::Thinking, LedState::Error]
        );
        assert!(dev.poll_wake(), "the Talk button starts a turn");
        dev.begin_playback();
        assert!(!dev.poll_wake(), "the Stop button silences playback");
        assert!(dev.playback_idle(), "and playback is over");
    }

    // ------------------------------------------------------------------------ always-on capture (REQ-DEV-025..)

    use std::sync::{Arc, Mutex};

    #[derive(Debug, Default)]
    struct MicState {
        open: bool,
        starts: usize,
        frames: VecDeque<Vec<i16>>,
        fail: Option<AudioError>,
    }

    /// A microphone the test controls: frames to hand out, one failure to raise, how often it was started.
    #[derive(Debug, Clone, Default)]
    struct FakeMic(Arc<Mutex<MicState>>);

    impl FakeMic {
        fn push(&self, frame: Vec<i16>) {
            self.0.lock().unwrap().frames.push_back(frame);
        }
        fn push_quiet(&self, n: usize) {
            for _ in 0..n {
                self.push(vec![7; FRAME_SAMPLES]);
            }
        }
        fn fail_with(&self, e: AudioError) {
            self.0.lock().unwrap().fail = Some(e);
        }
        fn starts(&self) -> usize {
            self.0.lock().unwrap().starts
        }
        fn open(&self) -> bool {
            self.0.lock().unwrap().open
        }
    }

    impl AudioIn for FakeMic {
        fn start(&mut self) -> Result<(), AudioError> {
            let mut s = self.0.lock().unwrap();
            if let Some(AudioError::NotFound(m)) = &s.fail {
                return Err(AudioError::NotFound(m.clone()));
            }
            if !s.open {
                s.open = true;
                s.starts += 1;
            }
            Ok(())
        }
        fn read_frame(&mut self, _wait: Duration) -> Result<Option<Vec<i16>>, AudioError> {
            let mut s = self.0.lock().unwrap();
            if !s.open {
                return Err(AudioError::NotStarted);
            }
            if let Some(e) = s.fail.take() {
                return Err(e);
            }
            Ok(s.frames.pop_front())
        }
        fn stop(&mut self) {
            self.0.lock().unwrap().open = false;
        }
    }

    /// A clock the test moves by hand.
    #[derive(Debug, Clone)]
    struct TestClock(Arc<Mutex<Instant>>);

    impl TestClock {
        fn new() -> Self {
            Self(Arc::new(Mutex::new(Instant::now())))
        }
        fn advance(&self, by: Duration) {
            *self.0.lock().unwrap() += by;
        }
        fn reader(&self) -> impl Fn() -> Instant + Send + 'static {
            let c = Arc::clone(&self.0);
            move || *c.lock().unwrap()
        }
    }

    fn live(
        mic: &FakeMic,
        wake: &ScriptedWake,
        clock: &TestClock,
    ) -> AudioDevice<FakeMic, WavOut, NoTrigger> {
        AudioDevice::new(
            mic.clone(),
            WavOut::in_memory(),
            NoTrigger,
            DeviceConfig {
                always_on: true,
                ..DeviceConfig::default()
            },
        )
        .with_wake(wake.clone())
        .with_clock(clock.reader())
    }

    fn wake_stats(w: &ScriptedWake) -> WakeStats {
        w.stats.lock().unwrap().clone()
    }

    #[test]
    fn req_dev_025_the_microphone_opens_once_and_stays_open_between_turns() {
        let (mic, wake, clock) = (
            FakeMic::default(),
            ScriptedWake::default(),
            TestClock::new(),
        );
        let mut dev = live(&mic, &wake, &clock);
        assert!(!mic.open(), "nothing is opened before the first pass");
        assert!(!dev.poll_wake());
        assert!(mic.open() && dev.mic_open());
        // A whole turn: the core asks for audio, stops asking, a reply plays.
        dev.mic_active(true);
        dev.mic_active(false);
        dev.begin_playback();
        dev.end_playback();
        assert!(dev.playback_idle());
        dev.poll_wake();
        assert!(mic.open());
        assert_eq!(mic.starts(), 1, "one open for the whole session");
    }

    #[test]
    fn req_dev_025_a_wake_word_wakes_the_core_and_the_audio_after_it_is_kept() {
        let (mic, wake, clock) = (
            FakeMic::default(),
            ScriptedWake::default(),
            TestClock::new(),
        );
        let mut dev = live(&mic, &wake, &clock);
        dev.poll_wake();
        mic.push_quiet(3);
        mic.push(ScriptedWake::frame());
        for i in 0..5 {
            mic.push(vec![100 + i; FRAME_SAMPLES]);
        }
        assert!(dev.poll_wake(), "the wake word is heard in the first pass");
        assert_eq!(dev.wakes(), 1);
        // The next pass is before the core's stream-start: the speech after the wake word is kept, not lost.
        assert!(!dev.poll_wake());
        dev.mic_active(true);
        let first = dev.read_mic().unwrap();
        assert_eq!(
            first[0], 100,
            "the first frame after the wake word comes first"
        );
        let second = dev.read_mic().unwrap();
        assert_eq!(second[0], 101);
        assert!(dev.problems.is_empty(), "{:?}", dev.problems);
    }

    #[test]
    fn req_dev_025_the_kept_audio_is_bounded_and_dropped_when_the_core_never_asks() {
        let (mic, wake, clock) = (
            FakeMic::default(),
            ScriptedWake::default(),
            TestClock::new(),
        );
        let mut dev = live(&mic, &wake, &clock);
        dev.poll_wake();
        mic.push(ScriptedWake::frame());
        assert!(dev.poll_wake());
        for _ in 0..40 {
            mic.push_quiet(8);
            dev.poll_wake();
        }
        assert!(dev.queue.len() <= 100, "{} frames kept", dev.queue.len());
        // Three seconds pass and the core never asked: back to listening, nothing kept, microphone still open.
        clock.advance(Duration::from_secs(4));
        dev.poll_wake();
        assert!(dev.queue.is_empty());
        assert!(mic.open());
        mic.push(ScriptedWake::frame());
        assert!(dev.poll_wake(), "a later wake word works again");
        assert_eq!(dev.wakes(), 2);
    }

    #[test]
    fn req_dev_025_audio_that_is_not_the_wake_word_never_wakes_the_core() {
        let (mic, wake, clock) = (
            FakeMic::default(),
            ScriptedWake::default(),
            TestClock::new(),
        );
        let mut dev = live(&mic, &wake, &clock);
        dev.poll_wake();
        for _ in 0..10 {
            mic.push_quiet(8);
            assert!(!dev.poll_wake());
        }
        assert_eq!(dev.wakes(), 0);
        assert_eq!(wake_stats(&wake).fed, 80, "all of it went to the engine");
    }

    #[test]
    fn req_dev_026_the_wake_word_is_ignored_while_the_assistant_speaks_and_for_a_moment_after() {
        let (mic, wake, clock) = (
            FakeMic::default(),
            ScriptedWake::default(),
            TestClock::new(),
        );
        let mut dev = live(&mic, &wake, &clock);
        dev.poll_wake();
        dev.begin_playback();
        let fed_before = wake_stats(&wake).fed;
        mic.push(ScriptedWake::frame());
        mic.push_quiet(2);
        assert!(!dev.poll_wake(), "a wake word during the reply is ignored");
        assert_eq!(
            wake_stats(&wake).fed,
            fed_before,
            "the engine is not fed while speaking"
        );
        assert!(mic.open(), "the microphone stays open");
        dev.end_playback();
        assert!(dev.playback_idle());
        // The reply just ended: the echo of the room is still in the air.
        let resets = wake_stats(&wake).resets;
        mic.push(ScriptedWake::frame());
        assert!(!dev.poll_wake(), "inside the hold-off");
        assert!(resets >= 1);
        clock.advance(Duration::from_millis(600));
        mic.push(ScriptedWake::frame());
        assert!(dev.poll_wake(), "after the hold-off the wake word works");
    }

    #[test]
    fn req_dev_026_the_engine_is_reset_when_the_core_stops_asking_for_speech() {
        let (mic, wake, clock) = (
            FakeMic::default(),
            ScriptedWake::default(),
            TestClock::new(),
        );
        let mut dev = live(&mic, &wake, &clock);
        dev.poll_wake();
        dev.mic_active(true);
        let before = wake_stats(&wake).resets;
        mic.push(ScriptedWake::frame());
        assert!(
            !dev.poll_wake(),
            "while the core listens the engine is not fed"
        );
        assert_eq!(wake_stats(&wake).fed, 0);
        dev.mic_active(false);
        assert_eq!(wake_stats(&wake).resets, before + 1);
    }

    #[test]
    fn req_dev_027_mute_releases_the_microphone_and_unmute_reopens_it_with_a_fresh_engine() {
        let (mic, wake, clock) = (
            FakeMic::default(),
            ScriptedWake::default(),
            TestClock::new(),
        );
        let mut dev = live(&mic, &wake, &clock);
        dev.poll_wake();
        assert!(mic.open());
        dev.set_mute(true);
        assert!(
            !mic.open(),
            "mute closes the device, it does not just stop reading"
        );
        mic.push(ScriptedWake::frame());
        assert!(!dev.poll_wake());
        assert!(!mic.open(), "still closed while muted");
        assert_eq!(wake_stats(&wake).fed, 0, "a muted engine is not fed");
        let resets = wake_stats(&wake).resets;
        dev.set_mute(false);
        assert!(wake_stats(&wake).resets > resets, "reset before any frame");
        dev.poll_wake();
        assert!(mic.open());
        assert_eq!(mic.starts(), 2);
    }

    #[test]
    fn req_dev_028_a_stalled_microphone_is_reported_and_reopened_with_a_growing_wait() {
        let (mic, wake, clock) = (
            FakeMic::default(),
            ScriptedWake::default(),
            TestClock::new(),
        );
        let mut dev = live(&mic, &wake, &clock);
        dev.poll_wake();
        mic.fail_with(AudioError::Stalled("no audio for 5 s".into()));
        dev.poll_wake();
        assert!(!mic.open(), "released");
        assert_eq!(dev.problems.len(), 1, "{:?}", dev.problems);
        assert!(dev.problems[0].contains("stalled"));
        clock.advance(Duration::from_millis(500));
        dev.poll_wake();
        assert!(!mic.open(), "not before one second");
        clock.advance(Duration::from_millis(600));
        dev.poll_wake();
        assert!(mic.open(), "reopened");
        assert_eq!(mic.starts(), 2);
        // A second fault before any good frame waits twice as long.
        mic.fail_with(AudioError::Stalled("again".into()));
        dev.poll_wake();
        clock.advance(Duration::from_millis(1_500));
        dev.poll_wake();
        assert!(!mic.open(), "two seconds this time");
        clock.advance(Duration::from_millis(600));
        dev.poll_wake();
        assert!(mic.open());
        // A good frame resets the wait to one second.
        mic.push_quiet(1);
        dev.poll_wake();
        mic.fail_with(AudioError::Eof);
        dev.poll_wake();
        clock.advance(Duration::from_millis(1_100));
        dev.poll_wake();
        assert!(mic.open());
    }

    #[test]
    fn req_dev_028_a_missing_capture_program_is_reported_once_and_not_retried() {
        let (mic, wake, clock) = (
            FakeMic::default(),
            ScriptedWake::default(),
            TestClock::new(),
        );
        mic.fail_with(AudioError::NotFound("`ffmpeg` was not found".into()));
        let mut dev = live(&mic, &wake, &clock);
        for _ in 0..5 {
            dev.poll_wake();
            clock.advance(Duration::from_secs(30));
        }
        assert_eq!(dev.problems.len(), 1, "{:?}", dev.problems);
        assert_eq!(mic.starts(), 0);
    }

    #[test]
    fn req_dev_025_push_to_talk_works_without_an_engine_and_loses_no_first_word() {
        let (mic, clock) = (FakeMic::default(), TestClock::new());
        let mut dev = AudioDevice::new(
            mic.clone(),
            WavOut::in_memory(),
            Scripted(VecDeque::from([Press::Nothing, Press::Talk])),
            DeviceConfig {
                always_on: true,
                ..DeviceConfig::default()
            },
        )
        .with_clock(clock.reader());
        assert!(!dev.poll_wake());
        mic.push(vec![1; FRAME_SAMPLES]);
        assert!(dev.poll_wake(), "the key press wakes the core");
        mic.push(vec![2; FRAME_SAMPLES]);
        mic.push(vec![3; FRAME_SAMPLES]);
        dev.poll_wake();
        dev.mic_active(true);
        assert_eq!(
            dev.read_mic().unwrap()[0],
            2,
            "audio captured before stream-start is sent first"
        );
        assert_eq!(dev.read_mic().unwrap()[0], 3);
    }

    #[test]
    fn frames_are_one_burst_of_four() {
        let dev = AudioDevice::new(
            WavIn::new(vec![], 0),
            WavOut::in_memory(),
            NoTrigger,
            DeviceConfig::default(),
        );
        assert_eq!(dev.mic_burst(), 4);
        assert_eq!(FRAME_SAMPLES, 320);
    }
}
