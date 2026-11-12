//! Per-source connection caps, shared by the HTTP and satellite listeners.
//!
//! A global connection limit alone lets one host open every slot and send nothing (threat T12). This
//! caps how many connections a single address may hold, so a flood from one machine leaves room for
//! the others. The table holds one small entry per address with an open connection and is emptied as
//! connections close, so it cannot grow past the global connection limit (REQ-NFR-110).

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

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
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
