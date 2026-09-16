use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const TOKENS_PER_USD_AT_COST: u64 = 5_500_000;
pub const TOKENS_PER_USD_BILLED: u64 = 1_100_000;
pub const PROFIT_MARGIN: f64 = 0.8;
pub const CREDIT_CURRENCY: &str = "USD";
pub const MIN_TOP_UP_USD: f64 = 1.0;
pub const COST_PER_TOKEN: f64 = 1.0 / TOKENS_PER_USD_AT_COST as f64;
pub const PRICE_PER_TOKEN: f64 = COST_PER_TOKEN / (1.0 - PROFIT_MARGIN);

pub const RESUME_TEMPLATES: &[ResumeTemplate] = &[
    ResumeTemplate {
        id: "modern",
        name: "Modern",
        description: "Clean two-column layout with a bold header.",
    },
    ResumeTemplate {
        id: "classic",
        name: "Classic",
        description: "Traditional single-column professional layout.",
    },
    ResumeTemplate {
        id: "minimal",
        name: "Minimal",
        description: "Minimal whitespace-focused layout.",
    },
    ResumeTemplate {
        id: "professional",
        name: "Professional",
        description: "Executive layout with strong hierarchy.",
    },
];

#[derive(Serialize, Deserialize, Clone)]
pub struct ResumeTemplate {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
}

pub fn is_valid_template_id(id: &str) -> bool {
    RESUME_TEMPLATES.iter().any(|t| t.id == id)
}

pub fn usd_to_tokens(usd: f64) -> u64 {
    (usd * TOKENS_PER_USD_BILLED as f64).max(0.0).floor() as u64
}

pub fn tokens_to_usd(tokens: u64) -> f64 {
    tokens as f64 / TOKENS_PER_USD_BILLED as f64
}

