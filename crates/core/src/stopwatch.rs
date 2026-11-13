//! A stopwatch that counts up (REQ-TMR-010..013).
//!
//! While the program runs, the reading is measured on the monotonic clock
//! ([`crate::sched::Clock::monotonic_secs`]), the same way a countdown is, so a step of the wall clock
//! (network time arriving) neither adds to it nor takes from it (REQ-TMR-012). Across a restart the wall
//! clock is the only thing that spans the gap (REQ-TMR-011): the stopwatch is saved as the seconds it had
//! counted and the wall time of the save, and at load the time the program was off is added when the wall
//! clock is valid now, was valid at the save and is not behind it. Otherwise nothing is added, and never a
//! negative amount.
//!
//! Files written by the first version (`started_at`, `laps`, `stopped`, `last`) still load: their reading is
//! the wall clock minus `started_at` when the clock is valid, else the time up to `last`.

use crate::sched::Ts;
use serde::{Deserialize, Serialize};

/// Most laps kept (REQ-TMR-011).
pub const MAX_LAPS: usize = 50;

/// The saved stopwatch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stopwatch {
    /// Total elapsed seconds at each lap, oldest first.
    laps: Vec<i64>,
    /// The final reading, once stopped.
    stopped: Option<i64>,
    /// File form: seconds counted when it was saved. Only meaningful between a save and the next load.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    elapsed: Option<i64>,
    /// File form: the wall time of the save, when the wall clock was valid then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    saved_at: Option<Ts>,
    /// First-version file form: the wall time of the start. Read, never written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    started_at: Option<Ts>,
    /// First-version file form: the latest moment recorded. Read, never written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last: Option<Ts>,
    /// The monotonic reading at which the count was 0, while the program runs. Means nothing in a file.
    #[serde(skip)]
    origin_mono: Option<i64>,
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
    /// [`MAX_LAPS`] laps are kept already.
    TooManyLaps,
}

impl Stopwatch {
    /// A stopwatch started at the monotonic reading `mono`.
    #[must_use]
    pub fn start(mono: i64) -> Self {
        Self {
            laps: Vec::new(),
            stopped: None,
            elapsed: None,
            saved_at: None,
            started_at: None,
            last: None,
            origin_mono: Some(mono),
        }
    }

    /// Put a stopwatch just read from a file back on the monotonic clock.
    ///
    /// `wall`, `mono` and `valid` are the clock now. The time the program was off is added only when `valid`
    /// and the save has a wall time that is not ahead of `wall`.
    pub fn resume(&mut self, wall: Ts, mono: i64, valid: bool) {
        if self.stopped.is_some() {
            self.origin_mono = None;
            return;
        }
        let counted = if let Some(e) = self.elapsed.take() {
            let off = match self.saved_at.take() {
                Some(at) if valid => wall.saturating_sub(at).max(0),
                _ => 0,
            };
            e.max(0).saturating_add(off)
        } else if let Some(start) = self.started_at {
            // The first version counted the wall clock from the start.
            let to = if valid {
                wall
            } else {
                self.last.unwrap_or(start)
            };
            to.saturating_sub(start).max(0)
        } else {
            0
        };
        self.started_at = None;
        self.last = None;
        self.saved_at = None;
        self.origin_mono = Some(mono.saturating_sub(counted));
    }

    /// The copy that goes to disk: the seconds counted so far and the wall time of the save (`None` when the
    /// wall clock is not valid, so a later load adds nothing).
    #[must_use]
    pub fn persisted(&self, wall: Ts, mono: i64, valid: bool) -> Self {
        let mut s = self.clone();
        s.elapsed = s.stopped.is_none().then(|| self.counted(mono));
        s.saved_at = valid.then_some(wall);
        s.started_at = None;
        s.last = None;
        s.origin_mono = None;
        s
    }

    fn counted(&self, mono: i64) -> i64 {
        self.origin_mono
            .map_or_else(|| self.elapsed.unwrap_or(0), |o| mono.saturating_sub(o))
            .max(0)
    }

    fn reading_at(&self, elapsed: i64) -> Reading {
        Reading {
            elapsed,
            running: self.stopped.is_none(),
            laps: self.laps.len(),
        }
    }

