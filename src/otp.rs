//! Email one-time codes: generation, storage form, and the rules a code must satisfy.
//!
//! The code is a short-lived secret, so only its hash is stored and the raw value exists
//! exactly once — in the email. Everything here is a pure function so the rules can be
//! tested without a database or an SMTP server.

/// How long a signup code stays valid. Long enough to fetch a mail, short enough that a
/// leaked mailbox is not a standing key.
pub const OTP_TTL_MINUTES: i64 = 10;

/// A reset code gets longer: the user has to remember they asked for one, find the mail,
/// and choose a new password, and a reset mail often sits unread for a while. The extra
/// window is paid for by the code still being single-use and attempt-limited.
pub const RESET_TTL_MINUTES: i64 = 30;

/// Codes are per purpose, never shared: a code that verifies an address must not be able to
/// change a password, or a leaked signup mail becomes an account takeover.
pub const PURPOSE_VERIFY: &str = "verify_email";
pub const PURPOSE_RESET: &str = "reset_password";

/// The storage key. One row per (user, purpose), so issuing a new code of a kind replaces
/// that kind only — asking for a reset does not invalidate a pending signup code.
pub fn otp_key(user_id: &str, purpose: &str) -> String {
    format!("{user_id}:{purpose}")
}

pub fn ttl_minutes(purpose: &str) -> i64 {
    match purpose {
        PURPOSE_RESET => RESET_TTL_MINUTES,
        _ => OTP_TTL_MINUTES,
    }
}

/// Wrong guesses allowed before the code is burned. A 6-digit code has 10^6 values; five
/// attempts keeps guessing hopeless while leaving room for a typo.
pub const OTP_MAX_ATTEMPTS: i32 = 5;

/// Seconds a user must wait before another code will be sent. Without this, "resend"
/// is a free mail bomb aimed at any address someone types in.
pub const OTP_RESEND_COOLDOWN_SECONDS: i64 = 60;

/// A 6-digit code from OS randomness. `uuid` is already a dependency and its v4 bytes come
/// from the same CSPRNG that guards ids, so this needs no new crate.
pub fn generate_code() -> String {
    let bytes = uuid::Uuid::new_v4().into_bytes();
    let n = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) % 1_000_000;
    format!("{n:06}")
}

