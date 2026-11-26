//! Education and work history → what the matcher reads.
//!
//! The Discord interview asks four things (name, education, last position, years) and the
//! matching itself stays in Rust — no model call. This module is the whole translation:
//!
//! - a **level** and a **field of study** are validated against fixed lists, so the stored
//!   document can only ever hold a value the rest of the system understands;
//! - a field maps to at most one job category, and the union of them becomes
//!   `match_profile.categories` — the matcher's query filter;
//! - the words of the last position and of any free-text ("Other") field become
//!   `match_profile.keywords`, which only re-rank rows the categories already admitted.
//!
//! Nothing here knows about Discord. It is called from `routes::discord::update_profile` and
//! is unit-tested on its own, because a silent change to these lists changes who gets sent
//! what.
use crate::error::AppError;
use crate::models::EducationEntry;
use crate::routes::auth::MAX_PROFILE_KEYWORDS;

/// How many degrees one member may record. The interview offers a fifth "another degree?"
/// round no longer than this, and a payload beyond it is refused rather than truncated: the
/// member clicked for it, so dropping one silently would lose an answer they gave.
pub const MAX_EDUCATION_ENTRIES: usize = 4;

/// A last position contributes at most this many search terms. A long job title otherwise
/// floods the keyword list and every keyword is scored against every candidate row.
pub const MAX_POSITION_TERMS: usize = 4;

/// The levels the interview offers, in ascending order. Stored verbatim as `education.level`.
pub const LEVELS: &[&str] = &[
    "none",
    "primary",
    "secondary",
    "diploma",
    "bachelor",
    "master",
    "phd",
];

/// Only these ask a follow-up question about the field of study.
pub const DEGREE_LEVELS: &[&str] = &["bachelor", "master", "phd"];

/// One allowed answer to "what did you study?" and the category it matches.
///
/// The labels are what the member sees and what is stored; the slug is the canonical
/// `Category` slug the matcher filters on. `None` means "no category": the free-text answer
/// ("Other") can only ever have keywords, because inventing a category for it would filter
/// the feed down to something the member did not ask for.
pub struct FieldSpec {
    pub label: &'static str,
    pub category: Option<&'static str>,
}

pub const FIELDS: &[FieldSpec] = &[
    FieldSpec {
        label: "Health",
        category: Some("health"),
    },
    FieldSpec {
        label: "Education",
        category: Some("education"),
    },
    FieldSpec {
        label: "Business administration",
        category: Some("business_and_finance"),
    },
    FieldSpec {
        label: "Finance & accounting",
        category: Some("business_and_finance"),
    },
    FieldSpec {
        label: "ICT",
        category: Some("ict"),
    },
    FieldSpec {
        label: "Engineering",
        category: Some("engineering_and_construction"),
    },
    FieldSpec {
        label: "Agriculture",
        category: Some("agriculture"),
    },
    FieldSpec {
        label: "Law",
        category: Some("legal_and_regulatory_services"),
    },
    FieldSpec {
        label: "Logistics & procurement",
        category: Some("logistics_and_supply_chain"),
    },
    FieldSpec {
        label: "Social science & development",
        category: Some("humanitarian_aid_and_emergency_relief"),
    },
    FieldSpec {
        label: "WASH",
        category: Some("wash"),
    },
    FieldSpec {
        label: "Nutrition",
        category: Some("food_security"),
    },
    FieldSpec {
        label: "Other",
        category: None,
    },
    // The academic faculties the bot offers first (owner's list). The older, more job-shaped
    // labels above stay because they are already stored on existing profiles — a re-save has to
    // canonicalise to the same string it was written with.
    FieldSpec {
        label: "Management",
        category: Some("business_and_finance"),
    },
    FieldSpec {
        label: "Computing",
        category: Some("ict"),
    },
    FieldSpec {
        label: "Social Science",
        category: Some("humanitarian_aid_and_emergency_relief"),
    },
    FieldSpec {
        label: "Economics",
        category: Some("business_and_finance"),
    },
    FieldSpec {
        label: "Accounting",
        category: Some("business_and_finance"),
    },
    FieldSpec {
        label: "Health and Medicine",
        category: Some("health"),
    },
];