    /// What it shows at the monotonic reading `mono`.
    #[must_use]
    pub fn read(&self, mono: i64) -> Reading {
        self.reading_at(self.stopped.unwrap_or_else(|| self.counted(mono)))
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
    /// [`Refusal::Stopped`] or [`Refusal::TooManyLaps`].
    pub fn lap(&mut self, mono: i64) -> Result<Lap, Refusal> {
        if let Some(total) = self.stopped {
            return Err(Refusal::Stopped(self.reading_at(total)));
        }
        let total = self.counted(mono);
        if self.laps.len() >= MAX_LAPS {
            return Err(Refusal::TooManyLaps);
        }
        let split = total - self.laps.last().copied().unwrap_or(0);
        self.laps.push(total);
        Ok(Lap {
            number: self.laps.len(),
            split,
            total,
        })
    }

    /// Stop it and return the final reading.
    ///
    /// # Errors
    /// [`Refusal::Stopped`] if it already is.
    pub fn stop(&mut self, mono: i64) -> Result<Reading, Refusal> {
        if let Some(total) = self.stopped {
            return Err(Refusal::Stopped(self.reading_at(total)));
        }
        let total = self.counted(mono);
        self.stopped = Some(total);
        Ok(self.reading_at(total))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: Ts = 1_800_000_000;
    const M0: i64 = 500;

    // REQ-TMR-010
    #[test]
    fn it_counts_whole_seconds_from_the_start() {
        let sw = Stopwatch::start(M0);
        assert_eq!(
            sw.read(M0 + 75),
            Reading {
                elapsed: 75,
                running: true,
                laps: 0
            }
        );
        assert_eq!(sw.read(M0).elapsed, 0);
    }

    // REQ-TMR-013
    #[test]
    fn laps_report_their_own_time_and_the_total() {
        let mut sw = Stopwatch::start(M0);
        assert_eq!(
            sw.lap(M0 + 60),
            Ok(Lap {
                number: 1,
                split: 60,
                total: 60
            })
        );
        assert_eq!(
            sw.lap(M0 + 125),
            Ok(Lap {
                number: 2,
                split: 65,
                total: 125
            })
        );
        assert_eq!(sw.splits(), vec![60, 65]);
        let done = sw.stop(M0 + 190).unwrap();
        assert_eq!((done.elapsed, done.running, done.laps), (190, false, 2));
    }

    // REQ-TMR-012
    #[test]
    fn a_stopped_stopwatch_keeps_its_final_time() {
        let mut sw = Stopwatch::start(M0);
        sw.stop(M0 + 10).unwrap();
        assert_eq!(sw.read(M0 + 99_999).elapsed, 10);
        assert!(matches!(sw.stop(M0 + 20), Err(Refusal::Stopped(r)) if r.elapsed == 10));
        assert!(matches!(sw.lap(M0 + 20), Err(Refusal::Stopped(_))));
    }

    // REQ-TMR-011
    #[test]
    fn laps_are_bounded() {
        let mut sw = Stopwatch::start(M0);
        for i in 1..=MAX_LAPS {
            sw.lap(M0 + i64::try_from(i).unwrap()).unwrap();
        }
        assert_eq!(sw.lap(M0 + 1000), Err(Refusal::TooManyLaps));
        assert_eq!(sw.stop(M0 + 1000).unwrap().laps, MAX_LAPS);
    }

    fn reload(
        sw: &Stopwatch,
        saved_wall: Ts,
        saved_valid: bool,
        wall: Ts,
        valid: bool,
    ) -> Stopwatch {
        let json = serde_json::to_string(&sw.persisted(saved_wall, M0 + 30, saved_valid)).unwrap();
        let mut back: Stopwatch = serde_json::from_str(&json).unwrap();
        back.resume(wall, 7, valid);
        back
    }

    // REQ-TMR-011
    #[test]
    fn a_restart_with_a_valid_clock_adds_the_time_it_was_off() {
        let mut sw = Stopwatch::start(M0);
        sw.lap(M0 + 30).unwrap();
        let back = reload(&sw, T0, true, T0 + 3_570, true);
        assert_eq!(back.read(7).elapsed, 30 + 3_570);
        assert_eq!(back.read(7 + 10).elapsed, 30 + 3_570 + 10);
        assert_eq!(back.splits(), vec![30]);
    }

    // REQ-TMR-012
    #[test]
    fn a_restart_with_a_clock_behind_the_save_or_not_valid_adds_nothing() {
        let sw = Stopwatch::start(M0);
        for (saved_valid, wall, valid) in [
            (true, T0 - 3_600, true),
            (true, 0, false),
            (false, T0 + 3_600, true),
            (false, 0, false),
        ] {
            let back = reload(&sw, T0, saved_valid, wall, valid);
            assert_eq!(back.read(7).elapsed, 30, "{saved_valid} {wall} {valid}");
        }
    }

    // REQ-TMR-011
    #[test]
    fn a_stopped_one_keeps_its_time_across_a_restart() {
        let mut sw = Stopwatch::start(M0);
        sw.stop(M0 + 12).unwrap();
        let back = reload(&sw, T0, true, T0 + 99_999, true);
        assert_eq!(back.read(99).elapsed, 12);
        assert!(!back.read(99).running);
    }

    // REQ-TMR-011
    #[test]
    fn a_file_from_the_first_version_still_loads() {
        let old = r#"{"started_at":1800000000,"laps":[30],"stopped":null,"last":1800000030}"#;
        let mut sw: Stopwatch = serde_json::from_str(old).unwrap();
        sw.resume(T0 + 100, 9, true);
        let r = sw.read(9 + 5);
        assert_eq!((r.elapsed, r.running, r.laps), (105, true, 1));
        // With the clock not valid, only the time up to the last record can be vouched for.
        let mut sw: Stopwatch = serde_json::from_str(old).unwrap();
        sw.resume(0, 9, false);
        assert_eq!(sw.read(9).elapsed, 30);
        // A stopped one keeps its final time.
        let stopped = r#"{"started_at":1800000000,"laps":[],"stopped":42,"last":1800000042}"#;
        let mut sw: Stopwatch = serde_json::from_str(stopped).unwrap();
        sw.resume(T0 + 9_999, 9, true);
        assert_eq!(sw.read(9).elapsed, 42);
    }

    // REQ-TMR-011
    #[test]
    fn it_writes_only_the_new_form() {
        let sw = Stopwatch::start(M0);
        let json = serde_json::to_string(&sw.persisted(T0, M0 + 5, true)).unwrap();
        assert!(
            !json.contains("started_at") && !json.contains("last"),
            "{json}"
        );
        assert!(
            json.contains("\"elapsed\":5") && json.contains("saved_at"),
            "{json}"
        );
    }
}
