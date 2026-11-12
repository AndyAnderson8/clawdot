//! The stopwatch, by voice and as a tool (REQ-TMR-010, REQ-TMR-012, REQ-TMR-013).
//!
//! One place makes the words, so the fast path and the model's tool say the same thing.

use super::{Assistant, CLOCK_UNSET, format_duration};
use crate::stopwatch::{Reading, Refusal};

/// What the user asked of the stopwatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Start counting.
    Start,
    /// Record a lap.
    Lap,
    /// Stop and report the total.
    Stop,
    /// Say the time so far.
    Read,
    /// Throw it away.
    Reset,
}

impl Action {
    /// The tool's `action` argument.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "start" => Self::Start,
            "lap" => Self::Lap,
            "stop" => Self::Stop,
            "read" => Self::Read,
            "reset" => Self::Reset,
            _ => return None,
        })
    }
}

fn laps_word(n: usize) -> String {
    format!("{n} lap{}", if n == 1 { "" } else { "s" })
}

/// A refusal in words.
fn refusal_words(r: Refusal) -> String {
    match r {
        Refusal::None => "There is no stopwatch running.".to_owned(),
        Refusal::Running(r) => format!(
            "The stopwatch is already running, at {}.",
            format_duration(r.elapsed)
        ),
        Refusal::Stopped(r) => format!(
            "The stopwatch is stopped at {}.",
            format_duration(r.elapsed)
        ),
        Refusal::ClockWentBack { last_good } => format!(
            "My clock went backwards while the stopwatch was running, so I can't trust its time. \
             The last time I can vouch for is {}.",
            format_duration(last_good)
        ),
        Refusal::TooManyLaps => format!(
            "That is {}, the most I can keep. Stop the stopwatch to hear the total.",
            laps_word(crate::stopwatch::MAX_LAPS)
        ),
    }
}

fn stopped_words(r: Reading) -> String {
    let mut s = format!("Stopped at {}", format_duration(r.elapsed));
    if r.laps > 0 {
        s.push_str(&format!(", {}", laps_word(r.laps)));
    }
    s.push('.');
    s
}

impl Assistant {
    /// Carry out a stopwatch action and say what happened.
    ///
    /// # Errors
    /// The words to say when the clock is not set; everything else, including a refusal, is an answer.
    pub fn stopwatch(&mut self, action: Action) -> Result<String, String> {
        if action != Action::Reset && !self.clock.wall_clock_valid() {
            return Err(CLOCK_UNSET.to_owned());
        }
        let now = self.clock.now();
        let said = match action {
            Action::Start => match self.sched.stopwatch_start(now) {
                Ok(()) => "Stopwatch started.".to_owned(),
                Err(r) => refusal_words(r),
            },
            Action::Lap => match self.sched.stopwatch_lap(now) {
                Ok(l) => format!(
                    "Lap {}, {}. Total {}.",
                    l.number,
                    format_duration(l.split),
                    format_duration(l.total)
                ),
                Err(r) => refusal_words(r),
            },
            Action::Stop => match self.sched.stopwatch_stop(now) {
                Ok(r) => stopped_words(r),
                Err(r) => refusal_words(r),
            },
            Action::Read => match self.sched.stopwatch_read(now) {
                Ok((r, splits)) if r.running => {
                    let mut s = format!("{} so far", format_duration(r.elapsed));
                    if let Some(last) = splits.last() {
                        s.push_str(&format!(
                            ", {}, the last one {}",
                            laps_word(splits.len()),
                            format_duration(*last)
                        ));
                    }
                    s.push('.');
                    s
                }
                Ok((r, _)) => format!(
                    "The stopwatch is stopped at {}.",
                    format_duration(r.elapsed)
                ),
                Err(r) => refusal_words(r),
            },
            Action::Reset => {
                if self.sched.stopwatch_reset() {
                    "Stopwatch reset.".to_owned()
                } else {
                    "There is no stopwatch to reset.".to_owned()
                }
            }
        };
        if matches!(
            action,
            Action::Start | Action::Lap | Action::Stop | Action::Reset
        ) {
            self.save();
        }
        Ok(said)
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{TestClock, setup, utc};
    use super::*;
    use crate::assistant::Assistant;
    use crate::sched::{FixedOffset, Scheduler};
    use crate::session::{Tier, ToolHost};
    use crate::store::JsonStore;
    use serde_json::json;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicI64, Ordering};

