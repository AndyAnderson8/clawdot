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
    /// Milliseconds without a reply before the thinking cue plays.
    pub thinking_delay_ms: u64,
    /// Endpointer tuning.
    pub vad: VadConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            follow_up_secs: 8,
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
        !self.muted && self.phase == Phase::Listening
    }

    /// Mute or unmute. Muting abandons whatever was being captured.
    pub fn set_muted(&mut self, muted: bool) -> Step {
        if muted == self.muted {
            return Step::Nothing;
        }
        self.muted = muted;
        if muted {
            self.endpointer.reset();
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

    /// The wake word was heard. Ignored while muted, thinking, or speaking (half-duplex).
    pub fn on_wake(&mut self) -> Step {
        if self.muted || matches!(self.phase, Phase::Thinking | Phase::Speaking) {
            return Step::Nothing;
        }
        self.start_listening();
        Step::Cue(Cue::Wake)
    }

    /// Microphone audio. Ignored unless the dialog is listening.
    pub fn on_audio(&mut self, now_ms: u64, pcm: &[i16]) -> Step {
        if !self.wants_audio() {
            return Step::Nothing;
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
    }

    /// The reply failed or there is nothing to say.
    pub fn on_reply_failed(&mut self) -> Step {
        self.phase = Phase::Idle;
        self.thinking_since = None;
        Step::Cue(Cue::Error)
    }

    /// Playback ended. Opens the follow-up window if configured (REQ-VOI-007).
    pub fn on_playback_done(&mut self) -> Step {
        self.thinking_since = None;
        if self.muted || self.cfg.follow_up_secs == 0 {
            self.phase = Phase::Idle;
            return Step::Nothing;
        }
        let mut vad = self.cfg.vad;
        vad.no_speech_ms =
            usize::try_from(self.cfg.follow_up_secs.saturating_mul(1000)).unwrap_or(usize::MAX);
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
    let mut splitter = SentenceSplitter::default();
    let mut sentences = splitter.push(&out.reply);
    sentences.extend(splitter.flush());
    for s in sentences {
        match tts.synthesize(&s).await {
            Ok(a) => out.audio.push(a),
            Err(e) => {
                out.problems.push(format!("speech synthesis: {e}"));
                break;
            }
        }
    }
    out
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
        let mut d = Dialog::new(Config {
            follow_up_secs: 0,
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
