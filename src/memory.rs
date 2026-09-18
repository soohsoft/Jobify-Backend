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
use crate::models::{MemoryExperience, UserDoc, UserMemoryDoc};
use crate::state::AppState;
use crate::util::now_iso;

/// The most a memory block may cost, in characters (~300 tokens).
const MAX_CONTEXT_CHARS: usize = 1200;
const MAX_SKILLS: usize = 8;
const MAX_EXPERIENCE: usize = 2;
const MAX_CATEGORIES: usize = 6;
const MAX_EXCLUSIONS: usize = 5;

#[derive(Debug, Default)]
pub struct UserMemory {
    /// Whether the account already has match categories. The opening intake prompt is
    /// only appended while this is false — once we know the work they want, asking
    /// again would be the interrogation this replaced.
    pub has_categories: bool,
    /// Fields the account already knows, in the shape a CV profile uses. Empty when
    /// nothing is known.
    pub fields: Value,
    /// The prompt block. Empty when nothing is known — do not append an empty
    /// section, it only spends tokens telling the model it knows nothing.
    pub context: String,
}

/// Load a user's memory from their account and their most recent saved CV.
pub async fn load(state: &AppState, user_id: &str) -> Result<UserMemory, AppError> {
    // The stored record is the authority once it exists: one read per turn instead of a
    // join over the account and the newest CV, and it is the same text a future consumer
    // (a digest, a bot, a support screen) would read.
    if let Some(record) = load_stored(state, user_id).await? {
        return Ok(from_record(&record));
    }

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
            has_categories: user
                .match_profile
                .as_ref()
                .is_some_and(|profile| !profile.categories.is_empty()),
        };
    }

    let has_categories = user
        .match_profile
        .as_ref()
        .is_some_and(|profile| !profile.categories.is_empty());

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
        has_categories,
    }
}

/// The prompt-facing view of a stored record: the seed a new CV chat starts from, and
/// the context block for every turn.
fn from_record(record: &UserMemoryDoc) -> UserMemory {
    let mut fields = serde_json::Map::new();
    let mut put = |key: &str, value: Option<&str>| {
        if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) {
            fields.insert(key.to_string(), json!(value));
        }
    };
    put("fullName", Some(record.name.as_str()));
    put("email", Some(record.email.as_str()));
    put("phone", record.phone.as_deref());
    put("location", record.location.as_deref());
    put("headline", record.headline.as_deref());
    if !record.skills.is_empty() {
        fields.insert("skills".to_string(), json!(record.skills));
    }
    if !record.languages.is_empty() {
        fields.insert("languages".to_string(), json!(record.languages));
    }
    if !record.experience.is_empty() {
        fields.insert(
            "experience".to_string(),
            json!(
                record
                    .experience
                    .iter()
                    .map(|entry| json!({
                        "title": entry.title,
                        "company": entry.organization,
                        "startDate": entry.period,
                    }))
                    .collect::<Vec<_>>()
            ),
        );
    }
    UserMemory {
        has_categories: !record.categories.is_empty(),
        context: prompt_block(record),
        fields: Value::Object(fields),
    }
}

// ---------------------------------------------------------------------------
// The stored record
// ---------------------------------------------------------------------------

/// Caps. The record is a summary, and a summary that grows without limit stops being
/// one: it is read on every turn, mailed in digests and shown to humans. Everything
/// here is "keep the newest, drop the rest", never "keep everything and hope".
const MAX_RECORD_SKILLS: usize = 12;
const MAX_RECORD_KEYWORDS: usize = 12;
const MAX_RECORD_EXPERIENCE: usize = 3;
const MAX_RECORD_MEMO_CHARS: usize = 700;

