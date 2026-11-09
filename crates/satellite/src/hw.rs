//! Optional hardware capabilities, independent of the core and of the satellite protocol (REQ-VOI-019).
//!
//! A board (a PC, a Pi, an Echo Dot) provides the audio traits in [`crate::audio`] and, if it has them, any of
//! these: lights, buttons, tones. Each is optional; a board without one uses the `Null` version and nothing
//! breaks. Nothing here names a device path or a core type, so the same traits can be driven from a JSON board
//! file on every Linux board.

/// What the lights should show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedState {
    /// Nothing happening.
    Idle,
    /// Capturing speech.
    Listening,
    /// Waiting for the assistant.
    Thinking,
    /// Playing a reply.
    Speaking,
    /// Something went wrong.
    Error,
    /// Microphone muted.
    Muted,
}

/// Lights. A board without lights uses [`NullLeds`].
pub trait Leds: std::fmt::Debug + Send {
    /// Show `state`.
    fn show(&mut self, state: LedState);
}

/// No lights.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullLeds;

impl Leds for NullLeds {
    fn show(&mut self, _state: LedState) {}
}

/// A physical button event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    /// Start a turn (push-to-talk, or the action button).
    Talk,
    /// Toggle the microphone mute.
    Mute,
    /// Stop what is playing now.
    Stop,
}

/// Buttons. A board without buttons uses [`NullButtons`].
pub trait Buttons: std::fmt::Debug + Send {
    /// The next event, without blocking.
    fn poll(&mut self) -> Option<Button>;
}

/// No buttons.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullButtons;

impl Buttons for NullButtons {
    fn poll(&mut self) -> Option<Button> {
        None
    }
}

/// What an audio output can do beyond playing samples. Advertised so callers do not guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OutputCaps {
    /// `duck` changes the level.
    pub duck: bool,
    /// `set_volume` changes the level.
    pub volume: bool,
    /// `stop` silences at once.
    pub stop: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn req_voi_019_null_hardware_does_nothing_and_never_fails() {
        NullLeds.show(LedState::Error);
        assert_eq!(NullButtons.poll(), None);
        assert_eq!(
            OutputCaps::default(),
            OutputCaps {
                duck: false,
                volume: false,
                stop: false
            }
        );
    }
}