// ---------------------------------------------------------------------------
// Job board / scraper
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone)]
pub struct JobDoc {
    #[serde(rename = "_id")]
    pub id: String,
    #[serde(default)]
    pub external_id: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub source_name: String,
    pub title: String,
    #[serde(default)]
    pub organization: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub requirements: Vec<String>,
    #[serde(default)]
    pub qualifications: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub posted_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline: Option<String>,
    #[serde(default)]
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
    /// Logo of the employer / hiring company (e.g. UNICEF, UN) as shown in
    /// the job listing. NOT the job board's own site branding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organization_image_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub employment_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub salary: Option<String>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct JobInput {
    pub external_id: String,
    pub source: String,
    #[serde(default)]
    pub source_name: String,
    pub title: String,
    #[serde(default)]
    pub organization: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub requirements: Vec<String>,
    #[serde(default)]
    pub qualifications: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub posted_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline: Option<String>,
    #[serde(default)]
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
    /// Logo of the employer / hiring company (e.g. UNICEF, UN) as shown in
    /// the job listing. NOT the job board's own site branding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organization_image_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub employment_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub salary: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct SourceSelectors {
    pub card: String,
    pub title: String,
    pub organization: String,
    pub location: String,
    pub description: String,
    pub requirements: String,
    pub posted_date: String,
    pub deadline: String,
    pub image: String,
    /// CSS selector for the employer / hiring company logo inside each job
    /// listing (not the job board's own site branding).
    #[serde(default)]
    pub organization_image: String,
    #[serde(default)]
    pub qualifications: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub employment_type: String,
    #[serde(default)]
    pub salary: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct SourceConfig {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub listing_url: String,
    #[serde(rename = "type")]
    pub source_type: String,
    pub enabled: bool,
    pub selectors: SourceSelectors,
}

pub fn default_job_sources() -> Vec<SourceConfig> {
    let selector = |card: &str,
                    title: &str,
                    organization: &str,
                    location: &str,
                    description: &str,
                    requirements: &str,
                    posted_date: &str,
                    deadline: &str,
                    image: &str,
                    organization_image: &str,
                    qualifications: &str,
                    category: &str,
                    employment_type: &str,
                    salary: &str| SourceSelectors {
        card: card.to_string(),
        title: title.to_string(),
        organization: organization.to_string(),
        location: location.to_string(),
        description: description.to_string(),
        requirements: requirements.to_string(),
        posted_date: posted_date.to_string(),
        deadline: deadline.to_string(),
        image: image.to_string(),
        organization_image: organization_image.to_string(),
        qualifications: qualifications.to_string(),
        category: category.to_string(),
        employment_type: employment_type.to_string(),
        salary: salary.to_string(),
    };

    vec![
        SourceConfig {
            id: "qaranjobs".to_string(),
            name: "QaranJobs".to_string(),
            base_url: "https://qaranjobs.com".to_string(),
            listing_url: "https://qaranjobs.com/jobs".to_string(),
            source_type: "external".to_string(),
            enabled: false,
            selectors: selector(
                ".job-card",
                ".job-title a",
                ".company",
                ".location",
                ".job-description",
                ".requirements",
                ".posted-date",
                ".deadline",
                ".job-image img",
                ".company-logo img",
                ".qualifications li",
                ".category",
                ".employment-type",
                ".salary",
            ),
        },
        SourceConfig {
            id: "somalijobs".to_string(),
            name: "SomaliJobs".to_string(),
            base_url: "https://somalijobs.com".to_string(),
            listing_url: "https://somalijobs.com/jobs".to_string(),
            source_type: "external".to_string(),
            enabled: true,
            selectors: selector(
                "a.jobs-listing-container",
                ".jobs-listing-title",
                ".jobs-listing-company",
                "",
                ".jobs-listing-details",
                "",
                "",
                "",
                "",
                ".jobs-listing-image img",
                "",
                "",
                "",
                "",
            ),
        },
        SourceConfig {
            id: "shaqodoon".to_string(),
            name: "Shaqodoon".to_string(),
            base_url: "https://shaqodoon.com".to_string(),
            listing_url: "https://shaqodoon.com/jobs".to_string(),
            source_type: "external".to_string(),
            enabled: false,
            selectors: selector(
                ".vacancy",
                ".vacancy-title a",
                ".org-name",
                ".vacancy-location",
                ".vacancy-excerpt",
                ".vacancy-requirements",
                ".posted-date",
                ".closing-date",
                ".job-image img",
                ".vacancy-logo img",
                ".qualifications li",
                ".category",
                ".employment-type",
                ".salary",
            ),
        },
        SourceConfig {
            id: "unjobs".to_string(),
            name: "UNjobs".to_string(),
            base_url: "https://unjobs.org".to_string(),
            listing_url: "https://unjobs.org/duty_stations/somalia".to_string(),
            source_type: "external".to_string(),
            enabled: false,
            selectors: selector(
                "div.job[id]",
                "a.jtitle",
                "@line:1",
                "",
                "",
                "",
                "",
                "span[id^=\"j\"]",
                "",
                "",
                "",
                "",
                "",
                "",
            ),
        },
    ]
}

// ---------------------------------------------------------------------------
// Auth
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct MatchProfile {
    /// Canonical category slugs, validated through `categories::Category`.
    #[serde(default)]
    pub categories: Vec<String>,
    /// Free-text role/skill signals the extraction pulled from the conversation.
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub locations: Vec<String>,
    #[serde(default)]
    pub updated_at: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct UserDoc {
    #[serde(rename = "_id")]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub password_hash: String,
    #[serde(default)]
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    /// What to match this user against. Derived from the CV conversation (or a
    /// manual category pick), and the thing a notifier reads to decide whether
    /// the user has anything to be matched on yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub match_profile: Option<MatchProfile>,
    #[serde(default)]
    pub created_at: String,
}

#[derive(Deserialize)]
pub struct RegisterRequest {
    pub name: String,
    pub email: String,
    pub password: String,
}

#[derive(Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Serialize)]
pub struct UserResponse {
    pub id: String,
    pub name: String,
    pub email: String,
    pub role: String,
    pub location: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub match_profile: Option<MatchProfile>,
}

#[derive(Serialize)]
pub struct AuthResult {
    pub user: UserResponse,
    pub token: String,
}

// ---------------------------------------------------------------------------
// Resume builder
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone)]
pub struct ResumeDoc {
    #[serde(rename = "_id")]
    pub id: String,
    #[serde(default)]
    pub user_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_id: Option<String>,
    pub title: String,
    #[serde(default)]
    pub profile: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template_id: Option<String>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

#[derive(Deserialize)]
#[allow(dead_code)]
pub struct CreateResumeRequest {
    #[serde(default)]
    pub chat_id: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub profile: Value,
    #[serde(default)]
    pub template_id: Option<String>,
}

#[derive(Deserialize)]
pub struct PatchResumeRequest {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub profile: Option<Value>,
    #[serde(default)]
    pub template_id: Option<String>,
}

#[derive(Deserialize)]
pub struct ResumeChatRequest {
    pub content: String,
}

// ---------------------------------------------------------------------------
// Credits & token usage
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone)]
pub struct CreditDoc {
    #[serde(rename = "_id")]
    pub id: String,
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub tokens: u64,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct TokenUsageDoc {
    #[serde(rename = "_id")]
    pub id: String,
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub date: String,
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default)]
    pub total_tokens: u64,
    #[serde(default)]
    pub cost_usd: f64,
    #[serde(default)]
    pub charged_usd: f64,
    #[serde(default)]
    pub created_at: String,
}