/// Rebuild and store a user's memory record.
///
/// Called by whatever just learned something — the end of a chat turn, a profile edit, a
/// CV being saved. It is a REBUILD from the durable sources plus a merge with what was
/// stored before, in that order of authority:
///
///   1. the account (name, contact, location, headline — the user's own choice),
///   2. this chat's extracted profile (freshest, richest: the interview's own words),
///   3. the user's newest saved CV,
///   4. the previously stored record — which is what keeps skills and history alive
///      after the chat or the CV they came from is deleted.
///
/// `extra_profile` is the current conversation's profile, so a fact learned in a chat is
/// remembered even if the user never saves a CV from it.
pub async fn refresh(
    state: &AppState,
    user_id: &str,
    source: &str,
    extra_profile: Option<&Value>,
) -> Result<UserMemoryDoc, AppError> {
    let Some(user) = state.users().find_one(doc! { "_id": user_id }).await? else {
        return Err(AppError::NotFound("User not found".to_string()));
    };
    let stored = state
        .user_memory()
        .find_one(doc! { "_id": user_id })
        .await?;
    let newest_cv = state
        .resumes()
        .find_one(doc! { "user_id": user_id })
        .sort(doc! { "updated_at": -1 })
        .await?
        .map(|resume| resume.profile);

    let now = now_iso();
    let mut record = stored.unwrap_or_else(|| UserMemoryDoc {
        id: user_id.to_string(),
        created_at: now.clone(),
        ..Default::default()
    });

    // 1. Identity always mirrors the account.
    record.name = user.name.trim().to_string();
    record.email = user.email.trim().to_string();
    record.phone = clean(user.phone.as_deref());
    record.location = clean(user.location.as_deref());
    record.headline = clean(user.headline.as_deref());

    // 2-4. Professional shape, merged oldest-to-newest so the freshest wording wins,
    //      then topped up from what is already stored (in case the sources are gone).
    let mut skills = Vec::new();
    let mut languages = Vec::new();
    let mut experience: Vec<MemoryExperience> = Vec::new();
    let mut education: Option<String> = None;

    for profile in [
        stored_cv_profile(&record),
        newest_cv,
        extra_profile.cloned(),
    ]
    .into_iter()
    .flatten()
    {
        collect(
            &profile,
            &mut skills,
            &mut languages,
            &mut experience,
            &mut education,
        );
    }
    collect_from_record(&record, &mut skills, &mut languages, &mut experience);

    dedupe_cap(&mut skills, MAX_RECORD_SKILLS);
    dedupe_cap(&mut languages, MAX_RECORD_SKILLS);
    record.experience = dedupe_experience(experience, MAX_RECORD_EXPERIENCE);
    record.education = education.or_else(|| record.education.clone());
    record.skills = skills;
    record.languages = languages;

    // Categories and keywords: the account's match profile is the matcher's source of
    // truth; the record mirrors it so one read answers everything.
    if let Some(profile) = user.match_profile.as_ref() {
        if !profile.categories.is_empty() {
            record.categories = profile.categories.clone();
        }
        if !profile.keywords.is_empty() {
            dedupe_cap(&mut profile.keywords.clone(), MAX_RECORD_KEYWORDS);
            record.keywords = capped_unique(profile.keywords.clone(), MAX_RECORD_KEYWORDS);
        }
        if !profile.locations.is_empty() {
            record.preferred_locations = profile.locations.clone();
        }
    }
    if let Some(location) = record.location.clone()
        && !record
            .preferred_locations
            .iter()
            .any(|value| value == &location)
    {
        record.preferred_locations.insert(0, location);
    }
    record.preferred_locations.truncate(MAX_RECORD_KEYWORDS);
    if let Some(alerts) = user.alerts.as_ref() {
        record.avoid_employers = alerts.excluded_employers.clone();
    }

    let source = source.to_string();
    if record.sources.last() != Some(&source) {
        record.sources.push(source);
        // A record that lists every chat it ever learned from is a log, not a summary.
        let keep = record.sources.len().saturating_sub(5);
        record.sources.drain(..keep);
    }
    record.revision = record.revision.saturating_add(1);
    record.updated_at = now;
    record.memo = compact_memo(&record);

    state
        .user_memory()
        .replace_one(doc! { "_id": user_id }, &record)
        .upsert(true)
        .await?;

    Ok(record)
}