    fn say(a: &mut Assistant, u: &str) -> String {
        a.handle(u)
            .unwrap_or_else(|| panic!("fell through: {u}"))
            .say
    }

    // REQ-TMR-010, REQ-TMR-013
    #[test]
    fn start_lap_read_stop_by_voice() {
        let (mut a, clock, _) = setup(false);
        assert_eq!(say(&mut a, "start the stopwatch"), "Stopwatch started.");
        clock.0.fetch_add(60, Ordering::Relaxed);
        assert_eq!(say(&mut a, "lap"), "Lap 1, 1 minute. Total 1 minute.");
        clock.0.fetch_add(65, Ordering::Relaxed);
        assert_eq!(
            say(&mut a, "mark a lap"),
            "Lap 2, 1 minute and 5 seconds. Total 2 minutes and 5 seconds."
        );
        clock.0.fetch_add(5, Ordering::Relaxed);
        assert_eq!(
            say(&mut a, "how long on the stopwatch"),
            "2 minutes and 10 seconds so far, 2 laps, the last one 1 minute and 5 seconds."
        );
        clock.0.fetch_add(50, Ordering::Relaxed);
        assert_eq!(
            say(&mut a, "stop the stopwatch"),
            "Stopped at 3 minutes, 2 laps."
        );
        clock.0.fetch_add(10_000, Ordering::Relaxed);
        assert_eq!(
            say(&mut a, "what's the stopwatch at"),
            "The stopwatch is stopped at 3 minutes."
        );
        assert_eq!(say(&mut a, "lap"), "The stopwatch is stopped at 3 minutes.");
        assert_eq!(
            say(&mut a, "stop the stopwatch"),
            "The stopwatch is stopped at 3 minutes."
        );
        assert_eq!(say(&mut a, "reset the stopwatch"), "Stopwatch reset.");
        assert_eq!(say(&mut a, "lap"), "There is no stopwatch running.");
        assert_eq!(
            say(&mut a, "reset the stopwatch"),
            "There is no stopwatch to reset."
        );
        assert_eq!(say(&mut a, "start timing"), "Stopwatch started.");
    }

    // REQ-TMR-010
    #[test]
    fn starting_a_running_stopwatch_does_not_restart_it() {
        let (mut a, clock, _) = setup(false);
        say(&mut a, "start a stopwatch");
        clock.0.fetch_add(90, Ordering::Relaxed);
        assert_eq!(
            say(&mut a, "start the stopwatch"),
            "The stopwatch is already running, at 1 minute and 30 seconds."
        );
        clock.0.fetch_add(10, Ordering::Relaxed);
        assert_eq!(
            say(&mut a, "stop the stopwatch"),
            "Stopped at 1 minute and 40 seconds."
        );
        // Starting after a stop begins a new one.
        assert_eq!(say(&mut a, "start the stopwatch"), "Stopwatch started.");
        assert_eq!(say(&mut a, "stopwatch time"), "0 seconds so far.");
    }

