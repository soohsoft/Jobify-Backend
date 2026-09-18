//! Per-user memory: what the service already knows about a user, made available to
//! the conversation about to happen.
//!
//! Two consumers, one source:
//!   - a new CV chat is seeded from the account's known fields instead of asking a
//!     returning user for their name again,
//!   - every turn carries a bounded context block in the system prompt so the
//!     assistant can use what it knows without re-interviewing.
//!
//! Bounded on purpose. The block rides on **every** turn and a turn is two LLM calls
//! (reply + extraction), so an unbounded memory is a per-turn tax on every
//! conversation: at the cap below it is ~300 tokens per call, which for a 60-turn CV
//! interview is about 36k extra prompt tokens (~$0.01 at peak uncached). It is
//! capped by characters, not by trusting the data to stay small.
//!
//! Kept out of the extraction prompt deliberately: the extractor reads the
//! conversation to build a CV, and a remembered fact is not something the user said
//! in this conversation. Feeding memory to it would put facts in a CV that the user
//! never confirmed here.

use mongodb::bson::doc;
use serde_json::{Value, json};

use crate::error::AppError;
use crate::models::UserDoc;
use crate::state::AppState;

/// The most a memory block may cost, in characters (~300 tokens).
const MAX_CONTEXT_CHARS: usize = 1200;
const MAX_SKILLS: usize = 8;
const MAX_EXPERIENCE: usize = 2;
const MAX_CATEGORIES: usize = 6;
const MAX_EXCLUSIONS: usize = 5;

#[derive(Debug, Default)]
pub struct UserMemory {
    /// Fields the account already knows, in the shape a CV profile uses. Empty when
    /// nothing is known.
    pub fields: Value,
    /// The prompt block. Empty when nothing is known — do not append an empty
    /// section, it only spends tokens telling the model it knows nothing.
    pub context: String,
}

/// Load a user's memory from their account and their most recent saved CV.
pub async fn load(state: &AppState, user_id: &str) -> Result<UserMemory, AppError> {
    let user = state.users().find_one(doc! { "_id": user_id }).await?;
    let Some(user) = user else {
        return Ok(UserMemory::default());
    };

    // The durable artifact is the CV, not the chat: a deleted conversation must not
    // erase what the user told us, and a saved CV is the user's own accepted version.
    let previous = state
        .resumes()
        .find_one(doc! { "user_id": user_id })
        .sort(doc! { "updated_at": -1 })
        .await?
        .map(|resume| resume.profile);

    Ok(build(&user, previous.as_ref()))
}

