//! Logical LED states, their precedence, and the console "LED" (REQ-DEV-004).
//!
//! The set is fixed and versioned (set 1, spec 0006 section 5.2). A board file maps each state to a device
//! pattern; a state with no mapping is a no-op. [`LedController`] holds what is active and shows the single
//! highest-precedence state, so boards never reorder it.

use crate::tone::Tone;
use std::io::Write;
use std::time::{Duration, Instant};

/// One logical state of the LED set (version 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LedState {
    /// Ready.
    Idle,
    /// Wake word or button heard.
    Wake,
    /// Capturing speech.
    Listening,
    /// Waiting for the assistant.
    Thinking,
    /// Reply playing.
    Speaking,
    /// Microphone muted.
    Muted,
    /// Something failed.
    Error,
    /// Core unreachable.
    Offline,
    /// A timer runs or fired.
    Timer,
    /// An alarm rings.
    Alarm,
    /// An inbox item waits.
    Inbox,
    /// Volume is changing.
    Volume,
    /// An update installs.
    Updating,
    /// Waiting for Wi-Fi setup.
    Setup,
}

/// Highest precedence first (spec 0006 section 5.2).
const PRECEDENCE: [LedState; 14] = [
    LedState::Updating,
    LedState::Setup,
    LedState::Offline,
    LedState::Error,
    LedState::Volume,
    LedState::Muted,
    LedState::Alarm,
    LedState::Timer,
    LedState::Wake,
    LedState::Listening,
    LedState::Speaking,
    LedState::Thinking,
    LedState::Inbox,
    LedState::Idle,
];

impl LedState {
    /// The state's name in a board file.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Wake => "wake",
            Self::Listening => "listening",
            Self::Thinking => "thinking",
            Self::Speaking => "speaking",
            Self::Muted => "muted",
            Self::Error => "error",
            Self::Offline => "offline",
            Self::Timer => "timer",
            Self::Alarm => "alarm",
            Self::Inbox => "inbox",
            Self::Volume => "volume",
            Self::Updating => "updating",
            Self::Setup => "setup",
        }
    }

    /// Parse a name; an unknown name is `None` so callers can ignore it (REQ-DEV-004).
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        PRECEDENCE.iter().copied().find(|s| s.name() == name)
    }

    /// Every state, highest precedence first.
    #[must_use]
    pub fn all() -> [Self; 14] {
        PRECEDENCE
    }

    /// The built-in tone that stands in for this state on a board with no LEDs, if the state has one.
    #[must_use]
    pub fn tone_equivalent(self) -> Option<Tone> {
        match self {
            Self::Wake | Self::Listening => Some(Tone::Wake),
            Self::Error => Some(Tone::Error),
            Self::Muted => Some(Tone::Mute),
            Self::Timer | Self::Alarm => Some(Tone::Timer),
            Self::Inbox => Some(Tone::Inbox),
            Self::Offline => Some(Tone::Error),
            _ => None,
        }
    }

    fn bit(self) -> u16 {
        1 << PRECEDENCE.iter().position(|s| *s == self).unwrap_or(0)
    }
}

/// Somewhere states are shown: LEDs, a console, nothing.
pub trait LedSink: Send {
    /// Show `state`. `step` carries the volume step for [`LedState::Volume`].
    fn show(&mut self, state: LedState, step: Option<u8>);
    /// Set brightness, 0 to 100 (quiet hours lower it).
    fn brightness(&mut self, pct: u8);
}

/// No LEDs.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullLeds;

impl LedSink for NullLeds {
    fn show(&mut self, _state: LedState, _step: Option<u8>) {}
    fn brightness(&mut self, _pct: u8) {}
}

/// Prints state changes, for the desktop board and tests: `led: thinking (active-thinking)`.
pub struct ConsoleLeds {
    out: Box<dyn Write + Send>,
    patterns: Vec<(String, String)>,
}

