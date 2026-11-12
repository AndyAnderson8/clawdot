//! The voice dialog: who is allowed to listen when, and how an utterance becomes a spoken reply
//! (REQ-VOI-002, 003, 004, 006, 007, 008, 010, 012).
//!
//! [`Dialog`] is a pure state machine. Hand it wake events, microphone audio, and the clock; it tells you which
//! cues to play and when an utterance is complete. It enforces the rules that must not depend on a particular
//! satellite: mute really disables capture, audio is ignored while the assistant is thinking or speaking
//! (half-duplex), the follow-up window re-opens listening without the wake word, and the "thinking" earcon
//! plays only if the reply is slow.
//!
//! [`voice_turn`] turns one captured utterance into text, asks a responder for a reply, and synthesises it one
//! sentence at a time so playback can start before the whole reply is spoken. Speech-to-text and text-to-speech
//! are traits; the providers live elsewhere.

use crate::llm::SentenceSplitter;
use crate::vad::{Config as VadConfig, Endpointer, Event};
use std::future::Future;

/// Something to show or sound on the satellite (REQ-VOI-006).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cue {
    /// The wake word was heard.
    Wake,
    /// The assistant is listening (also used when the follow-up window opens).
    Listening,
    /// The reply is taking a while.
    Thinking,
    /// Nothing was heard, or something went wrong.
    Error,
    /// The microphone was muted.
    Muted,
    /// The microphone was unmuted.
    Unmuted,
}

/// What the satellite should do after feeding the dialog something.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// Nothing.
    Nothing,
    /// Play or show a cue.
    Cue(Cue),
    /// A complete utterance, ready for speech-to-text.
    Utterance(Vec<i16>),
    /// The short capture that a wake word opened while a turn was running has ended (REQ-VOI-020). The audio
    /// is to be checked against the stop phrases only; it is empty when nothing was said.
    Interrupt(Vec<i16>),
}

/// Where the dialog is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Waiting for the wake word.
    Idle,
    /// Capturing speech.
    Listening,
    /// Working on a reply.
    Thinking,
    /// Playing the reply.
    Speaking,
}

/// Dialog settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Config {
    /// Seconds after a reply during which listening re-opens without the wake word; 0 turns it off.
    pub follow_up_secs: u64,
    /// Seconds the window stays open after a question or a held action (REQ-AST-021). Never shorter than
    /// `follow_up_secs`, and unused when follow-ups are off.
    pub follow_up_question_secs: u64,
    /// Whether a wake word heard while a turn is running (and nothing is playing) opens a short capture that
    /// is checked for a stop phrase (REQ-VOI-020).
    pub interrupt_while_working: bool,
    /// Milliseconds without a reply before the thinking cue plays.
    pub thinking_delay_ms: u64,
    /// Endpointer tuning.
    pub vad: VadConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            follow_up_secs: 8,
            follow_up_question_secs: 15,
            interrupt_while_working: true,
            thinking_delay_ms: 1_000,
            vad: VadConfig::default(),
        }
    }
}

/// The dialog state machine.
#[derive(Debug, Clone)]
pub struct Dialog {
    cfg: Config,
    phase: Phase,
    muted: bool,
    endpointer: Endpointer,
    thinking_since: Option<u64>,
    thinking_cued: bool,
    /// A capture opened by a wake word while a turn runs.
    interrupt: Option<Endpointer>,
    /// Assistant speech or a tone is playing while the turn still runs (progress speech).
    output_playing: bool,
    /// The reply ends with a question: the follow-up window is longer.
    expects_answer: bool,
}

impl Dialog {
    /// A dialog, waiting for the wake word.
    #[must_use]
    pub fn new(cfg: Config) -> Self {
        Self {
            cfg,
            phase: Phase::Idle,
            muted: false,
            endpointer: Endpointer::new(cfg.vad),
            thinking_since: None,
            thinking_cued: false,
            interrupt: None,
            output_playing: false,
            expects_answer: false,
        }
    }

    /// Where the dialog is.
    #[must_use]
    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// Whether capture is disabled.
    #[must_use]
    pub fn is_muted(&self) -> bool {
        self.muted
    }

    /// Whether the satellite should send microphone audio at all right now (REQ-VOI-008, REQ-VOI-012).
    #[must_use]
    pub fn wants_audio(&self) -> bool {
        !self.muted && (self.phase == Phase::Listening || self.interrupt.is_some())
    }