/// The pure part: everything here is decided from data, so it is testable without a
/// database and cannot drift from what `load` returns.
pub fn build(user: &UserDoc, previous: Option<&Value>) -> UserMemory {
    let mut fields = serde_json::Map::new();
    let mut lines: Vec<String> = Vec::new();

    let text = |value: &str| value.trim().to_string();
    let mut known = |label: &str, value: Option<&str>, key: &str| {
        let Some(value) = value.map(text).filter(|value| !value.is_empty()) else {
            return;
        };
        fields.insert(key.to_string(), json!(value));
        lines.push(format!("- {label}: {value}"));
    };

    known("Name", Some(user.name.as_str()), "fullName");
    known("Email", Some(user.email.as_str()), "email");
    known("Phone", user.phone.as_deref(), "phone");
    known("Location", user.location.as_deref(), "location");
    known("Headline", user.headline.as_deref(), "headline");

    if let Some(profile) = previous {
        let skills: Vec<String> = profile
            .get("skills")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(text)
                    .filter(|value| !value.is_empty())
                    .take(MAX_SKILLS)
                    .collect()
            })
            .unwrap_or_default();
        if !skills.is_empty() {
            fields.insert("skills".to_string(), json!(skills));
            lines.push(format!(
                "- Skills from their last CV: {}",
                skills.join(", ")
            ));
        }

        let experience: Vec<String> = profile
            .get("experience")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|entry| {
                        let title = entry.get("title").and_then(Value::as_str)?.trim();
                        let company = entry
                            .get("company")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .trim();
                        if title.is_empty() {
                            return None;
                        }
                        Some(if company.is_empty() {
                            title.to_string()
                        } else {
                            format!("{title} at {company}")
                        })
                    })
                    .take(MAX_EXPERIENCE)
                    .collect()
            })
            .unwrap_or_default();
        if !experience.is_empty() {
            lines.push(format!(
                "- Recent experience from their last CV: {}",
                experience.join("; ")
            ));
        }
    }

    if let Some(profile) = user.match_profile.as_ref() {
        let categories: Vec<String> = profile
            .categories
            .iter()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .take(MAX_CATEGORIES)
            .collect();
        if !categories.is_empty() {
            lines.push(format!(
                "- Job categories they are looking for: {}",
                categories.join(", ")
            ));
        }
    }

    if let Some(alerts) = user.alerts.as_ref() {
        let exclusions: Vec<String> = alerts
            .excluded_employers
            .iter()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .take(MAX_EXCLUSIONS)
            .collect();
        if !exclusions.is_empty() {
            lines.push(format!(
                "- Employers they asked not to see: {}",
                exclusions.join(", ")
            ));
        }
    }

    if lines.is_empty() {
        // A brand-new user: no block, no seed. The interview starts from scratch and
        // says so by asking, rather than being handed an empty "what we know" list.
        return UserMemory {
            fields: json!({}),
            context: String::new(),
        };
    }

    let mut context = String::from(
        "What you already know about this user, from their account and their previous CVs. \
         Use it instead of asking again, confirm anything that looks out of date, and never \
         invent a detail that is not listed here:\n",
    );
    for line in lines {
        if context.len() + line.len() + 1 > MAX_CONTEXT_CHARS {
            break;
        }
        context.push_str(&line);
        context.push('\n');
    }

    UserMemory {
        fields: Value::Object(fields),
        context,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user() -> UserDoc {
        UserDoc {
            id: "u1".to_string(),
            name: "Asiif Test".to_string(),
            email: "asiif@jobify.so".to_string(),
            password_hash: String::new(),
            provider: "password".to_string(),
            external_id: None,
            email_verified: false,
            avatar_url: None,
            phone: Some("+252 61 0000000".to_string()),
            headline: Some("Rust developer".to_string()),
            role: "user".to_string(),
            location: Some("Mogadishu".to_string()),
            match_profile: None,
            alerts: None,
            created_at: String::new(),
        }
    }

    #[test]
    fn a_stranger_gets_no_memory_at_all() {
        let stranger = UserDoc {
            name: String::new(),
            email: String::new(),
            phone: None,
            location: None,
            headline: None,
            ..user()
        };
        let memory = build(&stranger, None);
        // No block is better than a block that says nothing: it would spend tokens on
        // every turn telling the model it knows nothing.
        assert!(memory.context.is_empty());
        assert_eq!(memory.fields, json!({}));
    }

    #[test]
    fn unknown_fields_are_left_out_rather_than_blank() {
        let sparse = UserDoc {
            phone: None,
            headline: None,
            location: None,
            ..user()
        };
        let memory = build(&sparse, None);
        assert!(memory.context.contains("Name: Asiif Test"));
        assert!(!memory.context.contains("Phone:"));
        assert!(!memory.context.contains("Headline:"));
        assert!(memory.fields.get("phone").is_none());
    }

    #[test]
    fn a_previous_cv_contributes_skills_and_experience() {
        let previous = json!({
            "skills": ["Rust", "PostgreSQL", "Docker"],
            "experience": [{ "title": "Backend Engineer", "company": "Soohsoft" }],
        });
        let memory = build(&user(), Some(&previous));
        assert!(memory.context.contains("Rust, PostgreSQL, Docker"));
        assert!(memory.context.contains("Backend Engineer at Soohsoft"));
        assert_eq!(
            memory.fields.get("skills").unwrap(),
            &json!(["Rust", "PostgreSQL", "Docker"])
        );
    }

    #[test]
    fn the_block_is_capped_even_with_a_huge_cv() {
        let skills: Vec<String> = (0..500).map(|n| format!("skill-number-{n}")).collect();
        let previous = json!({ "skills": skills });
        let memory = build(&user(), Some(&previous));
        assert!(
            memory.context.len() <= MAX_CONTEXT_CHARS + 200,
            "block grew to {} chars",
            memory.context.len()
        );
        // Capped by dropping whole lines, never by cutting one mid-sentence.
        assert!(memory.context.ends_with('\n'));
    }

    #[test]
    fn alert_exclusions_are_carried_so_rejected_employers_are_not_reoffered() {
        let mut with_alerts = user();
        with_alerts.alerts = Some(crate::models::AlertPrefs {
            enabled: true,
            mode: "instant".to_string(),
            last_sent_at: String::new(),
            excluded_employers: vec!["A Company".to_string()],
            excluded_categories: Vec::new(),
        });
        let memory = build(&with_alerts, None);
        assert!(
            memory
                .context
                .contains("Employers they asked not to see: A Company")
        );
    }
}
