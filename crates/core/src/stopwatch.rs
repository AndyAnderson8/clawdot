//! A stopwatch that counts up (REQ-TMR-010..013).
//!
//! The reading is the wall-clock time since the start, in whole seconds. That is what keeps it correct
//! across a restart (REQ-TMR-011): time while the speaker was off counts, like a stopwatch left running.
//! The price is that it trusts the clock, so it is honest about two cases. A clock that is not set is
//! refused by the caller (REQ-TMR-009). A clock that has gone *backwards* since the last moment this value
//! recorded is detected here and never turned into a made-up number (REQ-TMR-012). A clock that jumps
//! *forward* (the network time arriving after a start on a wrong clock) cannot be told from time passing;
//! starting needs a set clock, which makes that rare, and the limit is documented.

use crate::sched::Ts;
use serde::{Deserialize, Serialize};

/// Most laps kept (REQ-TMR-011).
pub const MAX_LAPS: usize = 50;

/// The saved stopwatch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stopwatch {
    started_at: Ts,
    /// Total elapsed seconds at each lap, oldest first.
    laps: Vec<i64>,
    /// The final reading, once stopped.
    stopped: Option<i64>,
    /// The latest moment recorded (start, lap or stop), to notice a clock that went backwards.
    last: Ts,
}

/// What the stopwatch shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reading {
    /// Whole seconds so far, or the final time.
    pub elapsed: i64,
    /// Still counting.
    pub running: bool,
    /// Laps recorded.
    pub laps: usize,
}

/// A lap just taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lap {
    /// 1 for the first lap.
    pub number: usize,
    /// Seconds since the previous lap (or the start).
    pub split: i64,
    /// Seconds since the start.
    pub total: i64,
}

/// Why an operation did not happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// There is no stopwatch.
    None,
    /// It is already counting; carries the reading.
    Running(Reading),
    /// It is stopped; carries the final reading.
    Stopped(Reading),
    /// The clock is earlier than the last moment recorded; carries the last elapsed time that can be
    /// vouched for.
    ClockWentBack {
        /// Seconds up to the last recorded moment.
        last_good: i64,
    },
    /// [`MAX_LAPS`] laps are kept already.
    TooManyLaps,
}

impl Stopwatch {
    /// A stopwatch started at `now`.
    #[must_use]
    pub fn start(now: Ts) -> Self {
        Self {
            started_at: now,
            laps: Vec::new(),
            stopped: None,
            last: now,
        }
    }

    fn reading_at(&self, elapsed: i64) -> Reading {
        Reading {
            elapsed,
            running: self.stopped.is_none(),
            laps: self.laps.len(),
        }
    }

    /// Seconds up to a running stopwatch's `now`, unless the clock went backwards.
    fn running_elapsed(&self, now: Ts) -> Result<i64, Refusal> {
        if now < self.last {
            return Err(Refusal::ClockWentBack {
                last_good: (self.last - self.started_at).max(0),
            });
        }
        Ok(now - self.started_at)
    }

    /// What it shows now.
    ///
    /// # Errors
    /// [`Refusal::ClockWentBack`].
    pub fn read(&self, now: Ts) -> Result<Reading, Refusal> {
        match self.stopped {
            Some(total) => Ok(self.reading_at(total)),
            None => self.running_elapsed(now).map(|e| self.reading_at(e)),
        }
    }

    /// The splits in lap order, oldest first (each lap's own time).
    #[must_use]
    pub fn splits(&self) -> Vec<i64> {
        let mut prev = 0;
        self.laps
            .iter()
            .map(|t| {
                let s = t - prev;
                prev = *t;
                s
            })
            .collect()
    }

    /// Record a lap.
    ///
    /// # Errors
    /// [`Refusal::Stopped`], [`Refusal::ClockWentBack`] or [`Refusal::TooManyLaps`].
    pub fn lap(&mut self, now: Ts) -> Result<Lap, Refusal> {
        if let Some(total) = self.stopped {
            return Err(Refusal::Stopped(self.reading_at(total)));
        }
        let total = self.running_elapsed(now)?;
        if self.laps.len() >= MAX_LAPS {
            return Err(Refusal::TooManyLaps);
        }
        let split = total - self.laps.last().copied().unwrap_or(0);
        self.laps.push(total);
        self.last = now;
        Ok(Lap {
            number: self.laps.len(),
            split,
            total,
        })
    }

