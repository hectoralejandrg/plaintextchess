//! The attempt limits (spec "Authentication Rate Limiting").
//!
//! A sliding window per key: the key is a client address, a device, or
//! both. The window lives in memory, so it is lost when the server restarts:
//! that is deliberate, and the limit is a guard against a burst, not a
//! durable quota.
//!
//! The check runs *before* the password is hashed, so a refused attempt
//! costs a map lookup rather than an argon2 pass.

use std::collections::HashMap;
use std::sync::Mutex;

/// How many attempts are kept for one key. The window only ever needs the
/// most recent ones.
const MAX_ATTEMPTS_PER_KEY: usize = 64;

/// How many keys the map holds before the stale ones are trimmed.
const MAX_KEYS: usize = 4096;

/// How long a key is remembered after its last attempt. It covers the longest
/// window the server uses (the registration window).
const KEY_RETENTION_MS: i64 = 2 * 60 * 60 * 1000;

/// What the limiter says about an attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateDecision {
    /// The attempt may go ahead.
    Allowed,
    /// The attempt is refused, and the player must wait this long.
    Refused { retry_after_secs: u64 },
}

/// The attempt windows.
#[derive(Debug, Default)]
pub struct RateLimiter {
    attempts: Mutex<HashMap<String, Vec<i64>>>,
}

impl RateLimiter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the key may make another attempt, recording it when it may.
    ///
    /// The check and the record happen under one lock: a burst of concurrent
    /// attempts would otherwise all pass the check before any of them was
    /// recorded, and the limit would not hold.
    ///
    /// `limit` is the number of attempts allowed inside the window; `0`
    /// means "no limit". The window is `window_ms` long.
    pub fn check_and_record(&self, key: &str, limit: u32, window_ms: i64, now_ms: i64) -> RateDecision {
        if limit == 0 {
            return RateDecision::Allowed;
        }
        let mut attempts = self.attempts.lock().unwrap_or_else(|e| e.into_inner());
        let window_start = now_ms - window_ms;
        let times = attempts.entry(key.to_string()).or_default();
        times.retain(|at| *at >= window_start);
        let recent = times.len() as u32;
        if recent >= limit {
            // The oldest attempt inside the window is the one that will fall
            // out of it first: the wait is until then.
            let oldest = times.iter().min().copied().unwrap_or(now_ms);
            let retry_after_ms = (oldest + window_ms - now_ms).max(0);
            return RateDecision::Refused {
                retry_after_secs: (retry_after_ms as u64).div_ceil(1000),
            };
        }
        times.push(now_ms);
        if times.len() > MAX_ATTEMPTS_PER_KEY {
            let excess = times.len() - MAX_ATTEMPTS_PER_KEY;
            times.drain(0..excess);
        }
        if attempts.len() > MAX_KEYS {
            // The map is bounded: the oldest keys go, whether or not they
            // have expired. A flood of keys must not grow it without limit.
            let mut keys: Vec<(String, i64)> = attempts
                .iter()
                .map(|(key, times)| (key.clone(), times.last().copied().unwrap_or(0)))
                .collect();
            keys.sort_by_key(|(_, last)| *last);
            let excess = keys.len() - MAX_KEYS;
            for (key, _) in keys.into_iter().take(excess) {
                attempts.remove(&key);
            }
        }
        RateDecision::Allowed
    }

    /// Record an attempt, so the next check sees it.
    ///
    /// The window is bounded: only the most recent attempts are kept for a
    /// key, and the key map itself is trimmed when it grows past its cap, so
    /// a client that rotates its device id cannot grow the map without
    /// limit.
    pub fn record(&self, key: &str, now_ms: i64) {
        let mut attempts = self.attempts.lock().unwrap_or_else(|e| e.into_inner());
        let times = attempts.entry(key.to_string()).or_default();
        times.push(now_ms);
        if times.len() > MAX_ATTEMPTS_PER_KEY {
            let excess = times.len() - MAX_ATTEMPTS_PER_KEY;
            times.drain(0..excess);
        }
        if attempts.len() > MAX_KEYS {
            let cutoff = now_ms - KEY_RETENTION_MS;
            attempts.retain(|_, times| times.last().is_some_and(|last| *last >= cutoff));
        }
    }

    /// Drop the attempts that fell out of their window, returning how many
    /// went. Called at startup, so a restart does not carry a stale window.
    pub fn purge(&self, now_ms: i64, window_ms: i64) -> usize {
        let mut attempts = self.attempts.lock().unwrap_or_else(|e| e.into_inner());
        let before: usize = attempts.values().map(|times| times.len()).sum();
        let window_start = now_ms - window_ms;
        for times in attempts.values_mut() {
            times.retain(|at| *at >= window_start);
        }
        attempts.retain(|_, times| !times.is_empty());
        before - attempts.values().map(|times| times.len()).sum::<usize>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUTE: i64 = 60_000;

    #[test]
    fn an_unseen_key_may_attempt() {
        let limiter = RateLimiter::new();
        assert_eq!(
            limiter.check_and_record("a", 3, MINUTE, 1_000),
            RateDecision::Allowed
        );
    }

    #[test]
    fn the_limit_is_enforced_and_then_released() {
        let limiter = RateLimiter::new();
        for i in 0..3 {
            assert_eq!(
                limiter.check_and_record("a", 3, MINUTE, 1_000 + i),
                RateDecision::Allowed,
                "attempt {i} is inside the limit"
            );
        }
        match limiter.check_and_record("a", 3, MINUTE, 1_002) {
            RateDecision::Refused { retry_after_secs } => {
                assert!(retry_after_secs > 0, "the refusal says when to try again");
            }
            RateDecision::Allowed => panic!("the fourth attempt must be refused"),
        }
        // The window slides: after a minute the first attempts fall out.
        assert_eq!(
            limiter.check_and_record("a", 3, MINUTE, 1_000 + MINUTE + 1),
            RateDecision::Allowed
        );
    }

    #[test]
    fn another_key_is_unaffected() {
        let limiter = RateLimiter::new();
        for i in 0..3 {
            limiter.check_and_record("a", 3, MINUTE, 1_000 + i);
        }
        assert_eq!(
            limiter.check_and_record("b", 3, MINUTE, 1_000),
            RateDecision::Allowed
        );
    }

    #[test]
    fn a_zero_limit_means_no_limit() {
        let limiter = RateLimiter::new();
        for i in 0..100 {
            limiter.check_and_record("a", 0, MINUTE, 1_000 + i);
        }
        assert_eq!(
            limiter.check_and_record("a", 0, MINUTE, 1_000),
            RateDecision::Allowed
        );
    }

    #[test]
    fn purging_drops_the_attempts_that_fell_out() {
        let limiter = RateLimiter::new();
        limiter.check_and_record("a", 1, MINUTE, 1_000);
        limiter.check_and_record("a", 1, MINUTE, 2_000);
        assert_eq!(limiter.purge(2_000 + MINUTE, MINUTE), 1);
        assert_eq!(
            limiter.check_and_record("a", 1, MINUTE, 2_000 + MINUTE + 1),
            RateDecision::Allowed
        );
    }

    #[test]
    fn the_key_map_is_bounded() {
        let limiter = RateLimiter::new();
        // A flood of unique keys must not grow the map without limit.
        for i in 0..(MAX_KEYS + 500) {
            limiter.check_and_record(&format!("key-{i}"), 5, MINUTE, 1_000 + i as i64);
        }
        assert!(
            limiter.attempts.lock().unwrap().len() <= MAX_KEYS,
            "the map is trimmed to its cap"
        );
    }
}