fn clean(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// Pull skills / languages / experience / education out of a CV-shaped profile object.
fn collect(
    profile: &Value,
    skills: &mut Vec<String>,
    languages: &mut Vec<String>,
    experience: &mut Vec<MemoryExperience>,
    education: &mut Option<String>,
) {
    if let Some(values) = profile.get("skills").and_then(Value::as_array) {
        skills.extend(
            values
                .iter()
                .filter_map(Value::as_str)
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
        );
    }
    if let Some(values) = profile.get("languages").and_then(Value::as_array) {
        languages.extend(
            values
                .iter()
                .filter_map(Value::as_str)
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
        );
    }
    if let Some(entries) = profile.get("experience").and_then(Value::as_array) {
        for entry in entries {
            let title = entry
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim();
            let organization = entry
                .get("company")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim();
            if title.is_empty() && organization.is_empty() {
                continue;
            }
            experience.push(MemoryExperience {
                title: title.to_string(),
                organization: organization.to_string(),
                period: entry
                    .get("startDate")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim()
                    .to_string(),
            });
        }
    }
    if education.is_none()
        && let Some(first) = profile
            .get("education")
            .and_then(Value::as_array)
            .and_then(|entries| entries.first())
    {
        let degree = first
            .get("degree")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        let institution = first
            .get("institution")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if !degree.is_empty() || !institution.is_empty() {
            *education = Some(match (degree.is_empty(), institution.is_empty()) {
                (false, false) => format!("{degree} — {institution}"),
                (false, true) => degree.to_string(),
                _ => institution.to_string(),
            });
        }
    }
}

/// What the record already knew, used as a floor so a deleted chat or CV cannot erase it.
fn collect_from_record(
    record: &UserMemoryDoc,
    skills: &mut Vec<String>,
    languages: &mut Vec<String>,
    _experience: &mut Vec<MemoryExperience>,
) {
    skills.extend(record.skills.iter().cloned());
    languages.extend(record.languages.iter().cloned());
}

fn capped_unique(values: Vec<String>, cap: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for value in values {
        let value = value.trim().to_string();
        if value.is_empty() || out.iter().any(|seen| seen.eq_ignore_ascii_case(&value)) {
            continue;
        }
        out.push(value);
        if out.len() >= cap {
            break;
        }
    }
    out
}

fn dedupe_cap(values: &mut Vec<String>, cap: usize) {
    *values = capped_unique(std::mem::take(values), cap);
}

fn dedupe_experience(values: Vec<MemoryExperience>, cap: usize) -> Vec<MemoryExperience> {
    let mut out: Vec<MemoryExperience> = Vec::new();
    for entry in values.into_iter().rev() {
        if entry.title.is_empty() && entry.organization.is_empty() {
            continue;
        }
        if out.iter().any(|seen| {
            seen.title.eq_ignore_ascii_case(&entry.title)
                && seen.organization.eq_ignore_ascii_case(&entry.organization)
        }) {
            continue;
        }
        out.push(entry);
        if out.len() >= cap {
            break;
        }
    }
    out
}

fn stored_cv_profile(record: &UserMemoryDoc) -> Option<Value> {
    if record.skills.is_empty() && record.experience.is_empty() {
        return None;
    }
    Some(json!({
        "skills": record.skills,
        "languages": record.languages,
        "experience": record.experience.iter().map(|entry| json!({
            "title": entry.title,
            "company": entry.organization,
            "startDate": entry.period,
        })).collect::<Vec<_>>(),
        "education": record.education.clone().map(|line| json!([{ "degree": line }])).unwrap_or(Value::Null),
    }))
}

/// The bounded "who is this user" paragraph. Read by a human or handed to a model, so it
/// is prose, not JSON, and it says what the person wants as well as who they are.
pub fn compact_memo(record: &UserMemoryDoc) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut who = record.name.trim().to_string();
    if who.is_empty() {
        who = record.email.trim().to_string();
    }
    if let Some(headline) = record
        .headline
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        who = if who.is_empty() {
            headline.to_string()
        } else {
            format!("{who} — {headline}")
        };
    }
    if let Some(location) = record
        .location
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        && !who.is_empty()
    {
        who = format!("{who} ({location})");
    }
    if !who.is_empty() {
        parts.push(who);
    }
    if !record.categories.is_empty() {
        parts.push(format!("Looking for: {}.", record.categories.join(", ")));
    }
    if !record.experience.is_empty() {
        let roles = record
            .experience
            .iter()
            .map(
                |entry| match (entry.title.is_empty(), entry.organization.is_empty()) {
                    (false, false) => format!("{} at {}", entry.title, entry.organization),
                    (false, true) => entry.title.clone(),
                    _ => entry.organization.clone(),
                },
            )
            .collect::<Vec<_>>()
            .join("; ");
        parts.push(format!("Experience: {roles}."));
    }
    if let Some(education) = record
        .education
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        parts.push(format!("Education: {education}."));
    }
    if !record.skills.is_empty() {
        parts.push(format!("Skills: {}.", record.skills.join(", ")));
    }
    if !record.languages.is_empty() {
        parts.push(format!("Languages: {}.", record.languages.join(", ")));
    }
    if !record.avoid_employers.is_empty() {
        parts.push(format!(
            "Not interested in: {}.",
            record.avoid_employers.join(", ")
        ));
    }
    let memo = parts.join(" ");
    if memo.chars().count() <= MAX_RECORD_MEMO_CHARS {
        return memo;
    }
    let mut truncated: String = memo.chars().take(MAX_RECORD_MEMO_CHARS).collect();
    truncated.push('…');
    truncated
}