/// The label used for the free-text escape hatch. An "Other" entry stores the member's own
/// words after this prefix — `Other: Marine biology` — because an education entry is
/// `{level, field}` and the words have to live somewhere the matcher can read them.
pub const OTHER_LABEL: &str = "Other";

/// Words too common to be a search term. Lowercased before the lookup.
const STOPWORDS: &[&str] = &[
    "and", "the", "of", "in", "for", "at", "a", "an", "with", "to", "on", "by", "or", "as", "my",
    "i", "is", "from",
];

fn normalize(raw: &str) -> String {
    raw.trim().to_ascii_lowercase()
}

/// A level must be one of the seven. Anything else is named in the 400: the bot's buttons can
/// only send these, so a stray value is a wire bug worth seeing rather than a typo to absorb.
pub fn canonical_level(raw: &str) -> Result<&'static str, AppError> {
    let value = normalize(raw);
    LEVELS
        .iter()
        .copied()
        .find(|level| *level == value)
        .ok_or_else(|| {
            AppError::BadRequest(format!(
                "'{}' is not an education level. Use one of: {}.",
                raw.trim(),
                LEVELS.join(", ")
            ))
        })
}

/// A field of study, canonicalised.
#[derive(Debug)]
pub struct CanonicalField {
    /// What is stored in `education.field`: the canonical label, or `Other: <words>`.
    pub stored: String,
    /// The category slug this field matches, if any.
    pub category: Option<&'static str>,
}

pub fn canonical_field(raw: &str) -> Result<CanonicalField, AppError> {
    let trimmed = raw.trim();
    let lowered = normalize(trimmed);

    if let Some(spec) = FIELDS.iter().find(|spec| normalize(spec.label) == lowered) {
        return Ok(CanonicalField {
            stored: spec.label.to_string(),
            category: spec.category,
        });
    }

    // `Other` with or without the detail the member typed after it.
    if let Some(rest) = lowered.strip_prefix("other") {
        let rest = rest.trim_start_matches([':', '-', ' ']).trim();
        if rest.is_empty() {
            return Ok(CanonicalField {
                stored: OTHER_LABEL.to_string(),
                category: None,
            });
        }
        let text = raw.trim()[raw.trim().len() - rest.len()..]
            .trim()
            .to_string();
        if text.chars().count() > crate::routes::auth::MAX_PROFILE_TERM_CHARS {
            return Err(AppError::BadRequest(format!(
                "The detail for 'Other' must be {} characters or fewer.",
                crate::routes::auth::MAX_PROFILE_TERM_CHARS
            )));
        }
        return Ok(CanonicalField {
            stored: format!("{OTHER_LABEL}: {text}"),
            category: None,
        });
    }

    let labels: Vec<&str> = FIELDS.iter().map(|spec| spec.label).collect();
    Err(AppError::BadRequest(format!(
        "'{trimmed}' is not a field of study. Use one of: {}.",
        labels.join(", ")
    )))
}

/// Validates and canonicalises the whole list, preserving the order the member answered in.
///
/// A level that does not ask for a field must not carry one — that is the shape the bot
/// sends — and a degree level must. Both are refused by name instead of stored half-filled.
pub fn canonical_education(raw: &[EducationEntry]) -> Result<Vec<EducationEntry>, AppError> {
    if raw.len() > MAX_EDUCATION_ENTRIES {
        return Err(AppError::BadRequest(format!(
            "Add at most {MAX_EDUCATION_ENTRIES} education entries — you sent {}.",
            raw.len()
        )));
    }

    let mut education = Vec::with_capacity(raw.len());
    for entry in raw {
        let level = canonical_level(&entry.level)?;
        if DEGREE_LEVELS.contains(&level) {
            let field = canonical_field(&entry.field)?;
            education.push(EducationEntry {
                level: level.to_string(),
                field: field.stored,
            });
        } else {
            let field = entry.field.trim();
            if !field.is_empty() {
                return Err(AppError::BadRequest(format!(
                    "'{field}' cannot accompany '{level}': a field of study is only asked for {}.",
                    DEGREE_LEVELS.join(", ")
                )));
            }
            education.push(EducationEntry {
                level: level.to_string(),
                field: String::new(),
            });
        }
    }
    Ok(education)
}

/// The category slug a stored field maps to, if any.
pub fn category_for_field(field: &str) -> Option<&'static str> {
    canonical_field(field).ok().and_then(|field| field.category)
}

