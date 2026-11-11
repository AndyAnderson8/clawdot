//! Usage limits for the language model (REQ-AST-011).
//!
//! An always-on device calling a paid API needs a guardrail. Limits are per local day: a request
//! count and an optional spend cap derived from the cost the provider reports. When a limit is reached
//! the assistant says so and local features (timers, alarms, the fast path) keep working.

use crate::llm::Usage;
use crate::store::{JsonStore, StoreError};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Schema version of the persisted usage counters.
pub const SCHEMA: u32 = 1;

/// Configured limits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UsageLimits {
    /// Cap on generated tokens per response.
    pub max_output_tokens_per_turn: u32,
    /// Model requests allowed per local day.
    pub max_requests_per_day: u32,
    /// Spend allowed per local day, in US dollars, if set.
    pub daily_spend_cap_usd: Option<f64>,
}

impl Default for UsageLimits {
    fn default() -> Self {
        Self {
            max_output_tokens_per_turn: 300,
            max_requests_per_day: 1_000_000,
            daily_spend_cap_usd: None,
        }
    }
}

/// Which limit stopped a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitReached {
    /// The daily request count.
    Requests,
    /// The daily spend cap.
    Spend,
}

impl fmt::Display for LimitReached {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let what = match self {
            Self::Requests => "request",
            Self::Spend => "spending",
        };
        write!(
            f,
            "I've reached today's {what} limit for the language model. Timers, alarms, and simple commands still work."
        )
    }
}

impl std::error::Error for LimitReached {}

/// How many requests one past day made, kept so the portal can chart them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct PastDay {
    day: i64,
    requests: u32,
}

/// The most past days kept.
const KEEP_DAYS: usize = 30;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct State {
    /// Local day number the counters belong to.
    day: i64,
    requests: u32,
    /// Spend in millionths of a dollar, to avoid floating-point drift.
    micro_usd: u64,
    /// The days before today that made requests, oldest first.
    #[serde(default)]
    history: Vec<PastDay>,
}

/// Tracks usage against the limits.
#[derive(Debug, Clone)]
pub struct UsageTracker {
    limits: UsageLimits,
    state: State,
}

impl UsageTracker {
    /// A fresh tracker.
    #[must_use]
    pub fn new(limits: UsageLimits) -> Self {
        Self {
            limits,
            state: State::default(),
        }
    }

    /// Restore from a store.
    ///
    /// # Errors
    /// Storage errors.
    pub fn load(store: &JsonStore, limits: UsageLimits) -> Result<Self, StoreError> {
        Ok(Self {
            limits,
            state: store.load()?,
        })
    }

    /// Persist.
    ///
    /// # Errors
    /// Storage errors.
    pub fn save(&self, store: &JsonStore) -> Result<(), StoreError> {
        store.save(&self.state)
    }

    /// Replace the limits; the counters for today are kept.
    pub fn set_limits(&mut self, limits: UsageLimits) {
        self.limits = limits;
    }

    /// The configured limits.
    #[must_use]
    pub fn limits(&self) -> &UsageLimits {
        &self.limits
    }

    fn roll(&mut self, day: i64) {
        if self.state.day != day {
            let mut history = std::mem::take(&mut self.state.history);
            if self.state.requests > 0 && self.state.day != 0 {
                history.push(PastDay {
                    day: self.state.day,
                    requests: self.state.requests,
                });
            }
            if history.len() > KEEP_DAYS {
                history.drain(..history.len() - KEEP_DAYS);
            }
            self.state = State {
                day,
                requests: 0,
                micro_usd: 0,
                history,
            };
        }
    }

    /// Whether another request is allowed today.
    ///
    /// # Errors
    /// The limit that has been reached.
    pub fn check(&mut self, day: i64) -> Result<(), LimitReached> {
        self.roll(day);
        if self.state.requests >= self.limits.max_requests_per_day {
            return Err(LimitReached::Requests);
        }
        if let Some(cap) = self.limits.daily_spend_cap_usd {
            if self.state.micro_usd as f64 / 1_000_000.0 >= cap {
                return Err(LimitReached::Spend);
            }
        }
        Ok(())
    }

