//! Usage limits for the language model (REQ-AST-011).
//!
//! An always-on device calling a paid API needs a guardrail: an optional spend cap per local day. (Requests
//! are counted for the activity chart; the cap on their number was retired.) When the cap is reached the
//! assistant says so and local features (timers, alarms, the fast path) keep working.
//!
//! The spend cap has to work with every provider, and most do not say what a request cost. So each model
//! request is priced by the first rule that applies:
//!
//! 1. the cost the provider reported, when it is there and above zero (`reported`);
//! 2. otherwise the tokens the provider reported times the prices the person set (`priced`);
//! 3. otherwise an estimate, `estimated`: tokens counted from the bytes sent and received, rounded up
//!    ([`BYTES_PER_TOKEN`]), and/or the built-in prices below for a side whose price is not set.
//!
//! A price set to 0 is a price ("this server is free"): it gives zero cost, labelled `priced`.

use crate::app::persist::Writer;
use crate::llm::{ChatRequest, Completion, Usage};
use crate::store::{JsonStore, StoreError};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};

/// Schema version of the persisted usage counters. The counters only ever gained optional fields, which an
/// older file lacks and reads as empty, so the version has not changed.
pub const SCHEMA: u32 = 1;

/// Price per million prompt tokens used while none is set, in US dollars. Deliberately high: the dearest
/// mainstream chat models are priced at or under this (a flagship at about $5 to $15 in, a mid-range one
/// about $1 to $3), so a person who has set nothing meets the cap early rather than late. Enter the real
/// price and the cap follows the real spend.
pub const DEFAULT_USD_PER_MTOK_IN: f64 = 10.0;
/// Price per million generated tokens used while none is set, in US dollars. Output costs four to five times
/// the input on the mainstream models (a flagship $15 to $75, a mid-range one $10 to $15); see
/// [`DEFAULT_USD_PER_MTOK_IN`] for why this is set at the top.
pub const DEFAULT_USD_PER_MTOK_OUT: f64 = 40.0;

/// Bytes counted as one token when the provider reported no token counts. English prose runs near four bytes
/// a token, JSON (the tool specifications go out with each round) and code near three, and Chinese or
/// Japanese text about three bytes for one or two tokens. Three is therefore never low for a mix of these.
pub const BYTES_PER_TOKEN: usize = 3;

/// What today's spend rests on, weakest last. The portal says "an estimate" for [`Basis::Estimated`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Basis {
    /// Every cost was reported by the provider.
    Reported,
    /// Some cost is the provider's token counts times the person's own prices.
    Priced,
    /// Some cost rests on the built-in prices or on tokens counted from sizes.
    Estimated,
}

impl Basis {
    /// The word used in the status snapshot.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Reported => "reported",
            Self::Priced => "priced",
            Self::Estimated => "estimated",
        }
    }
}

/// Configured limits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UsageLimits {
    /// Cap on generated tokens per response.
    pub max_output_tokens_per_turn: u32,
    /// Model requests allowed per local day. Never limited for a person (it is not a setting); the evaluation
    /// harness sets it to bound its own run.
    pub max_requests_per_day: u32,
    /// Spend allowed per local day, in US dollars, if set.
    pub daily_spend_cap_usd: Option<f64>,
    /// What the person pays per million prompt tokens; `None` is unset (the built-in price applies), 0 is free.
    pub llm_usd_per_mtok_in: Option<f64>,
    /// What the person pays per million generated tokens; `None` is unset, 0 is free.
    pub llm_usd_per_mtok_out: Option<f64>,
}

impl Default for UsageLimits {
    fn default() -> Self {
        Self {
            max_output_tokens_per_turn: 300,
            max_requests_per_day: u32::MAX,
            daily_spend_cap_usd: None,
            llm_usd_per_mtok_in: None,
            llm_usd_per_mtok_out: None,
        }
    }
}

/// Tokens in `bytes` when only the size is known, rounded up.
fn estimated_tokens(bytes: usize) -> f64 {
    bytes.div_ceil(BYTES_PER_TOKEN) as f64
}

