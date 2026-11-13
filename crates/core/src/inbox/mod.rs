//! The agent inbox: events from agents and services that the speaker announces, summarises, and lets
//! the user answer (REQ-INB-001..020).
//!
//! * [`event`]: the event model.
//! * [`adapter`]: declarative mappings from an agent's native payload to an event (REQ-INB-003, EXT-004).
//! * [`endpoint`]: named, token-protected sources (REQ-INB-001/002).
//! * [`queue`]: persistence, filtering, coalescing, and announcement order (REQ-INB-004..006, 012, 014).
//! * [`reply`]: at-least-once reply delivery to agent bridges (REQ-INB-008).
//! * [`summary`]: bounded spoken summaries (REQ-INB-018).

pub mod adapter;
pub mod endpoint;
pub mod event;
pub mod queue;
pub mod reply;
pub mod summary;

use crate::sched::{Clock, Ts};

/// One instant read both ways. A calendar point (when something arrived, as shown to a person, or as a bridge
/// sees it) is wall time, which can jump when the network time arrives on a device with no battery clock. A
/// duration (how long an event lives, how long a snooze lasts, the coalesce window, how old a handled event is,
/// how long a reply waits) is measured on the monotonic clock, as the scheduler does for a countdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Moment {
    /// UTC unix time.
    pub wall: Ts,
    /// [`Clock::monotonic_secs`].
    pub mono: i64,
}

impl Moment {
    /// Now, on `clock`.
    #[must_use]
    pub fn of(clock: &dyn Clock) -> Self {
        Self {
            wall: clock.now(),
            mono: clock.monotonic_secs(),
        }
    }
}

/// For tests that have one number for both clocks.
#[cfg(test)]
impl From<Ts> for Moment {
    fn from(t: Ts) -> Self {
        Self { wall: t, mono: t }
    }
}

/// What is needed to turn the durations in a saved file back into monotonic readings.
///
/// A file holds each duration as seconds relative to the moment of the save, plus the wall time of the save when
/// the clock was valid. Like a saved countdown (see `sched`), the time the program was off is taken off with the
/// wall clock, as far as it can be trusted: a clock that is not valid, or is behind the save, takes off nothing.
#[derive(Debug, Clone, Copy)]
struct Restore {
    wall: Ts,
    mono: i64,
    valid: bool,
    /// Seconds the program was off.
    off: i64,
}

impl Restore {
    fn new(saved_at: Option<Ts>, clock: &dyn Clock) -> Self {
        let m = Moment::of(clock);
        let valid = clock.wall_clock_valid();
        let off = match saved_at {
            Some(at) if valid => m.wall.saturating_sub(at).max(0),
            _ => 0,
        };
        Self {
            wall: m.wall,
            mono: m.mono,
            valid,
            off,
        }
    }

    /// The monotonic reading of a moment that was `ago` seconds before the save.
    fn past(&self, ago: i64) -> i64 {
        self.mono.saturating_sub(ago.saturating_add(self.off))
    }

    /// The monotonic reading of a moment that was `left` seconds after the save.
    fn ahead(&self, left: i64) -> i64 {
        self.mono.saturating_add(left).saturating_sub(self.off)
    }
}

/// The wall time to put in a file: the save's, when the clock was valid.
fn saved_at(clock: &dyn Clock) -> Option<Ts> {
    clock.wall_clock_valid().then(|| clock.now())
}
