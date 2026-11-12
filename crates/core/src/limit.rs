//! Per-source connection caps, shared by the HTTP and satellite listeners.
//!
//! A global connection limit alone lets one host open every slot and send nothing (threat T12). This
//! caps how many connections a single address may hold, so a flood from one machine leaves room for
//! the others. The table holds one small entry per address with an open connection and is emptied as
//! connections close, so it cannot grow past the global connection limit (REQ-NFR-110).

use crate::sched::Ts;
use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::net::IpAddr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

/// How long a listener waits after a failed accept before trying again.
pub const ACCEPT_RETRY: Duration = Duration::from_millis(200);

/// Run `accept` until it yields a connection. A failed accept is logged and tried again after `pause`: running out
/// of file descriptors, or a connection reset before it was accepted, is not a reason to stop answering for good,
/// because a flood of connections would then switch the listener off until the next restart.
pub async fn accept_retrying<T, F, Fut>(what: &str, pause: Duration, mut accept: F) -> T
where
    F: FnMut() -> Fut,
    Fut: Future<Output = io::Result<T>>,
{
    loop {
        match accept().await {
            Ok(connection) => return connection,
            Err(e) => {
                crate::diag::warn(&format!("{what}: could not accept a connection: {e}"));
                tokio::time::sleep(pause).await;
            }
        }
    }
}

/// How many connections each address may hold at once.
#[derive(Debug)]
pub struct IpLimiter {
    max: usize,
    counts: Mutex<HashMap<IpAddr, usize>>,
}

/// Held while a connection is open; releases the address's slot on drop.
#[derive(Debug)]
pub struct IpGuard {
    limiter: Arc<IpLimiter>,
    ip: IpAddr,
}

impl IpLimiter {
    /// A limiter allowing `max` simultaneous connections per address (at least 1).
    #[must_use]
    pub fn new(max: usize) -> Arc<Self> {
        Arc::new(Self {
            max: max.max(1),
            counts: Mutex::new(HashMap::new()),
        })
    }

    /// Take a slot for `ip`, or `None` when that address already holds its share.
    #[must_use]
    pub fn try_acquire(self: &Arc<Self>, ip: IpAddr) -> Option<IpGuard> {
        // An IPv4 address seen through a dual-stack socket is the same host.
        let ip = ip.to_canonical();
        let mut counts = self.counts.lock().unwrap_or_else(PoisonError::into_inner);
        let n = counts.entry(ip).or_insert(0);
        if *n >= self.max {
            return None;
        }
        *n += 1;
        Some(IpGuard {
            limiter: Arc::clone(self),
            ip,
        })
    }

    /// Addresses currently holding at least one connection.
    #[must_use]
    pub fn tracked(&self) -> usize {
        self.counts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }
}

impl Drop for IpGuard {
    fn drop(&mut self) {
        let mut counts = self
            .limiter
            .counts
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(n) = counts.get_mut(&self.ip) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                counts.remove(&self.ip);
            }
        }
    }
}

/// What one source of failed attempts, or all of them together, may spend.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FailureBudget {
    /// Failures that may come at once.
    pub burst: f64,
    /// Failures per second once the burst is spent.
    pub per_sec: f64,
}

#[derive(Debug, Clone, Copy)]
struct Tank {
    tokens: f64,
    last: Ts,
}

impl Tank {
    fn full(budget: FailureBudget, now: Ts) -> Self {
        Self {
            tokens: budget.burst,
            last: now,
        }
    }

    /// The tokens there would be at `now`.
    fn level(&self, budget: FailureBudget, now: Ts) -> f64 {
        let dt = (now - self.last).max(0) as f64;
        (self.tokens + dt * budget.per_sec).min(budget.burst)
    }

    fn refill(&mut self, budget: FailureBudget, now: Ts) {
        self.tokens = self.level(budget, now);
        self.last = now.max(self.last);
    }