/// One side (prompt or reply) of a request: its cost in millionths of a dollar and what that rests on.
/// `reported` is the provider's token count (0 when it gave none), `bytes` the size, `price` the person's.
fn side_cost(reported: u32, bytes: usize, price: Option<f64>, default: f64) -> (f64, Basis) {
    let counted = reported > 0;
    let tokens = if counted {
        f64::from(reported)
    } else {
        estimated_tokens(bytes)
    };
    if tokens == 0.0 {
        return (0.0, Basis::Reported); // nothing was sent or said: nothing to rest on
    }
    let set = price.filter(|p| p.is_finite() && *p >= 0.0);
    let per_mtok = set.unwrap_or(default);
    // dollars per million tokens times tokens is millionths of a dollar
    let micro = tokens * per_mtok;
    let basis = match set {
        None => Basis::Estimated,
        Some(p) if counted || p == 0.0 => Basis::Priced,
        Some(_) => Basis::Estimated,
    };
    (micro, basis)
}

/// The cost of one model request in millionths of a dollar, and its basis, by the rules in the module docs.
/// `sent` and `got` are the request's and the reply's sizes in bytes.
#[must_use]
pub fn price_request(
    limits: &UsageLimits,
    usage: Option<&Usage>,
    sent: usize,
    got: usize,
) -> (u64, Basis) {
    if let Some(cost) = usage
        .and_then(|u| u.cost_usd)
        .filter(|c| c.is_finite() && *c > 0.0)
    {
        return ((cost * 1_000_000.0).round() as u64, Basis::Reported);
    }
    let (in_tokens, out_tokens) = usage.map_or((0, 0), |u| (u.prompt_tokens, u.completion_tokens));
    let (a, ab) = side_cost(
        in_tokens,
        sent,
        limits.llm_usd_per_mtok_in,
        DEFAULT_USD_PER_MTOK_IN,
    );
    let (b, bb) = side_cost(
        out_tokens,
        got,
        limits.llm_usd_per_mtok_out,
        DEFAULT_USD_PER_MTOK_OUT,
    );
    // Rounded up: a request is never free for being small.
    ((a + b).ceil() as u64, ab.max(bb))
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
    /// The weakest basis any of today's cost rests on; `None` before today has cost anything. A file from
    /// before this field existed has none, which reads as unknown until the next cost is added.
    #[serde(default)]
    basis: Option<Basis>,
}

/// The limits and the counters they are held against.
#[derive(Debug)]
struct Cell {
    limits: UsageLimits,
    state: State,
}

impl Cell {
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
                basis: None,
            };
        }
    }

    /// Add `micro` millionths of a dollar that rest on `basis`.
    fn charge(&mut self, micro: u64, basis: Basis) {
        self.state.micro_usd = self.state.micro_usd.saturating_add(micro);
        self.state.basis = self.state.basis.max(Some(basis));
    }
}

/// Tracks usage against the limits.
///
/// A clone is another handle on the same counters, not a copy: the main loop and every turn task that runs
/// beside it count into one place, so the spend cap sees all of them at once (docs/adr/0021). The lock is held
/// for a few arithmetic operations and never across an await point. The main loop remains the only one that
/// saves the counters and changes the limits.
#[derive(Debug, Clone)]
pub struct UsageTracker(Arc<Mutex<Cell>>);

impl UsageTracker {
    fn cell(&self) -> std::sync::MutexGuard<'_, Cell> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A fresh tracker.
    #[must_use]
    pub fn new(limits: UsageLimits) -> Self {
        Self(Arc::new(Mutex::new(Cell {
            limits,
            state: State::default(),
        })))
    }

    /// Restore from a store.
    ///
    /// # Errors
    /// Storage errors.
    pub fn load(store: &JsonStore, limits: UsageLimits) -> Result<Self, StoreError> {
        Ok(Self(Arc::new(Mutex::new(Cell {
            limits,
            state: store.load()?,
        }))))
    }

    /// Persist.
    ///
    /// # Errors
    /// Storage errors.
    pub fn save(&self, store: &JsonStore) -> Result<(), StoreError> {
        store.save(&self.cell().state)
    }