/// The prompt-facing block, built from the stored record so the model and any human
/// reading the record see the same facts.
pub fn prompt_block(record: &UserMemoryDoc) -> String {
    let mut lines: Vec<String> = Vec::new();
    if !record.name.trim().is_empty() {
        lines.push(format!("- Name: {}", record.name.trim()));
    }
    if !record.email.trim().is_empty() {
        lines.push(format!("- Email: {}", record.email.trim()));
    }
    if let Some(phone) = record.phone.as_deref() {
        lines.push(format!("- Phone: {phone}"));
    }
    if let Some(location) = record.location.as_deref() {
        lines.push(format!("- Location: {location}"));
    }
    if let Some(headline) = record.headline.as_deref() {
        lines.push(format!("- Headline: {headline}"));
    }
    if !record.categories.is_empty() {
        lines.push(format!(
            "- Job categories they are looking for: {}",
            record.categories.join(", ")
        ));
    }
    if !record.experience.is_empty() {
        let roles = record
            .experience
            .iter()
            .map(
                |entry| match (entry.title.is_empty(), entry.organization.is_empty()) {
                    (false, false) => format!("{} at {}", entry.title, entry.organization),
                    (false, true) => entry.title.clone(),
                    _ => entry.organization.clone(),
                },
            )
            .collect::<Vec<_>>()
            .join("; ");
        lines.push(format!("- Experience from their CVs: {roles}"));
    }
    if !record.skills.is_empty() {
        lines.push(format!(
            "- Skills from their CVs: {}",
            record.skills.join(", ")
        ));
    }
    if let Some(education) = record.education.as_deref() {
        lines.push(format!("- Education: {education}"));
    }
    if !record.avoid_employers.is_empty() {
        lines.push(format!(
            "- Employers they asked not to see: {}",
            record.avoid_employers.join(", ")
        ));
    }
    if lines.is_empty() {
        return String::new();
    }
    let mut block = String::from(
        "What you already know about this user, from their account and their previous CVs. \
         Use it instead of asking again, confirm anything that looks out of date, and never \
         invent a detail that is not listed here:\n",
    );
    for line in lines {
        if block.len() + line.len() + 1 > MAX_CONTEXT_CHARS {
            break;
        }
        block.push_str(&line);
        block.push('\n');
    }
    block
}