    /// Stop it and return the final reading.
    ///
    /// # Errors
    /// [`Refusal::Stopped`] if it already is; [`Refusal::ClockWentBack`].
    pub fn stop(&mut self, now: Ts) -> Result<Reading, Refusal> {
        if let Some(total) = self.stopped {
            return Err(Refusal::Stopped(self.reading_at(total)));
        }
        let total = self.running_elapsed(now)?;
        self.stopped = Some(total);
        self.last = now;
        Ok(self.reading_at(total))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: Ts = 1_800_000_000;

    #[test]
    fn it_counts_whole_seconds_from_the_start() {
        let sw = Stopwatch::start(T0);
        assert_eq!(
            sw.read(T0 + 75),
            Ok(Reading {
                elapsed: 75,
                running: true,
                laps: 0
            })
        );
        assert_eq!(sw.read(T0).unwrap().elapsed, 0);
    }

    #[test]
    fn laps_report_their_own_time_and_the_total() {
        let mut sw = Stopwatch::start(T0);
        assert_eq!(
            sw.lap(T0 + 60),
            Ok(Lap {
                number: 1,
                split: 60,
                total: 60
            })
        );
        assert_eq!(
            sw.lap(T0 + 125),
            Ok(Lap {
                number: 2,
                split: 65,
                total: 125
            })
        );
        assert_eq!(sw.splits(), vec![60, 65]);
        let done = sw.stop(T0 + 190).unwrap();
        assert_eq!((done.elapsed, done.running, done.laps), (190, false, 2));
    }

    #[test]
    fn a_stopped_stopwatch_keeps_its_final_time_whatever_the_clock_does() {
        let mut sw = Stopwatch::start(T0);
        sw.stop(T0 + 10).unwrap();
        assert_eq!(sw.read(T0 + 99_999).unwrap().elapsed, 10);
        assert_eq!(sw.read(T0 - 5_000).unwrap().elapsed, 10);
        assert!(matches!(sw.stop(T0 + 20), Err(Refusal::Stopped(r)) if r.elapsed == 10));
        assert!(matches!(sw.lap(T0 + 20), Err(Refusal::Stopped(_))));
    }

    #[test]
    fn a_clock_that_went_backwards_is_refused_not_guessed() {
        let mut sw = Stopwatch::start(T0);
        sw.lap(T0 + 100).unwrap();
        for now in [T0 + 99, T0 - 3600, 0] {
            assert_eq!(sw.read(now), Err(Refusal::ClockWentBack { last_good: 100 }));
            assert_eq!(sw.lap(now), Err(Refusal::ClockWentBack { last_good: 100 }));
            assert_eq!(sw.stop(now), Err(Refusal::ClockWentBack { last_good: 100 }));
        }
        // Nothing was recorded by the refusals, and a good clock carries on from the real start.
        assert_eq!(sw.read(T0 + 130).unwrap().elapsed, 130);
        assert_eq!(sw.splits(), vec![100]);
    }

    #[test]
    fn laps_are_bounded() {
        let mut sw = Stopwatch::start(T0);
        for i in 1..=MAX_LAPS {
            sw.lap(T0 + i64::try_from(i).unwrap()).unwrap();
        }
        assert_eq!(sw.lap(T0 + 1000), Err(Refusal::TooManyLaps));
        assert_eq!(sw.stop(T0 + 1000).unwrap().laps, MAX_LAPS);
    }

    #[test]
    fn it_round_trips_through_json_for_a_restart() {
        let mut sw = Stopwatch::start(T0);
        sw.lap(T0 + 30).unwrap();
        let back: Stopwatch = serde_json::from_str(&serde_json::to_string(&sw).unwrap()).unwrap();
        assert_eq!(back, sw);
        assert_eq!(back.read(T0 + 45).unwrap().elapsed, 45);
    }
}