    fn take(&mut self, budget: FailureBudget, now: Ts) -> bool {
        self.refill(budget, now);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    fn give_back(&mut self, budget: FailureBudget) {
        self.tokens = (self.tokens + 1.0).min(budget.burst);
    }
}

/// The key a source is counted under: an IPv4 address as it is, an IPv6 address by its /64 prefix (one subscriber
/// usually holds a whole /64, so counting each address would hand them 2^64 budgets).
fn source_key(ip: IpAddr) -> IpAddr {
    match ip.to_canonical() {
        IpAddr::V6(v6) => {
            let s = v6.segments();
            IpAddr::V6(std::net::Ipv6Addr::new(s[0], s[1], s[2], s[3], 0, 0, 0, 0))
        }
        v4 => v4,
    }
}

/// Throttles failed authentication attempts per source address, with a larger ceiling over all of them.
///
/// One shared bucket lets anyone who can reach the port empty it with bad tokens, and then the administrator's
/// right password is refused too. Here each source address (see [`source_key`]) has its own bucket, so one host
/// can only use up its own share; the shared ceiling stays as a backstop against many hosts guessing together,
/// and is sized so that no single source can reach it.
///
/// Memory is bounded: at most `max_sources` buckets exist. A bucket that has refilled to its burst says
/// nothing, so those are dropped first; when every bucket still carries a debt, the one with the most tokens
/// left is dropped (the source that has failed least recently or least often). A dropped source simply starts
/// again with a full budget, so dropping can only ever give budget, never take it from anyone: an attacker
/// cannot push an innocent address into a refusal by crowding the table. What many addresses can buy is fresh
/// budgets for themselves, each costing a failure from the shared ceiling.
#[derive(Debug)]
pub struct FailureLimiter {
    per_source: FailureBudget,
    overall: FailureBudget,
    max_sources: usize,
    sources: HashMap<IpAddr, Tank>,
    all: Tank,
}

impl FailureLimiter {
    /// A limiter starting full at `now`. `max_sources` is at least 1.
    #[must_use]
    pub fn new(
        per_source: FailureBudget,
        overall: FailureBudget,
        max_sources: usize,
        now: Ts,
    ) -> Self {
        Self {
            per_source,
            overall,
            max_sources: max_sources.max(1),
            sources: HashMap::new(),
            all: Tank::full(overall, now),
        }
    }

    /// Spend one attempt for `ip`. False when that source (or, as a backstop, everyone together) has used up its
    /// budget. A refused attempt spends nothing from the shared ceiling, so a refused source cannot drain it.
    pub fn spend(&mut self, ip: IpAddr, now: Ts) -> bool {
        let key = source_key(ip);
        if !self.sources.contains_key(&key) {
            self.make_room(now);
        }
        let per = self.per_source;
        let tank = self
            .sources
            .entry(key)
            .or_insert_with(|| Tank::full(per, now));
        if !tank.take(per, now) {
            return false;
        }
        if self.all.take(self.overall, now) {
            return true;
        }
        // The ceiling is spent: this attempt did not happen, so the source keeps its token.
        tank.give_back(per);
        false
    }

    /// Take back an attempt that turned out to be a success (the right password), so a good sign-in costs nothing.
    pub fn refund(&mut self, ip: IpAddr) {
        if let Some(t) = self.sources.get_mut(&source_key(ip)) {
            t.give_back(self.per_source);
        }
        self.all.give_back(self.overall);
    }

    /// Sources currently remembered (never more than the limit).
    #[must_use]
    pub fn tracked(&self) -> usize {
        self.sources.len()
    }