/// Read the stored record as prompt context. When no record exists yet (a user who has
/// never had a turn since this shipped) the block is derived from the account and the
/// newest CV exactly as before, so nothing regresses in the gap.
pub async fn load_stored(
    state: &AppState,
    user_id: &str,
) -> Result<Option<UserMemoryDoc>, AppError> {
    Ok(state
        .user_memory()
        .find_one(doc! { "_id": user_id })
        .await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> UserMemoryDoc {
        UserMemoryDoc {
            id: "u1".to_string(),
            name: "Fadumo Ali".to_string(),
            email: "fadumo@jobify.so".to_string(),
            phone: Some("+252 61 0000000".to_string()),
            location: Some("Mogadishu".to_string()),
            headline: Some("WASH Officer".to_string()),
            categories: vec!["wash".to_string()],
            keywords: vec!["water".to_string()],
            skills: vec!["Hygiene promotion".to_string(), "Reporting".to_string()],
            experience: vec![MemoryExperience {
                title: "WASH Officer".to_string(),
                organization: "UNICEF".to_string(),
                period: "2021".to_string(),
            }],
            education: Some("BSc, Public Health — University of Hargeisa".to_string()),
            languages: vec!["Somali".to_string()],
            preferred_locations: vec!["Mogadishu".to_string()],
            avoid_employers: vec!["Acme Corp".to_string()],
            memo: String::new(),
            sources: vec!["account".to_string()],
            revision: 1,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn the_memo_reads_as_prose_and_names_what_the_person_wants() {
        let memo = compact_memo(&record());
        assert!(memo.contains("Fadumo Ali — WASH Officer (Mogadishu)"));
        assert!(memo.contains("Looking for: wash."));
        assert!(memo.contains("WASH Officer at UNICEF"));
        assert!(memo.contains("Not interested in: Acme Corp."));
    }

    #[test]
    fn the_memo_is_capped_even_when_everything_is_known() {
        let mut long = record();
        long.skills = (0..40).map(|n| format!("skill {n}")).collect();
        let memo = compact_memo(&long);
        assert!(
            memo.chars().count() <= MAX_RECORD_MEMO_CHARS + 1,
            "memo grew to {} chars",
            memo.chars().count()
        );
    }

    #[test]
    fn the_prompt_block_leaves_out_what_is_unknown() {
        let mut sparse = record();
        sparse.phone = None;
        sparse.headline = None;
        sparse.education = None;
        sparse.experience.clear();
        sparse.skills.clear();
        sparse.languages.clear();
        sparse.avoid_employers.clear();
        let block = prompt_block(&sparse);
        assert!(block.contains("- Name: Fadumo Ali"));
        assert!(!block.contains("Phone:"));
        assert!(!block.contains("Education:"));
        assert!(!block.contains("Employers they asked not to see"));
    }

    #[test]
    fn a_deleted_source_cannot_erase_what_was_learned() {
        // The record already knows skills; neither a CV nor a conversation is available
        // any more. Those skills must still come out the other side.
        let mut skills = Vec::new();
        let mut languages = Vec::new();
        let mut experience = Vec::new();
        let mut education: Option<String> = None;
        let stored = record();
        collect_from_record(&stored, &mut skills, &mut languages, &mut experience);
        dedupe_cap(&mut skills, MAX_RECORD_SKILLS);
        assert_eq!(skills, stored.skills);
        assert_eq!(languages, stored.languages);
        let _ = education.take();
    }

    #[test]
    fn skills_are_capped_and_deduplicated_case_insensitively() {
        let values = vec![
            "Rust".to_string(),
            "rust".to_string(),
            "  ".to_string(),
            "PostgreSQL".to_string(),
        ];
        assert_eq!(capped_unique(values, 5), vec!["Rust", "PostgreSQL"]);
        let many: Vec<String> = (0..30).map(|n| format!("skill {n}")).collect();
        assert_eq!(
            capped_unique(many, MAX_RECORD_SKILLS).len(),
            MAX_RECORD_SKILLS
        );
    }

    #[test]
    fn experience_keeps_the_newest_and_drops_repeats() {
        let entries = vec![
            MemoryExperience {
                title: "Old Job".to_string(),
                organization: "A".to_string(),
                period: String::new(),
            },
            MemoryExperience {
                title: "Recent Job".to_string(),
                organization: "B".to_string(),
                period: String::new(),
            },
            MemoryExperience {
                title: "recent job".to_string(),
                organization: "b".to_string(),
                period: String::new(),
            },
            MemoryExperience {
                title: "Newest Job".to_string(),
                organization: "C".to_string(),
                period: String::new(),
            },
        ];
        let kept = dedupe_experience(entries, MAX_RECORD_EXPERIENCE);
        assert_eq!(kept[0].title, "Newest Job");
        assert_eq!(
            kept.iter()
                .filter(|entry| entry.organization.eq_ignore_ascii_case("b"))
                .count(),
            1
        );
        assert!(kept.len() <= MAX_RECORD_EXPERIENCE);
    }

    #[test]
    fn a_cv_profile_contributes_skills_history_and_education() {
        let cv = json!({
            "skills": ["Water treatment", "Team leadership"],
            "languages": ["Somali", "English"],
            "experience": [{ "title": "WASH Officer", "company": "UNICEF", "startDate": "2021" }],
            "education": [{ "degree": "BSc, Public Health", "institution": "University of Hargeisa" }],
        });
        let mut skills = Vec::new();
        let mut languages = Vec::new();
        let mut experience = Vec::new();
        let mut education = None;
        collect(
            &cv,
            &mut skills,
            &mut languages,
            &mut experience,
            &mut education,
        );
        assert_eq!(skills, vec!["Water treatment", "Team leadership"]);
        assert_eq!(languages, vec!["Somali", "English"]);
        assert_eq!(experience[0].organization, "UNICEF");
        assert_eq!(
            education.unwrap(),
            "BSc, Public Health — University of Hargeisa"
        );
    }

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