    /// Record one request and its reported cost.
    pub fn record(&mut self, day: i64, usage: Option<&Usage>) {
        self.roll(day);
        self.state.requests = self.state.requests.saturating_add(1);
        if let Some(cost) = usage
            .and_then(|u| u.cost_usd)
            .filter(|c| c.is_finite() && *c > 0.0)
        {
            self.state.micro_usd = self
                .state
                .micro_usd
                .saturating_add((cost * 1_000_000.0).round() as u64);
        }
    }

    /// The local day the counters are for, and the requests each recent day made, today last.
    #[must_use]
    pub fn request_days(&self) -> (i64, Vec<(i64, u32)>) {
        let mut days: Vec<(i64, u32)> = self
            .state
            .history
            .iter()
            .map(|p| (p.day, p.requests))
            .collect();
        if self.state.day != 0 {
            days.push((self.state.day, self.state.requests));
        }
        (self.state.day, days)
    }

    /// Requests made today.
    #[must_use]
    pub fn requests_today(&self) -> u32 {
        self.state.requests
    }

    /// Spend today, in dollars.
    #[must_use]
    pub fn spend_today(&self) -> f64 {
        self.state.micro_usd as f64 / 1_000_000.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::testutil::TempDir;

    fn usage(cost: Option<f64>) -> Usage {
        Usage {
            prompt_tokens: 10,
            completion_tokens: 5,
            cost_usd: cost,
        }
    }

    #[test]
    fn past_days_are_kept_for_the_chart() {
        let mut t = UsageTracker::new(UsageLimits::default());
        t.record(100, None);
        t.record(100, None);
        t.record(102, None);
        let (today, days) = t.request_days();
        assert_eq!(today, 102);
        assert_eq!(
            days,
            vec![(100, 2), (102, 1)],
            "a day with no requests is not listed"
        );
        for d in 103..160 {
            t.record(d, None);
        }
        assert!(
            t.request_days().1.len() <= KEEP_DAYS + 1,
            "only recent days are kept"
        );
    }

    #[test]
    fn the_request_cap_stops_requests_and_resets_the_next_day() {
        let mut t = UsageTracker::new(UsageLimits {
            max_requests_per_day: 2,
            ..UsageLimits::default()
        });
        for _ in 0..2 {
            t.check(100).unwrap();
            t.record(100, None);
        }
        assert_eq!(t.check(100), Err(LimitReached::Requests));
        assert!(t.check(101).is_ok(), "a new local day starts fresh");
        assert_eq!(t.requests_today(), 0);
    }

    #[test]
    fn the_spend_cap_uses_the_cost_the_provider_reports() {
        let mut t = UsageTracker::new(UsageLimits {
            daily_spend_cap_usd: Some(0.01),
            ..UsageLimits::default()
        });
        t.record(1, Some(&usage(Some(0.004))));
        assert!(t.check(1).is_ok());
        t.record(1, Some(&usage(Some(0.007))));
        assert_eq!(t.check(1), Err(LimitReached::Spend));
        assert!((t.spend_today() - 0.011).abs() < 1e-9);
    }

    #[test]
    fn bad_cost_values_are_ignored() {
        let mut t = UsageTracker::new(UsageLimits::default());
        for c in [f64::NAN, f64::INFINITY, -1.0] {
            t.record(1, Some(&usage(Some(c))));
        }
        assert_eq!(t.spend_today(), 0.0);
        assert_eq!(t.requests_today(), 3);
    }

    #[test]
    fn the_message_says_what_still_works() {
        assert!(
            LimitReached::Requests
                .to_string()
                .contains("Timers, alarms")
        );
    }

    #[test]
    fn counters_survive_a_restart() {
        let dir = TempDir::new("usage");
        let store = JsonStore::new(dir.0.join("usage.json"), SCHEMA);
        let mut t = UsageTracker::new(UsageLimits::default());
        t.record(7, Some(&usage(Some(0.5))));
        t.save(&store).unwrap();
        let back = UsageTracker::load(&store, UsageLimits::default()).unwrap();
        assert_eq!((back.requests_today(), back.spend_today()), (1, 0.5));
    }
}