    /// Whether a stop capture is open (REQ-VOI-020).
    #[must_use]
    pub fn is_interrupting(&self) -> bool {
        self.interrupt.is_some()
    }

    /// Tell the dialog that progress speech started or stopped playing while the turn runs. A wake word heard
    /// meanwhile is more likely the assistant's own voice than the user's, so it is ignored (REQ-VOI-012).
    pub fn set_output_playing(&mut self, playing: bool) {
        self.output_playing = playing;
        if playing {
            self.interrupt = None;
        }
    }

    /// The next reply ends with a question, so the follow-up window after it is longer (REQ-AST-021).
    pub fn expect_answer(&mut self) {
        self.expects_answer = true;
    }

    /// Mute or unmute. Muting abandons whatever was being captured.
    pub fn set_muted(&mut self, muted: bool) -> Step {
        if muted == self.muted {
            return Step::Nothing;
        }
        self.muted = muted;
        if muted {
            self.endpointer.reset();
            self.interrupt = None;
            if self.phase == Phase::Listening {
                self.phase = Phase::Idle;
            }
            Step::Cue(Cue::Muted)
        } else {
            Step::Cue(Cue::Unmuted)
        }
    }

    fn start_listening(&mut self) {
        self.endpointer.reset();
        self.phase = Phase::Listening;
    }

    /// The wake word was heard. Ignored while muted or speaking (half-duplex). While a turn is running and
    /// nothing is playing it opens a short capture that is checked for a stop phrase only (REQ-VOI-020).
    pub fn on_wake(&mut self) -> Step {
        if self.muted {
            return Step::Nothing;
        }
        if self.phase == Phase::Thinking && self.cfg.interrupt_while_working && !self.output_playing
        {
            if self.interrupt.is_none() {
                let mut vad = self.cfg.vad;
                vad.no_speech_ms = 4_000;
                self.interrupt = Some(Endpointer::new(vad));
                return Step::Cue(Cue::Wake);
            }
            return Step::Nothing;
        }
        if matches!(self.phase, Phase::Thinking | Phase::Speaking) {
            return Step::Nothing;
        }
        self.start_listening();
        Step::Cue(Cue::Wake)
    }

    /// Microphone audio. Ignored unless the dialog is listening (or a stop capture is open).
    pub fn on_audio(&mut self, now_ms: u64, pcm: &[i16]) -> Step {
        if !self.wants_audio() {
            return Step::Nothing;
        }
        if let Some(ep) = self.interrupt.as_mut() {
            return match ep.feed(pcm) {
                Event::Ended { pcm, .. } => {
                    self.interrupt = None;
                    Step::Interrupt(pcm)
                }
                Event::NoSpeech => {
                    self.interrupt = None;
                    Step::Interrupt(Vec::new())
                }
                Event::Nothing | Event::SpeechStarted => Step::Nothing,
            };
        }
        match self.endpointer.feed(pcm) {
            Event::Ended { pcm, .. } => {
                self.phase = Phase::Thinking;
                self.thinking_since = Some(now_ms);
                self.thinking_cued = false;
                Step::Utterance(pcm)
            }
            Event::NoSpeech => {
                self.phase = Phase::Idle;
                Step::Cue(Cue::Error)
            }
            Event::Nothing | Event::SpeechStarted => Step::Nothing,
        }
    }

    /// Call regularly. Returns the thinking cue once if the reply is slow (REQ-VOI-010).
    pub fn tick(&mut self, now_ms: u64) -> Step {
        if self.phase == Phase::Thinking && !self.thinking_cued {
            if let Some(t) = self.thinking_since {
                if now_ms.saturating_sub(t) >= self.cfg.thinking_delay_ms {
                    self.thinking_cued = true;
                    return Step::Cue(Cue::Thinking);
                }
            }
        }
        Step::Nothing
    }

    /// The reply has begun playing, so the thinking cue must stop.
    pub fn on_reply_started(&mut self) {
        self.phase = Phase::Speaking;
        self.thinking_since = None;
        self.interrupt = None;
        self.output_playing = false;
    }

    /// A command was carried out and there is nothing to say; go idle without an error cue.
    pub fn on_reply_silent(&mut self) -> Step {
        self.phase = Phase::Idle;
        self.thinking_since = None;
        self.interrupt = None;
        self.expects_answer = false;
        Step::Nothing
    }

    /// The reply failed or there is nothing to say.
    pub fn on_reply_failed(&mut self) -> Step {
        self.phase = Phase::Idle;
        self.thinking_since = None;
        self.interrupt = None;
        self.expects_answer = false;
        Step::Cue(Cue::Error)
    }

