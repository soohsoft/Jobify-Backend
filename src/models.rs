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
                    image: &str| SourceSelectors {
        card: card.to_string(),
        title: title.to_string(),
        organization: organization.to_string(),
        location: location.to_string(),
        description: description.to_string(),
        requirements: requirements.to_string(),
        posted_date: posted_date.to_string(),
        deadline: deadline.to_string(),
        image: image.to_string(),
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
            ),
        },
        SourceConfig {
            id: "somalijobs".to_string(),
            name: "SomaliJobs".to_string(),
            base_url: "https://somalijobs.com".to_string(),
            listing_url: "https://somalijobs.com/jobs".to_string(),
            source_type: "external".to_string(),
            enabled: false,
            selectors: selector(
                ".job-listing",
                ".job-title",
                ".company",
                ".job-location",
                ".job-summary",
                ".job-requirements",
                ".job-date",
                ".job-deadline",
                ".company-logo",
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
                ".vacancy-logo img",
            ),
        },
    ]
}

// ---------------------------------------------------------------------------
// Auth
// ---------------------------------------------------------------------------

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
