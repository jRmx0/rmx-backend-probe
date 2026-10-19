//! Slows down password guessing.
//!
//! Failed logins are counted per username and per client IP. After a few free
//! failures, each further one makes the key wait before the next attempt,
//! doubling every time (1 s, 2 s, 4 s, ... up to 15 minutes). While a key
//! waits, logins for it get 429 without the password even being checked, so
//! the right password doesn't help an attacker who is guessing.
//!
//! - Per username: stops guessing one account from many IPs.
//! - Per IP: stops one IP from trying a few passwords on many usernames.
//!
//! Kept in memory: a restart forgets it, and several server instances would
//! each count on their own (production with more than one instance would need
//! a shared store). A downside of per-username limits: an attacker can make
//! the real user wait too. The cap keeps that to minutes.

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug)]
pub struct Policy {
    /// Failures allowed before waiting starts.
    pub free_failures: u32,
    pub first_delay: Duration,
    pub max_delay: Duration,
    /// A key with no failure for this long starts from zero again.
    pub forget_after: Duration,
}

pub const PER_USERNAME: Policy = Policy {
    free_failures: 5,
    first_delay: Duration::from_secs(1),
    max_delay: Duration::from_secs(15 * 60),
    forget_after: Duration::from_secs(60 * 60),
};

/// Looser per IP: several people can share one (mobile networks, NAT).
pub const PER_IP: Policy = Policy {
    free_failures: 20,
    ..PER_USERNAME
};

#[derive(Debug)]
struct Entry {
    failures: u32,
    last_failure: Instant,
    blocked_until: Option<Instant>,
}

#[derive(Debug)]
pub struct Limiter {
    policy: Policy,
    entries: HashMap<String, Entry>,
}

/// Prune old entries once the map is this big, so it can't grow forever.
const PRUNE_AT: usize = 10_000;

impl Limiter {
    pub fn new(policy: Policy) -> Self {
        Self {
            policy,
            entries: HashMap::new(),
        }
    }

    /// `Err(wait)` if `key` must wait before the next attempt.
    pub fn check(&self, key: &str, now: Instant) -> Result<(), Duration> {
        match self.entries.get(key).and_then(|e| e.blocked_until) {
            Some(until) if until > now => Err(until - now),
            _ => Ok(()),
        }
    }

    pub fn failure(&mut self, key: &str, now: Instant) {
        if self.entries.len() >= PRUNE_AT {
            self.prune(now);
        }
        let policy = self.policy;
        let entry = self.entries.entry(key.to_owned()).or_insert(Entry {
            failures: 0,
            last_failure: now,
            blocked_until: None,
        });
        if now.duration_since(entry.last_failure) > policy.forget_after {
            entry.failures = 0;
        }
        entry.failures += 1;
        entry.last_failure = now;
        if let Some(extra) = entry.failures.checked_sub(policy.free_failures + 1) {
            // 1st failure over the limit waits first_delay, then it doubles.
            let delay = policy
                .first_delay
                .saturating_mul(2u32.saturating_pow(extra))
                .min(policy.max_delay);
            entry.blocked_until = Some(now + delay);
        }
    }

    pub fn success(&mut self, key: &str) {
        self.entries.remove(key);
    }

    fn prune(&mut self, now: Instant) {
        let forget = self.policy.forget_after;
        self.entries.retain(|_, e| {
            now.duration_since(e.last_failure) <= forget || e.blocked_until.is_some_and(|u| u > now)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: Duration = Duration::from_secs(1);

    #[test]
    fn free_failures_then_doubling_delay() {
        let mut l = Limiter::new(PER_USERNAME);
        let t0 = Instant::now();
        for _ in 0..5 {
            assert!(l.check("u", t0).is_ok());
            l.failure("u", t0);
        }
        // Five failures are free.
        assert!(l.check("u", t0).is_ok());
        l.failure("u", t0);
        assert_eq!(l.check("u", t0), Err(S));
        assert!(l.check("u", t0 + S).is_ok());
        l.failure("u", t0 + S);
        assert_eq!(l.check("u", t0 + S), Err(2 * S));
        l.failure("u", t0 + 3 * S);
        assert_eq!(l.check("u", t0 + 3 * S), Err(4 * S));
        // Other keys are not affected.
        assert!(l.check("other", t0).is_ok());
    }

    #[test]
    fn delay_is_capped() {
        let mut l = Limiter::new(PER_USERNAME);
        let t0 = Instant::now();
        for _ in 0..100 {
            l.failure("u", t0);
        }
        assert_eq!(l.check("u", t0), Err(PER_USERNAME.max_delay));
    }

    #[test]
    fn success_and_time_reset() {
        let mut l = Limiter::new(PER_USERNAME);
        let t0 = Instant::now();
        for _ in 0..6 {
            l.failure("u", t0);
        }
        assert!(l.check("u", t0).is_err());
        l.success("u");
        assert!(l.check("u", t0).is_ok());

        for _ in 0..5 {
            l.failure("v", t0);
        }
        // After forget_after, the count starts again: the next failure is free.
        let later = t0 + PER_USERNAME.forget_after + S;
        l.failure("v", later);
        assert!(l.check("v", later).is_ok());
    }

    #[test]
    fn prune_drops_only_old_entries() {
        let mut l = Limiter::new(PER_USERNAME);
        let t0 = Instant::now();
        l.failure("old", t0);
        let later = t0 + PER_USERNAME.forget_after + S;
        l.failure("new", later);
        l.prune(later);
        assert!(!l.entries.contains_key("old"));
        assert!(l.entries.contains_key("new"));
    }
}