    /// Playback ended. Opens the follow-up window if configured, longer after a question (REQ-VOI-007,
    /// REQ-AST-021).
    pub fn on_playback_done(&mut self) -> Step {
        self.thinking_since = None;
        let asked = std::mem::take(&mut self.expects_answer);
        if self.muted || self.cfg.follow_up_secs == 0 {
            self.phase = Phase::Idle;
            return Step::Nothing;
        }
        let secs = if asked {
            self.cfg
                .follow_up_question_secs
                .max(self.cfg.follow_up_secs)
        } else {
            self.cfg.follow_up_secs
        };
        let mut vad = self.cfg.vad;
        vad.no_speech_ms = usize::try_from(secs.saturating_mul(1000)).unwrap_or(usize::MAX);
        self.endpointer = Endpointer::new(vad);
        self.phase = Phase::Listening;
        Step::Cue(Cue::Listening)
    }
}

/// Speech to text. A provider turns one utterance into a transcript.
pub trait Transcriber {
    /// Transcribe 16 kHz mono audio.
    ///
    /// # Errors
    /// A message describing why it failed.
    fn transcribe(&self, pcm: &[i16]) -> impl Future<Output = Result<String, String>> + Send;
}

/// Text to speech. A provider turns one sentence into audio.
pub trait Synthesizer {
    /// Synthesise 16 kHz mono audio.
    ///
    /// # Errors
    /// A message describing why it failed.
    fn synthesize(&self, text: &str) -> impl Future<Output = Result<Vec<i16>, String>> + Send;

    /// Synthesise with `voice` for this one call, leaving the voice every other line uses alone (a portal
    /// preview). A provider with no choice of voice speaks as usual.
    ///
    /// # Errors
    /// A message describing why it failed.
    fn synthesize_voice(
        &self,
        text: &str,
        voice: &str,
    ) -> impl Future<Output = Result<Vec<i16>, String>> + Send {
        let _ = voice;
        self.synthesize(text)
    }
}

/// The result of one voice turn.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VoiceOutcome {
    /// What the user said, if it could be understood.
    pub transcript: Option<String>,
    /// What the assistant says back (empty when nothing should be spoken).
    pub reply: String,
    /// The reply as audio, one entry per sentence, in order.
    pub audio: Vec<Vec<i16>>,
    /// Problems worth logging; never contains audio or provider credentials.
    pub problems: Vec<String>,
}

/// Spoken when speech recognition fails outright.
pub const COULD_NOT_UNDERSTAND: &str = "Sorry, I couldn't make that out.";

/// Turn one utterance into a spoken reply: transcribe, ask `respond`, then synthesise sentence by sentence.
///
/// An empty transcript produces no reply (the satellite shows its error cue). A transcription failure speaks a
/// short apology without calling `respond`. A synthesis failure keeps the text reply and records the problem.
pub async fn voice_turn<T, S, F, Fut>(stt: &T, tts: &S, pcm: &[i16], respond: F) -> VoiceOutcome
where
    T: Transcriber,
    S: Synthesizer,
    F: FnOnce(String) -> Fut,
    Fut: Future<Output = String>,
{
    let mut out = VoiceOutcome::default();
    match stt.transcribe(pcm).await {
        Ok(t) if t.trim().is_empty() => return out,
        Ok(t) => {
            let t = t.trim().to_owned();
            out.reply = respond(t.clone()).await;
            out.transcript = Some(t);
        }
        Err(e) => {
            out.problems.push(format!("speech recognition: {e}"));
            out.reply = COULD_NOT_UNDERSTAND.into();
        }
    }
    out.audio = speak_text(tts, &out.reply, &mut out.problems).await;
    out
}

/// Synthesise `text` one sentence at a time. Stops at the first failure, keeping what was already made and
/// recording the problem.
pub async fn speak_text<S: Synthesizer>(
    tts: &S,
    text: &str,
    problems: &mut Vec<String>,
) -> Vec<Vec<i16>> {
    speak_text_as(tts, text, None, problems).await
}