/// What gets stored: the code is never written down in the clear.
pub fn hash_code(code: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(code.trim().as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Constant-time comparison, so a wrong guess cannot be narrowed down by response timing.
pub fn code_matches(stored_hash: &str, candidate: &str) -> bool {
    let candidate = hash_code(candidate);
    if stored_hash.len() != candidate.len() {
        return false;
    }
    stored_hash
        .bytes()
        .zip(candidate.bytes())
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

pub fn expires_at(now: chrono::DateTime<chrono::Utc>) -> String {
    expires_in(now, OTP_TTL_MINUTES)
}

pub fn expires_in(now: chrono::DateTime<chrono::Utc>, minutes: i64) -> String {
    (now + chrono::Duration::minutes(minutes)).to_rfc3339()
}

/// Passwords arrive from a public form, so the server enforces a floor rather than trusting
/// the client's own check. 8 characters matches what the signup screen already requires.
pub const MIN_PASSWORD_LENGTH: usize = 8;

pub fn validate_password(password: &str) -> Result<(), String> {
    if password.chars().count() < MIN_PASSWORD_LENGTH {
        return Err(format!(
            "Password must be at least {MIN_PASSWORD_LENGTH} characters"
        ));
    }
    Ok(())
}

pub fn is_expired(expires_at: &str, now: chrono::DateTime<chrono::Utc>) -> bool {
    match chrono::DateTime::parse_from_rfc3339(expires_at) {
        Ok(at) => at.with_timezone(&chrono::Utc) <= now,
        // An unparseable expiry is treated as expired: fail closed on a security check.
        Err(_) => true,
    }
}

/// Cooldown gate for resend, in seconds still to wait (0 = may send).
pub fn resend_wait_seconds(last_sent_at: &str, now: chrono::DateTime<chrono::Utc>) -> i64 {
    match chrono::DateTime::parse_from_rfc3339(last_sent_at) {
        Ok(at) => {
            let elapsed = (now - at.with_timezone(&chrono::Utc)).num_seconds();
            (OTP_RESEND_COOLDOWN_SECONDS - elapsed).max(0)
        }
        // No usable timestamp: allow the send rather than lock the user out.
        Err(_) => 0,
    }
}

/// `a***z@example.com` — enough for the client to show which address it mailed, without
/// echoing the whole thing back into logs and screens.
pub fn mask_email(email: &str) -> String {
    match email.split_once('@') {
        Some((local, domain)) if !local.is_empty() => {
            let first = local.chars().next().unwrap_or('*');
            let last = local.chars().last().unwrap_or('*');
            format!("{first}***{last}@{domain}")
        }
        _ => "***".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_six_digits() {
        for _ in 0..200 {
            let code = generate_code();
            assert_eq!(code.len(), 6, "code {code} is not 6 characters");
            assert!(code.chars().all(|c| c.is_ascii_digit()));
        }
    }

    #[test]
    fn codes_vary() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..50 {
            seen.insert(generate_code());
        }
        assert!(seen.len() > 20, "codes look constant: {seen:?}");
    }

    #[test]
    fn identical_codes_hash_identically_and_others_do_not() {
        let h = hash_code("123456");
        assert_eq!(h, hash_code("123456"));
        assert_ne!(h, hash_code("123457"));
        assert_eq!(h.len(), 64);
    }

    #[test]
    fn whitespace_around_a_code_is_tolerated() {
        assert!(code_matches(&hash_code("123456"), " 123456 "));
    }

    #[test]
    fn wrong_code_never_matches() {
        let stored = hash_code("000000");
        assert!(!code_matches(&stored, "111111"));
        assert!(!code_matches(&stored, ""));
        assert!(!code_matches(&stored, "0000000"));
    }

    #[test]
    fn expiry_is_inclusive_and_fails_closed() {
        let now = chrono::Utc::now();
        assert!(!is_expired(&expires_at(now), now));
        assert!(is_expired(
            &expires_at(now - chrono::Duration::minutes(11)),
            now
        ));
        assert!(
            is_expired("not-a-date", now),
            "unparseable expiry must be treated as expired"
        );
    }

    #[test]
    fn resend_cooldown_counts_down_then_releases() {
        let now = chrono::Utc::now();
        let just_sent = now.to_rfc3339();
        assert!(resend_wait_seconds(&just_sent, now) > 50);
        let long_ago = (now - chrono::Duration::minutes(5)).to_rfc3339();
        assert_eq!(resend_wait_seconds(&long_ago, now), 0);
        assert_eq!(
            resend_wait_seconds("broken", now),
            0,
            "a bad timestamp must not block the user"
        );
    }

    #[test]
    fn a_reset_code_lives_longer_than_a_signup_code() {
        assert_eq!(ttl_minutes(PURPOSE_VERIFY), OTP_TTL_MINUTES);
        assert_eq!(ttl_minutes(PURPOSE_RESET), RESET_TTL_MINUTES);
        assert!(RESET_TTL_MINUTES > OTP_TTL_MINUTES);
        let now = chrono::Utc::now();
        assert!(!is_expired(&expires_at(now), now));
        assert!(!is_expired(&expires_in(now, RESET_TTL_MINUTES), now));
    }

    #[test]
    fn purposes_get_separate_storage_keys() {
        let verify = otp_key("user-1", PURPOSE_VERIFY);
        let reset = otp_key("user-1", PURPOSE_RESET);
        assert_ne!(verify, reset, "one purpose must not overwrite the other");
        assert_eq!(verify, "user-1:verify_email");
        assert_eq!(reset, "user-1:reset_password");
    }

    #[test]
    fn password_floor_is_enforced() {
        assert!(validate_password("12345678").is_ok());
        assert!(validate_password("1234567").is_err());
        assert!(validate_password("").is_err());
        assert!(validate_password("a-very-long-passphrase").is_ok());
    }

    #[test]
    fn email_is_masked_but_still_recognisable() {
        assert_eq!(mask_email("halima.warsame@gmail.com"), "h***e@gmail.com");
        assert_eq!(mask_email("a@b.com"), "a***a@b.com");
        assert_eq!(mask_email("nonsense"), "***");
    }
}
