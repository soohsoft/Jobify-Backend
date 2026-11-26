//! One model call at the end of onboarding: which job areas — and which search terms — can
//! this member actually apply for?
//!
//! The interview collects a name, a degree history, a last job title and a band of years.
//! A fixed field-of-study → category map then says "ICT → ict", which is true but useless on
//! its own: the member needs the words an advert for those roles actually uses, and the
//! matcher needs those terms to rank the ICT rows it was already allowed to return.
//! `onboarding` still owns the deterministic map (it is the fallback and the unit of record
//! for the field labels); this module is the cheap model call that turns the same facts into
//! a small, validated decision.
//!
//! Three rules shape the file:
//! - **The model supplies labels only.** Categories are checked against the canonical
//!   taxonomy and dropped when unknown; every list is trimmed, deduped and capped here. The
//!   matcher's query filter is never allowed to contain something the model invented.
//! - **A failure is not a reason to match nobody.** `derive` returns an error; the caller
//!   keeps the deterministic profile and records `category_source: "rules"`.
//! - **1000 output tokens.** The configured model is a reasoning model and truncates a small
//!   JSON answer below that (measured on the assistant's routing call: 2 of 6 answers cut
//!   off at 300, none at 800). A truncated answer here is an error, never a partial profile.

use crate::categories::Category;
use crate::llm::{LlmClient, LlmMessage, LlmRequest};
use crate::models::{EducationEntry, MatchProfile};
use crate::util::json_object;

/// At most three categories. Each one is a query-filter term, and the matcher already reads
/// up to 300 candidate rows per profile.
pub const MAX_CATEGORIES: usize = 3;
/// At most eight search terms, the same budget the deterministic keyword builder keeps.
pub const MAX_KEYWORDS: usize = 8;
pub const MAX_KEYWORD_CHARS: usize = 40;
/// Shared with the assistant's extraction call: enough headroom for the reasoning tokens a
/// reasoning model spends before the first `{`.
pub const DERIVATION_MAX_TOKENS: u32 = 1000;
/// The two values stored in `match_profile.category_source`.
pub const SOURCE_LLM: &str = "llm";
pub const SOURCE_RULES: &str = "rules";

/// What the derivation read. Borrowed so a caller can build it from a `UserDoc` plus the
/// answers in the current request without cloning the whole account.
pub struct Facts<'a> {
    pub education: &'a [EducationEntry],
    pub last_position: Option<&'a str>,
    pub experience_years: Option<u8>,
    pub skills: &'a [String],
}

/// The validated decision. This — not the raw model output — is what gets stored.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Derived {
    pub categories: Vec<String>,
    pub keywords: Vec<String>,
}

impl Derived {
    /// Whether the answer can replace the deterministic one. A category list is the matcher's
    /// query filter: with no valid slug the profile would match nothing at all, so an answer
    /// like that is treated exactly like a failure.
    pub fn is_usable(&self) -> bool {
        !self.categories.is_empty()
    }
}

/// The shape asked for. Every field defaults, so a truncated answer deserializes to
/// something empty rather than failing mid-request.
#[derive(Debug, Default)]
pub struct RawDerivation {
    pub categories: Vec<String>,
    pub keywords: Vec<String>,
}

/// A fingerprint of the facts the decision was made from.
///
/// Stored on the profile so a re-save with unchanged answers reuses the decision instead of
/// paying for the model again. Hashed rather than stored raw: it is only ever compared for
/// equality, and the raw string would repeat the whole degree history on every document.
pub fn facts_key(facts: &Facts<'_>) -> String {
    let education = facts
        .education
        .iter()
        .map(|entry| {
            format!(
                "{}|{}",
                entry.level.trim().to_ascii_lowercase(),
                entry.field.trim().to_ascii_lowercase()
            )
        })
        .collect::<Vec<_>>()
        .join(";");
    let position = facts
        .last_position
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let years = facts
        .experience_years
        .map(|years| years.to_string())
        .unwrap_or_default();
    crate::links::token_fingerprint(&format!("{education}::{position}::{years}"))
}