/// The union of the categories the recorded fields map to, in entry order, deduped.
///
/// This is `match_profile.categories`. An all-"Other" history yields an empty list — a real
/// state the matcher reports as `no_match_profile` rather than an error — and the account is
/// still onboarding-complete, because they answered every question they were asked.
pub fn categories(education: &[EducationEntry]) -> Vec<String> {
    let mut categories: Vec<String> = Vec::new();
    for entry in education {
        if let Some(slug) = category_for_field(&entry.field)
            && !categories.iter().any(|kept| kept == slug)
        {
            categories.push(slug.to_string());
        }
    }
    categories
}

fn rank(level: &str) -> usize {
    LEVELS
        .iter()
        .position(|candidate| *candidate == normalize(level))
        .map(|index| index + 1)
        .unwrap_or(0)
}

/// The highest level recorded, so the single `education_level` field the portal renders keeps
/// saying something true after the interview writes a whole list.
pub fn highest_level(education: &[EducationEntry]) -> Option<&'static str> {
    education
        .iter()
        .filter_map(|entry| {
            LEVELS
                .iter()
                .copied()
                .find(|candidate| *candidate == normalize(&entry.level))
        })
        .max_by_key(|level| rank(level))
}

/// The member's own words for an "Other" entry, if the entry has any.
pub fn other_text(field: &str) -> Option<String> {
    let trimmed = field.trim();
    let rest = trimmed.get(..OTHER_LABEL.len())?;
    if !rest.eq_ignore_ascii_case(OTHER_LABEL) {
        return None;
    }
    let detail = trimmed[OTHER_LABEL.len()..]
        .trim_start_matches([':', '-', ' '])
        .trim();
    if detail.is_empty() {
        None
    } else {
        Some(detail.to_string())
    }
}

fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .map(|word| word.trim().to_ascii_lowercase())
        .filter(|word| word.chars().count() >= 2 && !STOPWORDS.contains(&word.as_str()))
        .collect()
}

fn add_term(terms: &mut Vec<String>, term: &str) {
    let trimmed = term.trim();
    if trimmed.is_empty() || trimmed.chars().count() > crate::routes::auth::MAX_PROFILE_TERM_CHARS {
        return;
    }
    if terms.len() >= MAX_PROFILE_KEYWORDS {
        return;
    }
    if !terms.iter().any(|kept| kept.eq_ignore_ascii_case(trimmed)) {
        terms.push(trimmed.to_string());
    }
}

/// `match_profile.keywords`: the words of the last position (capped), the detail behind every
/// "Other" field, then any skills still sent by another client. Total is capped at the
/// matcher's keyword limit — these are scored against every candidate row.
pub fn match_keywords(
    last_position: Option<&str>,
    education: &[EducationEntry],
    skills: &[String],
) -> Vec<String> {
    let mut terms: Vec<String> = Vec::new();

    if let Some(position) = last_position {
        for word in words(position).into_iter().take(MAX_POSITION_TERMS) {
            add_term(&mut terms, &word);
        }
    }

    for entry in education {
        if let Some(detail) = other_text(&entry.field) {
            for word in words(&detail) {
                add_term(&mut terms, &word);
            }
        }
    }

    for skill in skills {
        add_term(&mut terms, skill);
    }

    terms
}

