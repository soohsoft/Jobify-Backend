use serde_json::{Map, Value};

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

/// Recursively merge `incoming` into `base`. Arrays and primitives in
/// `incoming` replace the corresponding value in `base`.
#[allow(dead_code)]
pub fn merge_value(base: &mut Value, incoming: Value) {
    match incoming {
        Value::Object(incoming_map) => {
            if let Value::Object(base_map) = base {
                for (key, value) in incoming_map {
                    match base_map.get_mut(&key) {
                        Some(base_value) => merge_value(base_value, value),
                        None => {
                            base_map.insert(key, value);
                        }
                    }
                }
            } else {
                *base = Value::Object(incoming_map);
            }
        }
        other => {
            *base = other;
        }
    }
}

#[allow(dead_code)]
pub fn json_object() -> Map<String, Value> {
    Map::new()
}

/// Extract a JSON object from an LLM response, tolerating fenced code blocks
/// and surrounding prose.
pub fn extract_json(text: &str) -> Result<Value, crate::error::AppError> {
    let mut cleaned = text.trim();
    if let Some(stripped) = cleaned
        .strip_prefix("```json")
        .or_else(|| cleaned.strip_prefix("```"))
    {
        cleaned = stripped.trim_start();
    }
    if let Some(stripped) = cleaned.strip_suffix("```") {
        cleaned = stripped.trim_end();
    }

    if let Ok(value) = serde_json::from_str::<Value>(cleaned) {
        return Ok(value);
    }

    let start = cleaned.find('{');
    let end = cleaned.rfind('}');
    if let (Some(start), Some(end)) = (start, end)
        && end > start
        && let Ok(value) = serde_json::from_str::<Value>(&cleaned[start..=end])
    {
        return Ok(value);
    }

    Err(crate::error::AppError::BadGateway(
        "Failed to parse profile data from LLM".to_string(),
    ))
}

/// A value is "meaningful" when it is non-null, non-empty and has content.
pub fn is_meaningful(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Array(items) => !items.is_empty(),
        Value::String(text) => !text.trim().is_empty(),
        Value::Object(map) => !map.is_empty(),
        _ => true,
    }
}

/// Merge meaningful fields from `incoming` into `base`, returning a new value.
pub fn merge_profile(base: &Value, incoming: &Value) -> Value {
    let mut merged = base.clone();
    if let Value::Object(incoming_map) = incoming
        && let Value::Object(base_map) = &mut merged
    {
        for (key, value) in incoming_map {
            if is_meaningful(value) {
                base_map.insert(key.clone(), value.clone());
            }
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nairobi_today_is_iso_date() {
        let today = nairobi_today();
        assert_eq!(today.len(), 10, "expected YYYY-MM-DD, got {today}");
        assert!(chrono::NaiveDate::parse_from_str(&today, "%Y-%m-%d").is_ok());
    }
}