    // REQ-TMR-011
    #[test]
    fn it_survives_a_restart_and_counts_the_time_the_speaker_was_off() {
        let dir = crate::store::testutil::TempDir::new("stopwatch");
        let store = JsonStore::new(dir.0.join("sched.json"), crate::sched::SCHEMA);
        let clock = Arc::new(TestClock(AtomicI64::new(utc(2026, 11, 7, 14, 42))));
        let build = |clock: &Arc<TestClock>| {
            Assistant::new(
                clock.clone(),
                Box::new(FixedOffset(0)),
                Scheduler::load(&store, 16, &**clock).unwrap_or_else(|_| Scheduler::new(16)),
                Some(store.clone()),
                None,
            )
        };
        {
            let mut a = build(&clock);
            say(&mut a, "start the stopwatch");
            clock.0.fetch_add(30, Ordering::Relaxed);
            say(&mut a, "lap");
        }
        clock.0.fetch_add(3_570, Ordering::Relaxed);
        let mut a = build(&clock);
        assert_eq!(
            say(&mut a, "check the stopwatch"),
            "1 hour so far, 1 lap, the last one 30 seconds."
        );
    }

    // REQ-TMR-012, REQ-TMR-009
    #[test]
    fn an_unset_clock_is_admitted_and_nothing_is_started_or_read() {
        let (mut a, clock, _) = setup(false);
        say(&mut a, "start the stopwatch");
        clock.0.store(utc(2010, 1, 1, 0, 3), Ordering::Relaxed);
        for u in [
            "start the stopwatch",
            "lap",
            "stop the stopwatch",
            "how long on the stopwatch",
        ] {
            assert_eq!(say(&mut a, u), crate::assistant::CLOCK_UNSET, "{u}");
        }
        // Resetting needs no clock, and afterwards nothing remains to be wrong about.
        assert_eq!(say(&mut a, "reset the stopwatch"), "Stopwatch reset.");
        assert_eq!(
            a.stopwatch(Action::Start),
            Err(crate::assistant::CLOCK_UNSET.to_owned())
        );
        clock.0.store(utc(2026, 11, 7, 14, 42), Ordering::Relaxed);
        assert_eq!(say(&mut a, "lap"), "There is no stopwatch running.");
    }

    // REQ-TMR-012
    #[test]
    fn a_clock_that_goes_backwards_is_not_turned_into_a_time() {
        let (mut a, clock, _) = setup(false);
        say(&mut a, "start the stopwatch");
        clock.0.fetch_add(120, Ordering::Relaxed);
        say(&mut a, "lap");
        clock.0.fetch_sub(500, Ordering::Relaxed);
        for u in ["how long on the stopwatch", "lap", "stop the stopwatch"] {
            let s = say(&mut a, u);
            assert!(
                s.contains("went backwards") && s.contains("2 minutes"),
                "{u}: {s}"
            );
            assert!(!s.contains("so far"), "{u}: {s}");
        }
        // Back on a sane clock it carries on from the real start.
        clock.0.fetch_add(500 + 30, Ordering::Relaxed);
        assert!(
            say(&mut a, "how long on the stopwatch").starts_with("2 minutes and 30 seconds so far")
        );
    }

    fn block<T>(f: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(f)
    }

    // REQ-TMR-010
    #[test]
    fn the_model_gets_a_stopwatch_tool_with_honest_tiers() {
        let (mut a, clock, _) = setup(false);
        assert!(a.tool_specs().iter().any(|t| t.name == "stopwatch"));
        assert_eq!(
            a.risk("stopwatch", &json!({"action":"read"})).tier,
            Tier::ReadOnly
        );
        for action in ["start", "lap", "stop", "reset"] {
            assert_eq!(
                a.risk("stopwatch", &json!({ "action": action })).tier,
                Tier::Reversible
            );
        }
        assert!(!a.returns_untrusted_text("stopwatch"));
        let mut run = |action: &str| block(a.call_tool("stopwatch", &json!({ "action": action })));
        assert_eq!(run("start").unwrap(), "Stopwatch started.");
        clock.0.fetch_add(10, Ordering::Relaxed);
        assert_eq!(run("read").unwrap(), "10 seconds so far.");
        assert!(run("pause").unwrap_err().contains("unknown action"));
        let e = block(a.call_tool("stopwatch", &json!({}))).unwrap_err();
        assert!(e.contains("action"), "{e}");
    }
}
