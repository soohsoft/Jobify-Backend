//! One-time portal links.
//!
//! The bot mints a link; the person opens it, and `POST /auth/magic` trades the raw token
//! for a session. Only the SHA-256 of the token is ever stored, so the database (and an
//! admin reading it) cannot be used to enter someone's account. The raw value is returned
//! to the bot exactly once and is unrecoverable afterwards.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

/// 32 random bytes as 64 hex characters.
///
/// `uuid` v4 is used as the randomness source rather than pulling in another crate: its
/// bytes come from `getrandom` (the OS CSPRNG). Two UUIDs are 32 bytes exactly, and 122
/// bits per UUID is 244 bits of entropy — far beyond guessing range for a 15-minute link.
pub fn new_token() -> String {
    let mut bytes = [0u8; 32];
    bytes[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    bytes[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    hex::encode(bytes)
}

/// What the database stores: `_id` for a link token row.
pub fn token_fingerprint(raw: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    hex::encode(hasher.finalize())
}

/// A coarse in-memory attempt limiter, keyed by caller IP.
///
/// Deliberately loose and deliberately in-memory: its job is to stop one client from
/// turning `POST /auth/magic` into a token-guessing service, not to be a per-user account
/// policy. A restart forgets the window, which is fine because the tokens themselves are
/// single-use and short-lived, and every attempt is logged either way.
pub struct RateLimiter {
    window: Duration,
    max: usize,
    seen: Mutex<HashMap<String, Vec<Instant>>>,
}

impl RateLimiter {
    pub fn new(window: Duration, max: usize) -> Self {
        Self {
            window,
            max,
            seen: Mutex::new(HashMap::new()),
        }
    }

    /// Records an attempt and answers whether it may proceed.
    pub fn check(&self, key: &str) -> bool {
        // A poisoned lock means another thread panicked while holding it; the map is still
        // structurally sound, so recover rather than failing every request forever.
        let mut seen = self.seen.lock().unwrap_or_else(|err| err.into_inner());
        let now = Instant::now();
        let attempts = seen.entry(key.to_string()).or_default();
        attempts.retain(|at| now.duration_since(*at) < self.window);
        if attempts.len() >= self.max {
            return false;
        }
        attempts.push(now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_32_bytes_of_hex_and_never_repeat() {
        let a = new_token();
        let b = new_token();
        assert_eq!(a.len(), 64, "32 bytes hex-encoded");
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn the_fingerprint_is_stable_and_does_not_contain_the_token() {
        let raw = new_token();
        let digest = token_fingerprint(&raw);
        assert_eq!(digest, token_fingerprint(&raw));
        assert_eq!(digest.len(), 64);
        assert!(!digest.contains(&raw));
        assert_ne!(digest, token_fingerprint(&new_token()));
    }

    #[test]
    fn the_limiter_allows_up_to_the_cap_then_refuses() {
        let limiter = RateLimiter::new(Duration::from_secs(60), 3);
        assert!(limiter.check("1.2.3.4"));
        assert!(limiter.check("1.2.3.4"));
        assert!(limiter.check("1.2.3.4"));
        assert!(!limiter.check("1.2.3.4"));
        // A different caller has its own window.
        assert!(limiter.check("5.6.7.8"));
    }
}