// ---------------------------------------------------------------------------
// Payments
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone)]
pub struct PaymentDoc {
    #[serde(rename = "_id")]
    pub id: String,
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub amount_usd: f64,
    #[serde(default)]
    pub tokens: u64,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub reference: String,
    #[serde(default)]
    pub provider_ref: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

#[derive(Deserialize)]
pub struct InitiatePaymentRequest {
    pub amount_usd: f64,
}

#[derive(Deserialize)]
pub struct WaafiCallback {
    #[serde(default)]
    pub reference: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
}

// ---------------------------------------------------------------------------
// Notifications
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone)]
pub struct NotificationDoc {
    #[serde(rename = "_id")]
    pub id: String,
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub notification_type: String,
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub read: bool,
    #[serde(default)]
    pub created_at: String,
}

// ---------------------------------------------------------------------------
// Chat
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone)]
pub struct ChatTurn {
    pub role: String,
    pub content: String,
    pub created_at: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ChatDoc {
    #[serde(rename = "_id")]
    pub id: String,
    #[serde(default)]
    pub user_id: String,
    pub title: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub turns: Vec<ChatTurn>,
    #[serde(default)]
    pub profile: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template_id: Option<String>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

#[derive(Deserialize)]
pub struct CreateChatRequest {
    #[serde(default)]
    pub title: Option<String>,
}

#[derive(Deserialize)]
pub struct SelectTemplateRequest {
    pub template_id: String,
}

#[derive(Deserialize)]
pub struct ChatMessageRequest {
    pub content: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usd_to_tokens_one_dollar() {
        assert_eq!(usd_to_tokens(1.0), 1_100_000);
    }

    #[test]
    fn usd_to_tokens_zero_and_negative_floor_to_zero() {
        assert_eq!(usd_to_tokens(0.0), 0);
        assert_eq!(usd_to_tokens(-5.0), 0);
    }

    #[test]
    fn usd_to_tokens_half_dollar_is_exact() {
        assert_eq!(usd_to_tokens(0.5), 550_000);
    }

    #[test]
    fn tokens_to_usd_roundtrips() {
        assert_eq!(tokens_to_usd(1_100_000), 1.0);
        assert_eq!(tokens_to_usd(0), 0.0);
    }

    #[test]
    fn pricing_margin_is_consistent() {
        let expected = COST_PER_TOKEN / (1.0 - PROFIT_MARGIN);
        assert!((PRICE_PER_TOKEN - expected).abs() < f64::EPSILON);
    }

    #[test]
    fn template_id_validation() {
        assert!(is_valid_template_id("modern"));
        assert!(is_valid_template_id("professional"));
        assert!(!is_valid_template_id("nope"));
        assert!(!is_valid_template_id(""));
    }
}