/// Like [`speak_text`], in `voice` for these sentences only when one is given.
pub async fn speak_text_as<S: Synthesizer>(
    tts: &S,
    text: &str,
    voice: Option<&str>,
    problems: &mut Vec<String>,
) -> Vec<Vec<i16>> {
    let mut splitter = SentenceSplitter::default();
    let mut sentences = splitter.push(text);
    sentences.extend(splitter.flush());
    let mut audio = Vec::new();
    for s in sentences {
        let made = match voice {
            Some(v) => tts.synthesize_voice(&s, v).await,
            None => tts.synthesize(&s).await,
        };
        match made {
            Ok(a) => audio.push(a),
            Err(e) => {
                problems.push(format!("speech synthesis: {e}"));
                break;
            }
        }
    }
    audio
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vad::tests::{silence, tone};
    use std::sync::Mutex;

    fn speak_into(d: &mut Dialog, now: u64) -> Step {
        let mut audio = silence(200);
        audio.extend(tone(3000.0, 600));
        audio.extend(silence(1000));
        let mut last = Step::Nothing;
        for chunk in audio.chunks(320) {
            match d.on_audio(now, chunk) {
                Step::Nothing => {}
                s => last = s,
            }
        }
        last
    }

    #[test]
    fn wake_listen_utterance_think_speak_idle() {
        let mut d = Dialog::new(Config {
            follow_up_secs: 0,
            ..Config::default()
        });
        assert_eq!(d.phase(), Phase::Idle);
        assert!(!d.wants_audio());
        assert_eq!(d.on_wake(), Step::Cue(Cue::Wake));
        assert!(d.wants_audio());
        assert!(matches!(speak_into(&mut d, 1_000), Step::Utterance(_)));
        assert_eq!(d.phase(), Phase::Thinking);
        d.on_reply_started();
        assert_eq!(d.phase(), Phase::Speaking);
        assert_eq!(d.on_playback_done(), Step::Nothing);
        assert_eq!(d.phase(), Phase::Idle);
    }

    #[test]
    fn mute_disables_capture_and_is_observable() {
        let mut d = Dialog::new(Config::default());
        assert_eq!(d.set_muted(true), Step::Cue(Cue::Muted));
        assert!(d.is_muted());
        assert_eq!(
            d.on_wake(),
            Step::Nothing,
            "the wake word does nothing while muted"
        );
        assert!(!d.wants_audio());
        assert_eq!(d.on_audio(0, &tone(3000.0, 500)), Step::Nothing);
        assert_eq!(d.set_muted(true), Step::Nothing, "no repeated cue");
        assert_eq!(d.set_muted(false), Step::Cue(Cue::Unmuted));
        assert_eq!(d.on_wake(), Step::Cue(Cue::Wake));
        // Muting mid-capture abandons it.
        assert_eq!(d.set_muted(true), Step::Cue(Cue::Muted));
        assert_eq!(d.phase(), Phase::Idle);
    }

    #[test]
    fn audio_and_wake_are_ignored_while_thinking_or_speaking() {
        // With the stop capture off (REQ-VOI-020), half-duplex is as strict as REQ-VOI-012 says.
        let mut d = Dialog::new(Config {
            follow_up_secs: 0,
            interrupt_while_working: false,
            ..Config::default()
        });
        d.on_wake();
        assert!(matches!(speak_into(&mut d, 0), Step::Utterance(_)));
        assert_eq!(d.on_wake(), Step::Nothing);
        assert_eq!(d.on_audio(0, &tone(3000.0, 300)), Step::Nothing);
        d.on_reply_started();
        assert_eq!(d.on_wake(), Step::Nothing);
        assert!(
            !d.wants_audio(),
            "half-duplex: the microphone is not processed while speaking"
        );
    }

    #[test]
    fn the_thinking_cue_is_late_once_and_cancelled_by_the_reply() {
        let mut d = Dialog::new(Config {
            follow_up_secs: 0,
            ..Config::default()
        });
        d.on_wake();
        assert!(matches!(speak_into(&mut d, 5_000), Step::Utterance(_)));
        assert_eq!(d.tick(5_500), Step::Nothing);
        assert_eq!(d.tick(6_000), Step::Cue(Cue::Thinking));
        assert_eq!(d.tick(7_000), Step::Nothing, "only once");
        d.on_reply_started();
        assert_eq!(d.tick(9_000), Step::Nothing);

        // A fast reply never plays the cue.
        let mut d = Dialog::new(Config {
            follow_up_secs: 0,
            ..Config::default()
        });
        d.on_wake();
        assert!(matches!(speak_into(&mut d, 0), Step::Utterance(_)));
        d.on_reply_started();
        assert_eq!(d.tick(2_000), Step::Nothing);
    }

    #[test]
    fn the_follow_up_window_reopens_listening_without_the_wake_word() {
        let mut d = Dialog::new(Config::default());
        d.on_wake();
        assert!(matches!(speak_into(&mut d, 0), Step::Utterance(_)));
        d.on_reply_started();
        assert_eq!(d.on_playback_done(), Step::Cue(Cue::Listening));
        assert_eq!(d.phase(), Phase::Listening);
        assert!(
            matches!(speak_into(&mut d, 0), Step::Utterance(_)),
            "a follow-up is heard"
        );
        // If nobody speaks within the window the dialog gives up quietly with the error cue.
        d.on_reply_started();
        d.on_playback_done();
        let mut result = Step::Nothing;
        for chunk in silence(9_000).chunks(320) {
            match d.on_audio(0, chunk) {
                Step::Nothing => {}
                s => result = s,
            }
        }
        assert_eq!(result, Step::Cue(Cue::Error));
        assert_eq!(d.phase(), Phase::Idle);
    }

    #[test]
    fn no_follow_up_while_muted() {
        let mut d = Dialog::new(Config::default());
        d.on_wake();
        assert!(matches!(speak_into(&mut d, 0), Step::Utterance(_)));
        d.on_reply_started();
        d.set_muted(true);
        assert_eq!(d.on_playback_done(), Step::Nothing);
        assert_eq!(d.phase(), Phase::Idle);
    }

    /// A dialog that is thinking about an utterance it has just heard.
    fn thinking() -> Dialog {
        let mut d = Dialog::new(Config::default());
        d.on_wake();
        assert!(matches!(speak_into(&mut d, 0), Step::Utterance(_)));
        assert_eq!(d.phase(), Phase::Thinking);
        d
    }

    fn short_speech() -> Vec<i16> {
        let mut audio = tone(3000.0, 500);
        audio.extend(silence(1000));
        audio
    }

    fn drain(d: &mut Dialog, audio: &[i16]) -> Step {
        let mut got = Step::Nothing;
        for chunk in audio.chunks(320) {
            match d.on_audio(0, chunk) {
                Step::Nothing => {}
                s => got = s,
            }
        }
        got
    }

    // REQ-VOI-020: while a turn runs and nothing is playing, a wake word opens a stop capture and nothing else.
    #[test]
    fn a_wake_word_while_thinking_opens_a_stop_capture_and_ordinary_audio_is_still_ignored() {
        let mut d = thinking();
        assert_eq!(
            d.on_audio(0, &tone(3000.0, 300)),
            Step::Nothing,
            "no wake word, no listening"
        );
        assert!(!d.wants_audio());
        assert_eq!(d.on_wake(), Step::Cue(Cue::Wake));
        assert!(d.wants_audio() && d.is_interrupting());
        assert_eq!(d.phase(), Phase::Thinking, "the turn is still running");
        let got = drain(&mut d, &short_speech());
        assert!(
            matches!(&got, Step::Interrupt(p) if !p.is_empty()),
            "{got:?}"
        );
        assert!(!d.is_interrupting() && !d.wants_audio());
        assert_eq!(d.phase(), Phase::Thinking);
    }

    #[test]
    fn a_stop_capture_with_no_speech_ends_empty_and_a_second_wake_does_not_restart_it() {
        let mut d = thinking();
        d.on_wake();
        assert_eq!(d.on_wake(), Step::Nothing);
        assert_eq!(drain(&mut d, &silence(5_000)), Step::Interrupt(Vec::new()));
        assert_eq!(d.phase(), Phase::Thinking);
    }

    #[test]
    fn half_duplex_still_holds_while_speech_plays_and_the_switch_turns_it_off() {
        // Progress speech is playing: the wake word is ignored, as REQ-VOI-012 says.
        let mut d = thinking();
        d.set_output_playing(true);
        assert_eq!(d.on_wake(), Step::Nothing);
        assert!(!d.wants_audio());
        d.set_output_playing(false);
        assert_eq!(d.on_wake(), Step::Cue(Cue::Wake));
        // Playing speech closes a capture that was open.
        d.set_output_playing(true);
        assert!(!d.is_interrupting());
        // While speaking the final reply, nothing changes.
        let mut d = thinking();
        d.on_reply_started();
        assert_eq!(d.on_wake(), Step::Nothing);
        // The switch.
        let mut d = Dialog::new(Config {
            interrupt_while_working: false,
            ..Config::default()
        });
        d.on_wake();
        assert!(matches!(speak_into(&mut d, 0), Step::Utterance(_)));
        assert_eq!(d.on_wake(), Step::Nothing);
        assert!(!d.wants_audio());
    }

    #[test]
    fn muting_or_finishing_the_reply_abandons_a_stop_capture() {
        let mut d = thinking();
        d.on_wake();
        d.set_muted(true);
        assert!(!d.is_interrupting());
        let mut d = thinking();
        d.on_wake();
        d.on_reply_started();
        assert!(!d.is_interrupting());
    }

    // REQ-AST-021: after a question the follow-up window is longer.
    #[test]
    fn the_follow_up_window_after_a_question_is_longer_once() {
        let heard_after = |asked: bool, secs: usize| {
            let mut d = thinking();
            if asked {
                d.expect_answer();
            }
            d.on_reply_started();
            assert_eq!(d.on_playback_done(), Step::Cue(Cue::Listening));
            drain(&mut d, &silence(secs * 1000))
        };
        assert_eq!(
            heard_after(false, 9),
            Step::Cue(Cue::Error),
            "8 s is the normal window"
        );
        assert_eq!(
            heard_after(true, 9),
            Step::Nothing,
            "still open after 9 s when a question was asked"
        );
        assert_eq!(
            heard_after(true, 16),
            Step::Cue(Cue::Error),
            "but not forever"
        );
        // The longer window applies once only.
        let mut d = thinking();
        d.expect_answer();
        d.on_reply_started();
        d.on_playback_done();
        d.on_reply_started();
        d.on_playback_done();
        assert_eq!(drain(&mut d, &silence(9_000)), Step::Cue(Cue::Error));
    }

    struct Heard(Result<&'static str, &'static str>);
    impl Transcriber for Heard {
        async fn transcribe(&self, _: &[i16]) -> Result<String, String> {
            self.0.map(str::to_owned).map_err(str::to_owned)
        }
    }

    struct Voice {
        fail_on: Option<&'static str>,
        spoken: Mutex<Vec<String>>,
    }
    impl Synthesizer for Voice {
        async fn synthesize(&self, text: &str) -> Result<Vec<i16>, String> {
            if self.fail_on.is_some_and(|f| text.contains(f)) {
                return Err("voice unavailable".into());
            }
            self.spoken.lock().unwrap().push(text.to_owned());
            Ok(vec![1; text.len()])
        }
    }

    fn voice(fail_on: Option<&'static str>) -> Voice {
        Voice {
            fail_on,
            spoken: Mutex::new(Vec::new()),
        }
    }

    #[tokio::test]
    async fn a_turn_transcribes_responds_and_speaks_sentence_by_sentence() {
        let tts = voice(None);
        let out = voice_turn(
            &Heard(Ok("  what time is it ")),
            &tts,
            &[0; 10],
            |t| async move {
                assert_eq!(t, "what time is it", "the transcript is trimmed");
                "It is noon. Lunch is soon!".to_owned()
            },
        )
        .await;
        assert_eq!(out.transcript.as_deref(), Some("what time is it"));
        assert_eq!(out.reply, "It is noon. Lunch is soon!");
        assert_eq!(
            *tts.spoken.lock().unwrap(),
            vec!["It is noon.", "Lunch is soon!"]
        );
        assert_eq!(out.audio.len(), 2);
        assert!(out.problems.is_empty());
    }

    #[tokio::test]
    async fn nothing_heard_means_nothing_said() {
        let tts = voice(None);
        let out = voice_turn(&Heard(Ok("   ")), &tts, &[], |_| async {
            panic!("must not respond")
        })
        .await;
        assert_eq!(out, VoiceOutcome::default());
    }

    #[tokio::test]
    async fn a_recognition_failure_apologises_without_asking_the_assistant() {
        let tts = voice(None);
        let out = voice_turn(&Heard(Err("timeout")), &tts, &[], |_| async {
            panic!("must not respond")
        })
        .await;
        assert_eq!(out.reply, COULD_NOT_UNDERSTAND);
        assert!(out.transcript.is_none());
        assert!(out.problems[0].contains("speech recognition"));
        assert_eq!(out.audio.len(), 1);
    }

    #[tokio::test]
    async fn a_synthesis_failure_keeps_the_text_and_records_the_problem() {
        let tts = voice(Some("Second"));
        let out = voice_turn(&Heard(Ok("hi")), &tts, &[], |_| async {
            "First one. Second one. Third one.".to_owned()
        })
        .await;
        assert_eq!(out.reply, "First one. Second one. Third one.");
        assert_eq!(
            out.audio.len(),
            1,
            "playback of what was ready is still possible"
        );
        assert!(out.problems[0].contains("speech synthesis"));
    }
}
