//! Token-bucket rate limits: one bucket per key (a provider, a visitor).
//! Each bucket holds up to `burst` tokens and refills at `per_sec`; every
//! request takes one, and a request finding the bucket empty is refused.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Buckets kept before full (idle) ones are thrown away.
const MAX_KEYS: usize = 10_000;

pub struct Limiter {
    per_sec: f64,
    burst: f64,
    buckets: Mutex<HashMap<String, (f64, Instant)>>,
}

impl Limiter {
    pub fn new(per_sec: f64, burst: u32) -> Limiter {
        Limiter {
            per_sec,
            burst: f64::from(burst.max(1)),
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// Take a token for `key`. If there's none, how long until there is.
    pub fn take(&self, key: &str) -> Result<(), Duration> {
        self.take_at(key, Instant::now())
    }

    fn take_at(&self, key: &str, now: Instant) -> Result<(), Duration> {
        let mut buckets = self.buckets.lock().unwrap_or_else(|e| e.into_inner());
        if buckets.len() >= MAX_KEYS && !buckets.contains_key(key) {
            let (rate, burst) = (self.per_sec, self.burst);
            buckets.retain(|_, (t, at)| *t + now.duration_since(*at).as_secs_f64() * rate < burst);
        }
        let (tokens, at) = buckets
            .entry(key.to_string())
            .or_insert((self.burst, now));
        let have = (*tokens + now.duration_since(*at).as_secs_f64() * self.per_sec).min(self.burst);
        *at = now;
        if have >= 1.0 {
            *tokens = have - 1.0;
            Ok(())
        } else {
            *tokens = have;
            Err(Duration::from_secs_f64((1.0 - have) / self.per_sec))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_a_burst_then_refills() {
        let l = Limiter::new(2.0, 3);
        let t0 = Instant::now();
        assert!((0..3).all(|_| l.take_at("a", t0).is_ok()));
        let wait = l.take_at("a", t0).unwrap_err();
        assert!(wait <= Duration::from_millis(500) && wait > Duration::ZERO);
        // Other keys have their own bucket.
        assert!(l.take_at("b", t0).is_ok());
        // Half a second at 2 per second is one more token.
        let t1 = t0 + Duration::from_millis(500);
        assert!(l.take_at("a", t1).is_ok());
        assert!(l.take_at("a", t1).is_err());
        // Never more than the burst, however long it waits.
        let t2 = t1 + Duration::from_secs(3600);
        assert!((0..3).all(|_| l.take_at("a", t2).is_ok()));
        assert!(l.take_at("a", t2).is_err());
    }
}