fn clean_label(value: &str, max_chars: usize) -> Option<String> {
    // Collapse internal runs of whitespace: the model occasionally pads a title, and the
    // matcher looks for a phrase, so "Data  Analyst" would never match "Data Analyst".
    let collapsed = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() || collapsed.chars().count() > max_chars {
        return None;
    }
    Some(collapsed)
}

fn push_unique(target: &mut Vec<String>, value: String, cap: usize) {
    if target.len() >= cap {
        return;
    }
    if !target.iter().any(|kept| kept.eq_ignore_ascii_case(&value)) {
        target.push(value);
    }
}

/// Validate and cap a raw answer. Unknown category slugs are DROPPED, never repaired: a
/// guessed slug would filter the feed down to nothing, while dropping it returns more rows.
pub fn sanitize(raw: RawDerivation) -> Derived {
    let mut categories: Vec<String> = Vec::new();
    for value in &raw.categories {
        if let Some(category) = Category::from_slug(value.trim()) {
            push_unique(&mut categories, category.slug().to_string(), MAX_CATEGORIES);
        }
    }

    let mut keywords: Vec<String> = Vec::new();
    for value in &raw.keywords {
        if let Some(term) = clean_label(value, MAX_KEYWORD_CHARS) {
            push_unique(&mut keywords, term, MAX_KEYWORDS);
        }
    }

    Derived {
        categories,
        keywords,
    }
}

/// Parse a model answer. `None` means there was no JSON object at all (prose, or an answer
/// cut off before the opening brace) — the caller falls back to rules either way, but the
/// two cases are logged differently.
pub fn parse(raw: &str) -> Option<RawDerivation> {
    let object = json_object(raw)?;
    // Hand-rolled rather than `serde_json::from_str` into a struct so that one unexpected
    // field type cannot throw the whole answer away: `{"categories":"ict"}` is a model
    // mistake, and the keywords it also wrote are still worth keeping.
    let value: serde_json::Value = serde_json::from_str(object).ok()?;
    let object = value.as_object()?;
    let list = |key: &str| -> Vec<String> {
        object
            .get(key)
            .and_then(|value| value.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(|text| text.to_string()))
                    .collect()
            })
            .unwrap_or_default()
    };
    Some(RawDerivation {
        categories: list("categories"),
        keywords: list("keywords"),
    })
}