    /// Hand the counters to the background writer instead of writing them here, so the caller does not wait for
    /// flash. The bytes are made under the writer's lock, so a save queued later never carries older counters
    /// than one queued earlier. Without a writer (tests, no storage) this saves inline.
    pub fn save_queued(&self, store: &JsonStore, writer: Option<&Writer>) {
        let Some(w) = writer else {
            if let Err(e) = self.save(store) {
                crate::diag::error(&format!("could not save usage counters: {e}"));
            }
            return;
        };
        w.submit_with(store, || store.encode(&self.cell().state));
    }

    /// Replace the limits; the counters for today are kept.
    pub fn set_limits(&self, limits: UsageLimits) {
        self.cell().limits = limits;
    }

    /// The configured limits.
    #[must_use]
    pub fn limits(&self) -> UsageLimits {
        self.cell().limits
    }

    /// Whether another request is allowed today.
    ///
    /// # Errors
    /// The limit that has been reached.
    pub fn check(&self, day: i64) -> Result<(), LimitReached> {
        let mut c = self.cell();
        c.roll(day);
        if c.state.requests >= c.limits.max_requests_per_day {
            return Err(LimitReached::Requests);
        }
        if let Some(cap) = c.limits.daily_spend_cap_usd {
            if c.state.micro_usd as f64 / 1_000_000.0 >= cap {
                return Err(LimitReached::Spend);
            }
        }
        Ok(())
    }

    /// Count one request that cost nothing we can know: it failed (a request the provider refused is not
    /// billed), or it is a realtime session, whose audio is added by [`Self::add_cost`].
    pub fn record_request(&self, day: i64) {
        let mut c = self.cell();
        c.roll(day);
        c.state.requests = c.state.requests.saturating_add(1);
    }

    /// Count one finished request and price it (see [`price_request`]). `sent` and `got` are the sizes in
    /// bytes of the request and of the reply.
    pub fn record_sized(&self, day: i64, usage: Option<&Usage>, sent: usize, got: usize) {
        let mut c = self.cell();
        c.roll(day);
        c.state.requests = c.state.requests.saturating_add(1);
        let (micro, basis) = price_request(&c.limits, usage, sent, got);
        c.charge(micro, basis);
    }

    /// Count a request that was answered, and price it from what the provider reported and the sizes.
    pub fn record_reply(&self, day: i64, req: &ChatRequest, reply: &Completion) {
        self.record_sized(
            day,
            reply.usage.as_ref(),
            req.sent_bytes(),
            reply.received_bytes(),
        );
    }

    /// Count a request that was abandoned while in flight. The provider has the prompt and may bill all
    /// of it, so the prompt is priced from its size and nothing is assumed of the reply.
    pub fn record_abandoned(&self, day: i64, req: &ChatRequest) {
        self.record_sized(day, None, req.sent_bytes(), 0);
    }

    /// Add spend that is not a model request, such as the audio time of a realtime voice session
    /// (REQ-RTV-004). The request count is not changed; the daily spend cap sees the cost. It is the
    /// measured time at the configured rates, so it counts as priced.
    pub fn add_cost(&self, day: i64, usd: f64) {
        let mut c = self.cell();
        c.roll(day);
        if usd.is_finite() && usd > 0.0 {
            c.charge((usd * 1_000_000.0).round() as u64, Basis::Priced);
        }
    }

    /// What today's spend rests on, or `None` before anything today has cost money.
    #[must_use]
    pub fn basis(&self) -> Option<Basis> {
        self.cell().state.basis
    }

    /// The local day the counters are for, and the requests each recent day made, today last.
    #[must_use]
    pub fn request_days(&self) -> (i64, Vec<(i64, u32)>) {
        let c = self.cell();
        let mut days: Vec<(i64, u32)> = c
            .state
            .history
            .iter()
            .map(|p| (p.day, p.requests))
            .collect();
        if c.state.day != 0 {
            days.push((c.state.day, c.state.requests));
        }
        (c.state.day, days)
    }

    /// Requests made today.
    #[must_use]
    pub fn requests_today(&self) -> u32 {
        self.cell().state.requests
    }

    /// Spend today, in dollars.
    #[must_use]
    pub fn spend_today(&self) -> f64 {
        self.cell().state.micro_usd as f64 / 1_000_000.0
    }
}

#[cfg(test)]
mod tests;