impl std::fmt::Debug for ConsoleLeds {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConsoleLeds")
            .field("patterns", &self.patterns.len())
            .finish_non_exhaustive()
    }
}

impl ConsoleLeds {
    /// Print to `out`, naming the board file's pattern for each state when it has one.
    #[must_use]
    pub fn new(out: Box<dyn Write + Send>, patterns: Vec<(String, String)>) -> Self {
        Self { out, patterns }
    }

    /// Print to standard error.
    #[must_use]
    pub fn stderr(patterns: Vec<(String, String)>) -> Self {
        Self::new(Box::new(std::io::stderr()), patterns)
    }
}

impl LedSink for ConsoleLeds {
    fn show(&mut self, state: LedState, step: Option<u8>) {
        let pattern =
            self.patterns
                .iter()
                .find(|(k, _)| k == state.name())
                .map(|(_, p)| match step {
                    Some(n) => p.replacen("{nn}", &format!("{n:02}"), 1),
                    None => p.clone(),
                });
        let _ = match pattern {
            Some(p) => writeln!(self.out, "led: {} ({p})", state.name()),
            None => writeln!(self.out, "led: {}", state.name()),
        };
    }

    fn brightness(&mut self, pct: u8) {
        let _ = writeln!(self.out, "led: brightness {pct}%");
    }
}

/// How long the transient volume state stays.
pub const VOLUME_SHOWN: Duration = Duration::from_millis(1200);

/// Holds the active states and drives a [`LedSink`] with the winner.
pub struct LedController {
    sink: Box<dyn LedSink>,
    /// The turn state: one of idle, wake, listening, thinking, speaking, error, inbox.
    turn: LedState,
    flags: u16,
    volume_until: Option<Instant>,
    volume_step: u8,
    shown: Option<(LedState, Option<u8>)>,
    present: bool,
}

impl std::fmt::Debug for LedController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LedController")
            .field("turn", &self.turn)
            .field("present", &self.present)
            .finish_non_exhaustive()
    }
}

impl LedController {
    /// A controller over `sink`. `present` is false for a board with no LEDs: states are still tracked (so
    /// status can report them) but nothing is shown.
    #[must_use]
    pub fn new(sink: Box<dyn LedSink>, present: bool) -> Self {
        Self {
            sink,
            turn: LedState::Idle,
            flags: 0,
            volume_until: None,
            volume_step: 0,
            shown: None,
            present,
        }
    }

    /// Whether this board has LEDs.
    #[must_use]
    pub fn present(&self) -> bool {
        self.present
    }

    /// The state that is shown now.
    #[must_use]
    pub fn current(&self) -> LedState {
        let active = |s: LedState| s == self.turn || self.flags & s.bit() != 0;
        PRECEDENCE
            .iter()
            .copied()
            .find(|s| active(*s))
            .unwrap_or(LedState::Idle)
    }

    /// Set the turn state (idle, wake, listening, thinking, speaking, error, inbox).
    pub fn turn(&mut self, state: LedState) {
        self.turn = state;
        self.refresh();
    }

    /// Turn a held state on or off (muted, offline, timer, alarm, updating, setup).
    pub fn set(&mut self, state: LedState, on: bool) {
        if on {
            self.flags |= state.bit();
        } else {
            self.flags &= !state.bit();
        }
        self.refresh();
    }

    /// Show the transient volume state for [`VOLUME_SHOWN`].
    pub fn show_volume(&mut self, step: u8, now: Instant) {
        self.volume_step = step;
        self.volume_until = Some(now + VOLUME_SHOWN);
        self.flags |= LedState::Volume.bit();
        self.refresh();
    }

    /// Drop the volume state once its time is up. Call on every pass of the loop.
    pub fn tick(&mut self, now: Instant) {
        if self.volume_until.is_some_and(|t| now >= t) {
            self.volume_until = None;
            self.flags &= !LedState::Volume.bit();
            self.refresh();
        }
    }