fn slug_catalogue() -> String {
    Category::ALL
        .iter()
        .map(|category| format!("{} — {}", category.slug(), category.label()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn messages(facts: &Facts<'_>) -> Vec<LlmMessage> {
    let system = format!(
        "You decide which job areas a candidate on a Somali jobs and tenders board can \
actually apply for, from their education and work history. Reply with ONE JSON object and \
nothing else:\n\
{{\"categories\":[\"slug\"],\"keywords\":[\"term\"]}}\n\
Rules:\n\
- categories: 1 to {MAX_CATEGORIES} slugs, and ONLY from this list:\n{slugs}\n\
- keywords: up to {MAX_KEYWORDS} short terms (tools, skills, sector words, job titles) that \
appear in adverts for those roles — for example \"data analyst\", \"SQL\", \"programme officer\".\n\
- Use what the person actually studied and actually did. Do not invent a field they did not \
study or a job they did not hold. A master's in business administration on top of an ICT \
degree with a data job means both areas.\n\
- Most adverts are NGO, UN agency, telecom, bank or government roles in Mogadishu, Hargeisa, \
Garowe or Kismayo.",
        slugs = slug_catalogue()
    );

    let user = serde_json::json!({
        "education": facts.education,
        "last_position": facts.last_position,
        "experience_years": facts.experience_years,
        "skills": facts.skills,
    })
    .to_string();

    vec![LlmMessage::system(system), LlmMessage::user(user)]
}

/// The one call. `Err` carries a log-safe detail (never the key, never the prompt).
pub async fn derive(llm: &dyn LlmClient, facts: &Facts<'_>) -> Result<Derived, String> {
    let reply = llm
        .complete(LlmRequest {
            messages: messages(facts),
            max_tokens: DERIVATION_MAX_TOKENS,
            json_mode: true,
        })
        .await
        .map_err(|err| err.detail())?;

    let raw = parse(&reply.content).ok_or_else(|| {
        format!(
            "no JSON object in the answer (finish_reason={:?})",
            reply.finish_reason
        )
    })?;
    Ok(sanitize(raw))
}

/// The profile a completed interview is matched on.
///
/// `categories` are the gate, `keywords` rank inside it. `source` says which of the two
/// paths produced them, and `facts_key` fingerprints the answers so the next save can skip
/// the model.
pub fn profile(
    categories: Vec<String>,
    keywords: Vec<String>,
    source: &str,
    facts_key: String,
    updated_at: String,
) -> MatchProfile {
    MatchProfile {
        categories,
        keywords,
        // Somalia-wide: the interview never asks for a city, and the matcher has no location
        // signal to rank on.
        locations: Vec::new(),
        employer_types: Vec::new(),
        positions: Vec::new(),
        category_source: source.to_string(),
        facts_key,
        updated_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{LlmError, LlmReply, LlmUsage};
    use futures_util::future::{BoxFuture, ready};
    use std::sync::{Arc, Mutex};

    fn facts(education: &[EducationEntry]) -> Facts<'_> {
        Facts {
            education,
            last_position: Some("Data Analyst"),
            experience_years: Some(3),
            skills: &[],
        }
    }

    #[test]
    fn a_valid_answer_is_accepted_and_every_list_is_capped() {
        let raw = RawDerivation {
            categories: vec![
                "ict".to_string(),
                "business_and_finance".to_string(),
                "health".to_string(),
                "education".to_string(),
            ],
            keywords: (0..12).map(|n| format!("term{n}")).collect(),
        };
        let derived = sanitize(raw);
        assert_eq!(
            derived.categories,
            vec!["ict", "business_and_finance", "health"],
            "three categories are kept, the fourth is dropped"
        );
        assert_eq!(derived.keywords.len(), MAX_KEYWORDS);
        assert!(derived.is_usable());
    }

    #[test]
    fn unknown_slugs_are_dropped_and_prose_slugs_are_not_repaired() {
        let raw = RawDerivation {
            categories: vec![
                "ICT".to_string(),
                "  ict  ".to_string(),
                "computer_technology".to_string(),
                "Information Technology".to_string(),
            ],
            keywords: Vec::new(),
        };
        let derived = sanitize(raw);
        assert_eq!(
            derived.categories,
            vec!["ict"],
            "the label and the hallucinated slug are dropped, and the duplicate collapses"
        );

        // Nothing valid is an unusable answer, not an empty-profile decision.
        let none = sanitize(RawDerivation {
            categories: vec!["nope".to_string()],
            keywords: vec!["data analyst".to_string()],
        });
        assert!(none.categories.is_empty());
        assert!(!none.is_usable());
    }

    #[test]
    fn keywords_are_cleaned_deduped_and_bounded() {
        let raw = RawDerivation {
            categories: vec!["ict".to_string()],
            keywords: vec![
                "  Data   Analyst ".to_string(),
                "data analyst".to_string(),
                String::new(),
                "x".repeat(MAX_KEYWORD_CHARS + 1),
                "SQL".to_string(),
                "sql".to_string(),
                "Reporting".to_string(),
            ],
        };
        let derived = sanitize(raw);
        assert_eq!(
            derived.keywords,
            vec!["Data Analyst", "SQL", "Reporting"],
            "whitespace collapsed, duplicates and over-long terms dropped"
        );
    }

    #[test]
    fn an_answer_without_an_object_is_not_an_answer() {
        assert!(parse("I think this person should be a data analyst.").is_none());
        assert!(parse("").is_none());
        // A truncated object does not deserialize into anything: the caller falls back.
        assert!(parse(r#"{"categories":["ict"],"keywor"#).is_none());
        // Prose around a real object is fine — reasoning models write a sentence first.
        let framed = parse("Here is the JSON:\n```json\n{\"categories\":[\"ict\"]}\n```");
        assert_eq!(framed.unwrap().categories, vec!["ict"]);
    }

    #[test]
    fn the_fingerprint_changes_only_when_the_answers_do() {
        let a = [EducationEntry {
            level: "bachelor".to_string(),
            field: "ICT".to_string(),
        }];
        let b = [EducationEntry {
            level: "bachelor".to_string(),
            field: "ICT".to_string(),
        }];
        assert_eq!(facts_key(&facts(&a)), facts_key(&facts(&b)));
        assert_ne!(facts_key(&facts(&a)), {
            let other = Facts {
                experience_years: Some(6),
                ..facts(&a)
            };
            facts_key(&other)
        });
        // A different position is a different decision too.
        let other = Facts {
            last_position: Some("MIS Officer"),
            ..facts(&a)
        };
        assert_ne!(facts_key(&facts(&a)), facts_key(&other));
    }

    struct StubLlm {
        reply: Mutex<Option<Result<LlmReply, LlmError>>>,
        seen: Mutex<Vec<LlmRequest>>,
    }

    impl StubLlm {
        fn new(reply: Result<LlmReply, LlmError>) -> Arc<Self> {
            Arc::new(Self {
                reply: Mutex::new(Some(reply)),
                seen: Mutex::new(Vec::new()),
            })
        }

        fn calls(&self) -> usize {
            self.seen.lock().unwrap().len()
        }
    }

    impl LlmClient for StubLlm {
        fn complete<'a>(
            &'a self,
            request: LlmRequest,
        ) -> BoxFuture<'a, Result<LlmReply, LlmError>> {
            self.seen.lock().unwrap().push(request);
            let next = self
                .reply
                .lock()
                .unwrap()
                .take()
                .unwrap_or_else(|| Err(LlmError::Transport("stub exhausted".to_string())));
            Box::pin(ready(next))
        }
    }

    #[tokio::test]
    async fn derive_passes_the_facts_as_json_and_asks_for_a_bounded_answer() {
        let stub = StubLlm::new(Ok(LlmReply {
            content:
                r#"{"categories":["ict","business_and_finance"],"keywords":["SQL","data analyst"]}"#
                    .to_string(),
            usage: LlmUsage::default(),
            finish_reason: Some("stop".to_string()),
        }));
        let education = [EducationEntry {
            level: "bachelor".to_string(),
            field: "ICT".to_string(),
        }];
        let derived = derive(stub.as_ref(), &facts(&education)).await.unwrap();
        assert_eq!(stub.calls(), 1);
        assert_eq!(derived.categories, vec!["ict", "business_and_finance"]);
        assert_eq!(derived.keywords, vec!["SQL", "data analyst"]);

        let seen = stub.seen.lock().unwrap();
        let request = &seen[0];
        assert!(request.json_mode, "the answer is parsed as JSON");
        assert_eq!(
            request.max_tokens, DERIVATION_MAX_TOKENS,
            "a reasoning model truncates this at 300"
        );
        assert!(request.messages[1].content.contains("Data Analyst"));
    }

    #[tokio::test]
    async fn a_provider_failure_is_an_error_so_the_caller_can_fall_back() {
        let stub = StubLlm::new(Err(LlmError::Provider(502)));
        let education = [EducationEntry {
            level: "bachelor".to_string(),
            field: "ICT".to_string(),
        }];
        assert!(derive(stub.as_ref(), &facts(&education)).await.is_err());
    }
}