/// Whether finishing the interview must switch the daily digest on.
///
/// Enabling alerts is the product's default, but an explicit switch-off (`/stop`,
/// `/alerts off`) has to survive a re-run of onboarding: `alerts.user_disabled` is that
/// promise. A member who already has alerts on is left exactly as they are, so a re-run
/// cannot silently change an "instant" cadence back to daily.
pub fn should_enable_alerts(complete: bool, alerts: Option<&crate::models::AlertPrefs>) -> bool {
    if !complete {
        return false;
    }
    match alerts {
        None => true,
        Some(alerts) => !alerts.user_disabled && !alerts.enabled,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(level: &str, field: &str) -> EducationEntry {
        EducationEntry {
            level: level.to_string(),
            field: field.to_string(),
        }
    }

    #[test]
    fn levels_are_case_and_space_insensitive_and_anything_else_is_named() {
        assert_eq!(canonical_level(" Bachelor ").unwrap(), "bachelor");
        assert_eq!(canonical_level("PhD").unwrap(), "phd");
        let err = canonical_level("university").unwrap_err();
        assert!(matches!(err, AppError::BadRequest(_)));
        assert!(err.message().contains("university"));
    }

    #[test]
    fn the_academic_faculties_the_bot_offers_map_to_real_categories() {
        // The nine labels the bot puts on screen. Law / Education / Engineering already existed
        // under the same spelling; the rest are additive, so an old profile keeps canonicalising.
        let expected = [
            ("Management", "business_and_finance"),
            ("Computing", "ict"),
            ("Social Science", "humanitarian_aid_and_emergency_relief"),
            ("Economics", "business_and_finance"),
            ("Law", "legal_and_regulatory_services"),
            ("Education", "education"),
            ("Engineering", "engineering_and_construction"),
            ("Accounting", "business_and_finance"),
            ("Health and Medicine", "health"),
        ];
        for (label, category) in expected {
            let field = canonical_field(label).unwrap();
            assert_eq!(field.stored, label);
            assert_eq!(field.category, Some(category), "{label} maps elsewhere");
            assert!(
                crate::categories::Category::from_slug(category).is_some(),
                "{category} is not a real category"
            );
        }
        // A master's programme uses the same labels, so the union dedupes rather than double-counts.
        let cats = categories(&[entry("bachelor", "Computing"), entry("master", "Computing")]);
        assert_eq!(cats, vec!["ict".to_string()]);
    }

    #[test]
    fn the_twelve_fields_map_to_the_categories_the_matcher_filters_on() {
        let expected = [
            ("Health", Some("health")),
            ("Education", Some("education")),
            ("Business administration", Some("business_and_finance")),
            ("Finance & accounting", Some("business_and_finance")),
            ("ICT", Some("ict")),
            ("Engineering", Some("engineering_and_construction")),
            ("Agriculture", Some("agriculture")),
            ("Law", Some("legal_and_regulatory_services")),
            (
                "Logistics & procurement",
                Some("logistics_and_supply_chain"),
            ),
            (
                "Social science & development",
                Some("humanitarian_aid_and_emergency_relief"),
            ),
            ("WASH", Some("wash")),
            ("Nutrition", Some("food_security")),
        ];
        for (label, category) in expected {
            let field = canonical_field(label).unwrap();
            assert_eq!(field.stored, label, "{label} must be stored canonically");
            assert_eq!(
                field.category, category,
                "{label} maps to the wrong category"
            );
            // Every mapped slug must be a real category, or the matcher's filter matches
            // nothing at all.
            if let Some(slug) = category {
                assert!(crate::categories::Category::from_slug(slug).is_some());
            }
        }
        // Case and stray spacing from a client must not lose the mapping.
        assert_eq!(canonical_field("  ict ").unwrap().stored, "ICT");
        assert_eq!(
            canonical_field("finance & ACCOUNTING").unwrap().category,
            Some("business_and_finance")
        );
    }

    #[test]
    fn an_unknown_field_is_a_400_that_names_the_offender() {
        let err = canonical_field("Marine biology").unwrap_err();
        assert!(matches!(err, AppError::BadRequest(_)));
        assert!(err.message().contains("Marine biology"));
        assert!(err.message().contains("Health"), "the list is offered back");

        let err = canonical_education(&[entry("bachelor", "Nursing science")]).unwrap_err();
        assert!(err.message().contains("Nursing science"));
    }

    #[test]
    fn other_carries_free_text_and_matches_no_category() {
        let field = canonical_field("Other: Marine biology").unwrap();
        assert_eq!(field.stored, "Other: Marine biology");
        assert_eq!(field.category, None);
        assert_eq!(other_text(&field.stored).as_deref(), Some("Marine biology"));

        // Bare "Other" is legitimate too — the member may decline to elaborate.
        let bare = canonical_field("other").unwrap();
        assert_eq!(bare.stored, "Other");
        assert_eq!(bare.category, None);
        assert_eq!(other_text(&bare.stored), None);
    }

    #[test]
    fn two_degrees_union_their_categories_and_keep_entry_order() {
        let education = canonical_education(&[
            entry("bachelor", "Health"),
            entry("master", "Business administration"),
            entry("diploma", ""),
        ])
        .unwrap();
        assert_eq!(education.len(), 3);
        assert_eq!(
            categories(&education),
            vec!["health", "business_and_finance"],
            "health first, in the order the degrees were given"
        );
        // A second Health degree must not duplicate the filter value.
        let repeated =
            canonical_education(&[entry("bachelor", "Health"), entry("master", "Health")]).unwrap();
        assert_eq!(categories(&repeated), vec!["health"]);
        // An "Other" degree adds no category, so this is legitimately empty.
        let other = canonical_education(&[entry("phd", "Other: Famine studies")]).unwrap();
        assert!(categories(&other).is_empty());
    }

    #[test]
    fn at_most_four_entries_are_accepted() {
        let four = vec![
            entry("bachelor", "Health"),
            entry("master", "Health"),
            entry("phd", "Health"),
            entry("diploma", ""),
        ];
        assert_eq!(canonical_education(&four).unwrap().len(), 4);

        let mut five = four.clone();
        five.push(entry("master", "ICT"));
        let err = canonical_education(&five).unwrap_err();
        assert!(matches!(err, AppError::BadRequest(_)));
        assert!(err.message().contains("at most 4"));
    }

    #[test]
    fn a_non_degree_level_must_not_carry_a_field() {
        assert!(canonical_education(&[entry("secondary", "")]).is_ok());
        let err = canonical_education(&[entry("secondary", "Health")]).unwrap_err();
        assert!(err.message().contains("secondary"));
        // A degree without a field is refused as well, naming the empty answer.
        assert!(canonical_education(&[entry("bachelor", "")]).is_err());
    }

    #[test]
    fn the_highest_level_is_what_the_single_education_field_reports() {
        let education = canonical_education(&[
            entry("diploma", ""),
            entry("master", "Health"),
            entry("bachelor", "ICT"),
        ])
        .unwrap();
        assert_eq!(highest_level(&education), Some("master"));
        assert_eq!(highest_level(&[]), None);
    }

    #[test]
    fn keywords_come_from_the_last_position_first_and_are_capped() {
        let education = canonical_education(&[
            entry("bachelor", "Health"),
            entry("master", "Other: Marine biology"),
        ])
        .unwrap();
        let keywords = match_keywords(Some("Senior Programme Officer"), &education, &[]);
        // A stopword ("of") never survives, and the title's own words lead.
        assert_eq!(&keywords[..3], vec!["senior", "programme", "officer"]);
        assert!(keywords.contains(&"marine".to_string()));
        assert!(keywords.contains(&"biology".to_string()));
        assert!(
            keywords.len() <= MAX_PROFILE_KEYWORDS,
            "the matcher scores every keyword against every row"
        );

        // At most four words from the position, however long the title.
        let long = match_keywords(
            Some("Deputy Director of Programmes and Partnerships for Somalia"),
            &[],
            &[],
        );
        assert_eq!(long.len(), MAX_POSITION_TERMS);
        assert!(!long.iter().any(|term| term == "of"));

        // Skills from another client still land, deduped case-insensitively.
        let mixed = match_keywords(
            Some("Programme Officer"),
            &[],
            &[
                "officer".to_string(),
                "M&E".to_string(),
                "officer ".to_string(),
            ],
        );
        assert_eq!(mixed.iter().filter(|term| *term == "officer").count(), 1);
        assert!(mixed.iter().any(|term| term.eq_ignore_ascii_case("m&e")));
    }

    #[test]
    fn alerts_are_enabled_on_completion_unless_the_member_switched_them_off() {
        use crate::models::AlertPrefs;

        assert!(should_enable_alerts(true, None));
        assert!(!should_enable_alerts(false, None));

        let off_by_stop = AlertPrefs {
            enabled: false,
            user_disabled: true,
            ..Default::default()
        };
        assert!(
            !should_enable_alerts(true, Some(&off_by_stop)),
            "/stop must survive a re-run of onboarding"
        );

        let already_on = AlertPrefs {
            enabled: true,
            mode: "instant".to_string(),
            ..Default::default()
        };
        assert!(
            !should_enable_alerts(true, Some(&already_on)),
            "a re-run must not rewrite an instant cadence"
        );

        let off_but_not_by_the_member = AlertPrefs {
            enabled: false,
            ..Default::default()
        };
        assert!(should_enable_alerts(true, Some(&off_but_not_by_the_member)));
    }
}
