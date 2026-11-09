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

/// Settings for [`AudioDevice`].
#[derive(Debug, Clone, Copy)]
pub struct DeviceConfig {
    /// Leave after this many replies; `None` runs until the trigger says quit.
    pub max_turns: Option<usize>,
    /// Stop capturing if the core does not ask for audio this long after a press.
    pub arm_timeout: Duration,
    /// Print what is happening (one short line per state) to standard error.
    pub verbose: bool,
}

impl Default for DeviceConfig {
    fn default() -> Self {
        Self {
            max_turns: None,
            arm_timeout: Duration::from_secs(3),
            verbose: false,
        }
    }
}

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
            problems: Vec::new(),
        }
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
        match self.input.start() {
            Ok(()) => {
                self.capturing = true;
                true
            }
            Err(e) => {
                self.problem("microphone", &e);
                false
            }
        }
    }

    fn stop_capture(&mut self) {
        self.capturing = false;
        self.armed_at = None;
        self.input.stop();
    }
}

impl<I: AudioIn, O: AudioOut, T: Trigger> Device for AudioDevice<I, O, T> {
    fn read_mic(&mut self) -> Option<Vec<i16>> {
        if !self.capturing {
            return None;
        }
        match self.input.read_frame(Duration::ZERO) {
            Ok(frame) => frame,
            Err(e) => {
                let what = if e == AudioError::Eof {
                    "microphone source ended"
                } else {
                    "microphone"
                };
                self.problem(what, &e);
                // Release it so the next turn starts afresh instead of reading a dead source.
                self.stop_capture();
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
        } else {
            self.stop_capture();
        }
    }

    fn mic_burst(&self) -> usize {
        BURST
    }

    fn poll_wake(&mut self) -> bool {
        if let Some(at) = self.armed_at
            && !self.listening
            && at.elapsed() >= self.cfg.arm_timeout
        {
            // The core never asked for audio (busy, or muted from its side): do not keep the microphone open.
            self.stop_capture();
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
            Press::Nothing => false,
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
                    self.armed_at = Some(Instant::now());
                    true
                } else {
                    false
                }
            }
        }
    }

    fn begin_playback(&mut self) {
        // Half-duplex (REQ-VOI-012): the microphone is closed while the assistant speaks.
        self.stop_capture();
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
            self.playing = false;
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
                self.playing = false;
                true
            }
            Err(e) => {
                self.problem("speaker", &e);
                self.output.stop();
                self.playing = false;
                true
            }
        }
    }

    fn stop_playback(&mut self) {
        self.output.stop();
        self.playing = false;
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
            Indicator::Idle => self.say("ready: press Enter to talk, q then Enter to quit"),
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
