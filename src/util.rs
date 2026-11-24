pub fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Today's date (YYYY-MM-DD) in Africa/Nairobi (UTC+3, no DST).
pub fn nairobi_today() -> String {
    let nairobi = chrono::FixedOffset::east_opt(3 * 3600).expect("valid +03:00 offset");
    chrono::Utc::now()
        .with_timezone(&nairobi)
        .format("%Y-%m-%d")
        .to_string()
}

/// The Nairobi calendar date `days` days before today, as YYYY-MM-DD. Used as a
/// lexicographic lower bound: an ISO date and an ISO timestamp sort the same way by
/// date, so `created_at >= cutoff` works without parsing.
pub fn nairobi_days_ago(days: i64) -> String {
    let nairobi = chrono::FixedOffset::east_opt(3 * 3600).expect("valid +03:00 offset");
    (chrono::Utc::now().with_timezone(&nairobi) - chrono::Duration::days(days))
        .format("%Y-%m-%d")
        .to_string()
}

pub fn uuid_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Escape a user-supplied string for use inside a MongoDB `$regex`. A company name
/// containing `.` or `(` must match literally, not as a pattern.
pub fn escape_regex(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() * 2);
    for c in value.chars() {
        if ".^$*+?()[]{}|\\".contains(c) {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nairobi_today_is_iso_date() {
        let today = nairobi_today();
        assert_eq!(today.len(), 10);
        assert!(chrono::NaiveDate::parse_from_str(&today, "%Y-%m-%d").is_ok());
        assert_eq!(nairobi_days_ago(0), today);
    }

    #[test]
    fn regex_metacharacters_are_escaped() {
        assert_eq!(escape_regex("A.B (Ltd)"), "A\\.B \\(Ltd\\)");
        assert_eq!(escape_regex("plain"), "plain");
    }
}
