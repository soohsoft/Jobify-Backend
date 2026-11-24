//! Email one-time codes: generation, storage form, and the rules a code must satisfy.
//!
//! The code is a short-lived secret, so only its hash is stored and the raw value exists
//! exactly once — in the email. Everything here is a pure function so the rules can be
//! tested without a database or an SMTP server.

/// How long a signup code stays valid.
pub const OTP_TTL_MINUTES: i64 = 10;

/// A reset code gets longer: the user has to find the mail and pick a new password, and
/// reset mail often sits unread. The extra window is paid for by single use + attempts.
pub const RESET_TTL_MINUTES: i64 = 30;

/// Codes are per purpose, never shared: a code that verifies an address must not be able
/// to change a password.
pub const PURPOSE_VERIFY: &str = "verify_email";
pub const PURPOSE_RESET: &str = "reset_password";

/// The storage key. One row per (user, purpose), so asking for a reset does not
/// invalidate a pending signup code.
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

/// Seconds before another code may be sent. Without this, "resend" is a free mail bomb
/// aimed at any address someone types in.
pub const OTP_RESEND_COOLDOWN_SECONDS: i64 = 60;

/// A 6-digit code from OS randomness. `uuid` v4 bytes come from the same CSPRNG that
/// guards ids, so this needs no new crate.
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

/// Constant-time comparison, so a wrong guess cannot be narrowed by response timing.
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

pub fn expires_in(now: chrono::DateTime<chrono::Utc>, minutes: i64) -> String {
    (now + chrono::Duration::minutes(minutes)).to_rfc3339()
}

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

/// `a***z@example.com` — enough to show which address was mailed, without echoing the
/// whole thing into logs and screens.
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
    fn codes_are_six_digits_and_vary() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..50 {
            let code = generate_code();
            assert_eq!(code.len(), 6);
            assert!(code.chars().all(|c| c.is_ascii_digit()));
            seen.insert(code);
        }
        assert!(seen.len() > 20);
    }

    #[test]
    fn hashing_is_stable_and_matching_tolerates_whitespace() {
        assert_eq!(hash_code("123456"), hash_code("123456"));
        assert_ne!(hash_code("123456"), hash_code("123457"));
        assert_eq!(hash_code("123456").len(), 64);
        assert!(code_matches(&hash_code("123456"), " 123456 "));
        assert!(!code_matches(&hash_code("000000"), "111111"));
    }

    #[test]
    fn expiry_and_cooldown() {
        let now = chrono::Utc::now();
        assert!(!is_expired(&expires_in(now, OTP_TTL_MINUTES), now));
        assert!(is_expired(&expires_in(now, -1), now));
        assert!(is_expired("not-a-date", now));
        assert!(resend_wait_seconds(&now.to_rfc3339(), now) > 50);
        assert_eq!(
            resend_wait_seconds(&(now - chrono::Duration::minutes(5)).to_rfc3339(), now),
            0
        );
        assert_eq!(resend_wait_seconds("broken", now), 0);
    }

    #[test]
    fn purposes_get_separate_storage_keys() {
        assert_ne!(
            otp_key("user-1", PURPOSE_VERIFY),
            otp_key("user-1", PURPOSE_RESET)
        );
        assert_eq!(otp_key("user-1", PURPOSE_VERIFY), "user-1:verify_email");
    }

    #[test]
    fn password_floor_and_masked_email() {
        assert!(validate_password("12345678").is_ok());
        assert!(validate_password("1234567").is_err());
        assert_eq!(mask_email("halima.warsame@gmail.com"), "h***e@gmail.com");
        assert_eq!(mask_email("nonsense"), "***");
    }
}