    fn make_room(&mut self, now: Ts) {
        if self.sources.len() < self.max_sources {
            return;
        }
        let per = self.per_source;
        // A refilled bucket carries no information: forget all of them.
        self.sources.retain(|_, t| t.level(per, now) < per.burst);
        if self.sources.len() < self.max_sources {
            return;
        }
        // Every source still owes something. Drop the one that owes least (ties: the one idle longest).
        let victim = self
            .sources
            .iter()
            .max_by(|a, b| {
                a.1.level(per, now)
                    .total_cmp(&b.1.level(per, now))
                    .then(b.1.last.cmp(&a.1.last))
            })
            .map(|(k, _)| *k);
        if let Some(k) = victim {
            self.sources.remove(&k);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn limiter() -> FailureLimiter {
        FailureLimiter::new(
            FailureBudget {
                burst: 20.0,
                per_sec: 1.0,
            },
            FailureBudget {
                burst: 100.0,
                per_sec: 5.0,
            },
            64,
            0,
        )
    }

    // REQ-SEC-004
    #[test]
    fn one_source_spends_only_its_own_budget() {
        let mut l = limiter();
        let a = ip("203.0.113.9");
        let b = ip("203.0.113.10");
        let mut spent = 0;
        while l.spend(a, 0) {
            spent += 1;
            assert!(spent <= 20, "never throttled");
        }
        assert_eq!(spent, 20);
        assert!(l.spend(b, 0), "another source is unaffected");
        assert!(!l.spend(a, 0));
        assert!(l.spend(a, 2), "the source recovers at its own rate");
    }

    #[test]
    fn a_refund_gives_the_attempt_back_but_never_beyond_the_burst() {
        let mut l = limiter();
        let a = ip("10.0.0.1");
        for _ in 0..20 {
            assert!(l.spend(a, 0));
        }
        assert!(!l.spend(a, 0));
        l.refund(a);
        assert!(l.spend(a, 0));
        for _ in 0..50 {
            l.refund(a);
        }
        let mut n = 0;
        while l.spend(a, 0) {
            n += 1;
        }
        assert_eq!(n, 20, "a refund never lifts a source above its burst");
    }

    #[test]
    fn the_table_stays_bounded_under_many_addresses() {
        let mut l = limiter();
        for i in 0..10_000u32 {
            let a = IpAddr::from([10, (i >> 16) as u8, (i >> 8) as u8, i as u8]);
            let _ = l.spend(a, i64::from(i) / 100);
            assert!(l.tracked() <= 64, "{} entries", l.tracked());
        }
    }

    #[test]
    fn crowding_the_table_never_costs_an_innocent_address_anything() {
        let mut l = limiter();
        let victim = ip("192.168.1.50");
        assert!(l.spend(victim, 0));
        // A host with many addresses fills the table; the victim's entry may be dropped, which hands it a fresh
        // budget. It is never worse off than before (19 attempts left).
        for i in 0..1000u32 {
            let a = IpAddr::from([10, 0, (i >> 8) as u8, i as u8]);
            let _ = l.spend(a, i64::from(i) / 5);
        }
        assert!(
            l.spend(victim, 200),
            "the crowd cannot have used the victim's budget"
        );
    }

    #[test]
    fn many_sources_together_hit_the_backstop_but_one_source_cannot() {
        let mut l = limiter();
        // One source, however hard it tries, stops long before the ceiling.
        let mut one = 0;
        for t in 0..600 {
            for _ in 0..50 {
                if l.spend(ip("203.0.113.1"), t) {
                    one += 1;
                }
            }
        }
        assert!(one <= 20 + 600, "{one}");
        assert!(
            l.spend(ip("203.0.113.2"), 600),
            "the ceiling was not reached by one source"
        );
        // Many sources spend the shared ceiling.
        let mut l = limiter();
        let mut allowed = 0;
        for i in 0..64u32 {
            let a = IpAddr::from([198, 51, 100, i as u8]);
            for _ in 0..20 {
                if l.spend(a, 0) {
                    allowed += 1;
                }
            }
        }
        assert_eq!(allowed, 100, "the shared ceiling is the burst of 100");
        assert!(!l.spend(ip("198.51.100.200"), 0), "and then nobody gets in");
    }

    #[test]
    fn a_refused_source_does_not_drain_the_shared_ceiling() {
        let mut l = limiter();
        let a = ip("203.0.113.1");
        for _ in 0..20 {
            assert!(l.spend(a, 0));
        }
        for _ in 0..1000 {
            assert!(!l.spend(a, 0));
        }
        let mut others = 0;
        for i in 0..5u32 {
            let b = IpAddr::from([198, 51, 100, i as u8]);
            for _ in 0..16 {
                if l.spend(b, 0) {
                    others += 1;
                }
            }
        }
        assert_eq!(others, 80, "100 minus the 20 the first source really spent");
    }

    #[test]
    fn mapped_v4_is_the_same_source_and_a_v6_prefix_is_one_source() {
        let mut l = limiter();
        for _ in 0..10 {
            assert!(l.spend(ip("192.0.2.5"), 0));
        }
        for _ in 0..10 {
            assert!(l.spend(ip("::ffff:192.0.2.5"), 0));
        }
        assert!(!l.spend(ip("192.0.2.5"), 0));
        let mut l = limiter();
        for i in 0..20u16 {
            assert!(l.spend(ip(&format!("2001:db8:1:2::{i:x}")), 0));
        }
        assert!(
            !l.spend(ip("2001:db8:1:2:ffff::1"), 0),
            "one /64 shares a budget"
        );
        assert!(l.spend(ip("2001:db8:1:3::1"), 0), "another /64 does not");
    }

    #[test]
    fn one_address_cannot_take_more_than_its_share() {
        let l = IpLimiter::new(2);
        let a = l.try_acquire(ip("192.168.1.5")).unwrap();
        let _b = l.try_acquire(ip("192.168.1.5")).unwrap();
        assert!(l.try_acquire(ip("192.168.1.5")).is_none());
        assert!(
            l.try_acquire(ip("192.168.1.6")).is_some(),
            "other hosts are unaffected"
        );
        drop(a);
        assert!(
            l.try_acquire(ip("192.168.1.5")).is_some(),
            "a closed connection frees its slot"
        );
    }

    #[test]
    fn the_table_empties_as_connections_close() {
        let l = IpLimiter::new(3);
        let guards: Vec<_> = (1..=3)
            .filter_map(|i| l.try_acquire(ip(&format!("10.0.0.{i}"))))
            .collect();
        assert_eq!(l.tracked(), 3);
        drop(guards);
        assert_eq!(l.tracked(), 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_failed_accept_is_retried_not_fatal() {
        let mut failures = 2;
        let mut calls = 0;
        let got = accept_retrying("test", Duration::from_millis(5), || {
            calls += 1;
            let result = if failures > 0 {
                failures -= 1;
                Err(io::Error::other("too many open files"))
            } else {
                Ok("conn")
            };
            async move { result }
        })
        .await;
        assert_eq!(got, "conn");
        assert_eq!(calls, 3);
    }

    #[test]
    fn a_mapped_ipv4_address_counts_as_the_same_host() {
        let l = IpLimiter::new(1);
        let _g = l.try_acquire(ip("192.168.1.5")).unwrap();
        assert!(l.try_acquire(ip("::ffff:192.168.1.5")).is_none());
    }
}