    /// Set brightness (quiet hours).
    pub fn brightness(&mut self, pct: u8) {
        if self.present {
            self.sink.brightness(pct);
        }
    }

    fn refresh(&mut self) {
        let now = (
            self.current(),
            (self.current() == LedState::Volume).then_some(self.volume_step),
        );
        if self.shown != Some(now) {
            self.shown = Some(now);
            if self.present {
                self.sink.show(now.0, now.1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct Buf(Arc<Mutex<Vec<u8>>>);
    impl Write for Buf {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl Buf {
        fn text(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    fn controller(buf: &Buf, present: bool) -> LedController {
        let leds = ConsoleLeds::new(
            Box::new(buf.clone()),
            vec![
                ("thinking".into(), "active-thinking".into()),
                ("volume".into(), "volume_step-{nn}".into()),
            ],
        );
        LedController::new(Box::new(leds), present)
    }

    #[test]
    fn req_dev_004_names_round_trip_and_unknown_names_are_ignored() {
        for s in LedState::all() {
            assert_eq!(LedState::parse(s.name()), Some(s));
        }
        assert_eq!(LedState::parse("sparkle"), None);
        assert_eq!(LedState::all().len(), 14);
    }

    #[test]
    fn req_dev_004_the_console_names_the_mapped_pattern() {
        let buf = Buf::default();
        let mut c = controller(&buf, true);
        c.turn(LedState::Thinking);
        c.turn(LedState::Listening);
        assert_eq!(
            buf.text(),
            "led: thinking (active-thinking)\nled: listening\n"
        );
    }

    #[test]
    fn req_dev_004_precedence_picks_one_state() {
        let buf = Buf::default();
        let mut c = controller(&buf, true);
        c.turn(LedState::Speaking);
        c.set(LedState::Muted, true);
        assert_eq!(c.current(), LedState::Muted);
        c.set(LedState::Offline, true);
        assert_eq!(c.current(), LedState::Offline);
        c.set(LedState::Updating, true);
        assert_eq!(c.current(), LedState::Updating);
        c.set(LedState::Updating, false);
        c.set(LedState::Offline, false);
        c.set(LedState::Muted, false);
        assert_eq!(c.current(), LedState::Speaking);
        c.turn(LedState::Idle);
        assert_eq!(c.current(), LedState::Idle);
    }

    #[test]
    fn req_dev_004_repeats_are_not_printed_twice() {
        let buf = Buf::default();
        let mut c = controller(&buf, true);
        c.turn(LedState::Thinking);
        c.turn(LedState::Thinking);
        c.set(LedState::Inbox, false);
        assert_eq!(buf.text().lines().count(), 1);
    }

    #[test]
    fn req_dev_004_volume_is_transient_and_carries_its_step() {
        let buf = Buf::default();
        let mut c = controller(&buf, true);
        let t0 = Instant::now();
        c.show_volume(7, t0);
        assert_eq!(c.current(), LedState::Volume);
        c.tick(t0 + Duration::from_millis(100));
        assert_eq!(c.current(), LedState::Volume);
        c.tick(t0 + VOLUME_SHOWN);
        assert_eq!(c.current(), LedState::Idle);
        assert!(buf.text().contains("led: volume (volume_step-07)"));
    }

    #[test]
    fn req_dev_013_a_board_without_leds_shows_nothing_but_still_tracks() {
        let buf = Buf::default();
        let mut c = controller(&buf, false);
        c.turn(LedState::Error);
        c.brightness(5);
        assert_eq!(c.current(), LedState::Error);
        assert_eq!(buf.text(), "");
    }

    #[test]
    fn req_dev_013_states_with_a_spoken_or_sound_equivalent_have_a_tone() {
        assert!(LedState::Error.tone_equivalent().is_some());
        assert!(LedState::Wake.tone_equivalent().is_some());
        assert!(LedState::Inbox.tone_equivalent().is_some());
        assert!(LedState::Thinking.tone_equivalent().is_none());
    }
}
