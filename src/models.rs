use serde::{Deserialize, Serialize};
use serde_json::Value;

/// What a million tokens actually cost us, as tokens-per-dollar. Measured, not assumed:
/// DeepInfra reports `estimated_cost` per call, and google/gemini-3.7-flash came back at ~$1.21
/// per 1M blended (reasoning tokens bill as output, which is most of the surprise). The previous
/// value, 5,500,000 ($0.18/1M), was a DeepSeek-era assumption and understated our cost 6.7x —
/// which is how a "80% margin" was reported while the product was selling below cost.
pub const TOKENS_PER_USD_AT_COST: u64 = 826_446;
/// Tokens a customer gets for $1. DERIVED, not chosen: cost rate x (1 - margin). It is written
/// out rather than computed because the ledger reads it as a constant, and a test below pins the
/// two together — they drifted apart once and the margin went negative without anything failing.
pub const TOKENS_PER_USD_BILLED: u64 = 413_223;
pub const PROFIT_MARGIN: f64 = 0.5;
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
    /// Always serialised, including as null. A client cannot tell "no closing date was listed"
    /// from "the field is missing" if the key vanishes, and a job list needs to render those two
    /// differently.
    #[serde(default)]
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
    /// Always serialised, including as null. A client cannot tell "no closing date was listed"
    /// from "the field is missing" if the key vanishes, and a job list needs to render those two
    /// differently.
    #[serde(default)]
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

/// One live code per (user, purpose), `_id` = "user_id:purpose" so the uniqueness that "only
/// the newest code of this kind can be used" needs comes from the primary key rather than an
/// extra index. Only the hash is stored: the code itself exists once, in the email.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct EmailOtpDoc {
    #[serde(rename = "_id")]
    pub id: String,
    pub user_id: String,
    /// `verify_email` or `reset_password` (see `otp::PURPOSE_*`). Kept explicit so a code
    /// issued for one flow can never satisfy the other.
    #[serde(default = "default_otp_purpose")]
    pub purpose: String,
    pub email: String,
    pub code_hash: String,
    pub expires_at: String,
    /// Wrong guesses so far. At `otp::OTP_MAX_ATTEMPTS` the code is dead and only a resend
    /// can produce a working one.
    #[serde(default)]
    pub attempts: i32,
    /// When a code was last sent, for the resend cooldown.
    pub last_sent_at: String,
    pub created_at: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct VerifyEmailRequest {
    pub code: String,
}

fn default_otp_purpose() -> String {
    crate::otp::PURPOSE_VERIFY.to_string()
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ForgotPasswordRequest {
    pub email: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ResetPasswordRequest {
    pub email: String,
    pub code: String,
    pub new_password: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ResendOtpRequest {
    /// Accepted but ignored: the account comes from the token, never from the body, so this
    /// endpoint cannot be aimed at somebody else's mailbox.
    #[serde(default)]
    pub email: Option<String>,
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
    /// How this account signs in: `"password"` today, `"google"`/`"apple"` once
    /// those land. An OAuth account carries no `password_hash`, so password
    /// login must refuse it rather than compare against an empty hash.
    #[serde(default = "default_provider")]
    pub provider: String,
    /// The provider's own subject id (Google/Apple `sub`). Absent for password
    /// accounts, which is why the uniqueness index on it is partial.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_id: Option<String>,
    #[serde(default)]
    pub email_verified: bool,
    /// "en" or "so". Remembered so a returning user is not asked for their language on every
    /// new chat — the whole point of the session state the assistant reads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
    /// Settings-screen fields. Separate from the CV profile, which the
    /// interview extracts per chat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headline: Option<String>,
    #[serde(default)]
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    /// What to match this user against. Derived from the CV conversation (or a
    /// manual category pick), and the thing a notifier reads to decide whether
    /// the user has anything to be matched on yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub match_profile: Option<MatchProfile>,
    /// Whether and how this user wants job alerts. Absent means never asked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alerts: Option<AlertPrefs>,
    #[serde(default)]
    pub created_at: String,
}

/// Accounts created before provider tracking existed are password accounts,
/// and so is every account this service creates itself.
pub fn default_provider() -> String {
    "password".to_string()
}

/// A user's job-alert subscription, and the negative signals that shape it.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AlertPrefs {
    #[serde(default)]
    pub enabled: bool,
    /// "instant" or "daily".
    #[serde(default)]
    pub mode: String,
    /// When the last batch was queued, so a daily run happens at most once a day.
    #[serde(default)]
    pub last_sent_at: String,
    /// Employers the user rejected with "not a fit". Matched case-insensitively
    /// against `organization`, and kept here rather than inferred so the signal is
    /// visible and reversible.
    #[serde(default)]
    pub excluded_employers: Vec<String>,
    #[serde(default)]
    pub excluded_categories: Vec<String>,
}

/// One role the user has held, compacted for the memory record: no description, no
/// dates-as-strings mess, just enough to say what they have done.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoryExperience {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub organization: String,
    /// Free text as given ("2023-2026", "3 years"). Not parsed: the memory is prose.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub period: String,
}

/// The compacted, durable answer to "who is this user?", one document per user.
///
/// Separate from `UserDoc` on purpose. The user document carries what the ACCOUNT needs
/// (credentials, role, alert prefs) and is read on every `/auth/me` and every `/jobs`
/// location fallback, so it must stay small and boring. This record carries what a
/// CONVERSATION needs — skills, history, what they are looking for, what they refused —
/// and it has to survive the things that are only context: a deleted chat, a deleted CV,
/// a reset conversation. Recomputing it per turn from those sources means the memory dies
/// with them, which is exactly what a "memory" must not do.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UserMemoryDoc {
    /// The user id, so one user has exactly one record.
    #[serde(rename = "_id")]
    pub id: String,
    // Identity. Taken from the account, because that is what the user themselves chose.
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub email: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headline: Option<String>,
    // Professional shape, from their CVs and conversations.
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub experience: Vec<MemoryExperience>,
    /// Highest education as one line ("BSc, Public Health — University of Hargeisa").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub education: Option<String>,
    #[serde(default)]
    pub languages: Vec<String>,
    // Preferences and negative signals.
    #[serde(default)]
    pub preferred_locations: Vec<String>,
    /// Employers the user asked not to see. A memory that keeps offering a rejected
    /// employer is worse than having no memory at all.
    #[serde(default)]
    pub avoid_employers: Vec<String>,
    /// The compacted "who is this user" paragraph, bounded. This is the field a future
    /// consumer (a bot, a digest mailer, a support screen) reads first.
    #[serde(default)]
    pub memo: String,
    /// What fed this record, oldest first: `account`, `resume:<id>`, `chat:<id>`.
    #[serde(default)]
    pub sources: Vec<String>,
    #[serde(default)]
    pub revision: u32,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

/// One job saved by one user.
///
/// A collection rather than an array on the user document: the user document is
/// read by every `/auth/me` and by every `/jobs` call that falls back to the
/// profile location, and a list that grows with browsing does not belong in the
/// document every request already reads.
#[derive(Serialize, Deserialize, Clone)]
pub struct SavedJobDoc {
    #[serde(rename = "_id")]
    pub id: String,
    pub user_id: String,
    pub job_id: String,
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

/// PATCH /auth/me body. Every field is optional; a field that is present but
/// blank clears the stored value, and an absent field is left alone.
#[derive(Deserialize)]
pub struct UpdateProfileRequest {
    pub name: Option<String>,
    pub phone: Option<String>,
    pub location: Option<String>,
    pub headline: Option<String>,
    /// Accepted only so a change can be refused with a reason. Email is the
    /// sign-in address and there is no verification mail to prove ownership of
    /// a new one, so letting it be edited here would let anyone claim an
    /// address they do not own.
    #[allow(dead_code)]
    pub email: Option<String>,
}

#[derive(Serialize)]
pub struct UserResponse {
    pub id: String,
    pub name: String,
    pub email: String,
    pub role: String,
    /// So a client can tell a password account from an OAuth one before it
    /// offers a password form it would be refused.
    pub provider: String,
    pub email_verified: bool,
    pub location: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headline: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
    /// Always serialised, unlike the fields above: the client uses it to restore the language tab
    /// when the app starts, so "absent" and "not chosen yet" must not look the same on the wire.
    #[serde(default)]
    pub preferred_language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub match_profile: Option<MatchProfile>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alerts: Option<AlertPrefs>,
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
    /// Tokens a delivered-but-unpaid request consumed. Recorded instead of letting
    /// the balance go negative, so the shortfall stays visible in reporting rather
    /// than silently vanishing.
    #[serde(default)]
    pub debt_tokens: u64,
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
    /// The job this notification refers to, for job matches. Paired with
    /// `user_id` in a partial unique index so the same job is never queued
    /// twice for one user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub read: bool,
    /// When the bot handed this to the user. `read` is not the same thing: the bot
    /// must be able to ask for what it has not delivered yet, or it either re-sends
    /// or silently misses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_at: Option<String>,
    /// Why delivery failed, when it did. Set instead of leaving the notification pending
    /// forever: a blocked bot or a deleted account will refuse every retry, and an
    /// endless retry loop looks identical to a working one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed_reason: Option<String>,
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
    /// "assistant" (the sequenced conversation: language, then intent, then jobs),
    /// "job_search" or "cv". Decides which system prompt the turn uses.
    #[serde(default)]
    pub purpose: String,
    /// "en" or "so" — the language chosen inside this conversation. Held on the chat so a
    /// mid-conversation switch sticks, and copied from the account so a returning user is
    /// not asked again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default)]
    pub turns: Vec<ChatTurn>,
    #[serde(default)]
    pub profile: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template_id: Option<String>,
    /// Tokens this session has consumed, counted from real provider usage. Bounded
    /// by `CHAT_SESSION_TOKEN_BUDGET`: capping each request still leaves a session
    /// unbounded, because a CV is many turns rather than one call.
    #[serde(default)]
    pub tokens_used: u64,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

#[derive(Deserialize)]
pub struct CreateChatRequest {
    /// "en" or "so" from the client's language tab. Optional: absent means English, and a
    /// mid-chat switch is still picked up from what the user writes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    /// What the conversation is for: "cv" (default) or "job_search". Selects the system
    /// prompt, so a job seeker is never interrogated like a CV writer.
    #[serde(default)]
    pub purpose: Option<String>,
    /// Start the conversation with the agent speaking. The CV tab opens straight into an
    /// interview, and an empty screen with a blinking cursor is a worse first impression than the
    /// agent saying hello first. Handled like a normal turn in every other way: the reply streams,
    /// is charged, and is stored — only the user's side of it is not stored, because the user
    /// never wrote anything.
    #[serde(default)]
    pub opening: bool,
}

#[derive(Deserialize)]
pub struct SelectTemplateRequest {
    pub template_id: String,
}

#[derive(Deserialize)]
pub struct ChatMessageRequest {
    /// Set by the client when the user has only opened a chat and written nothing. The server
    /// writes the kickoff itself and does not store a user turn for it.
    #[serde(default)]
    pub opening: bool,
    /// Absent on an opening turn — there is nothing for the user to have written. Required
    /// otherwise, and a normal request that omits it is still refused where emptiness matters.
    #[serde(default)]
    pub content: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usd_to_tokens_one_dollar() {
        assert_eq!(usd_to_tokens(1.0), TOKENS_PER_USD_BILLED);
    }

    #[test]
    fn usd_to_tokens_zero_and_negative_floor_to_zero() {
        assert_eq!(usd_to_tokens(0.0), 0);
        assert_eq!(usd_to_tokens(-5.0), 0);
    }

    #[test]
    fn usd_to_tokens_half_dollar_is_exact() {
        assert_eq!(usd_to_tokens(0.5), TOKENS_PER_USD_BILLED / 2);
    }

    #[test]
    /// The billed rate must be the cost rate discounted by the margin. These two numbers decide
    /// the price, and when they disagree the product sells below cost while every margin report
    /// still looks healthy — the failure this pins shut.
    #[test]
    fn the_billed_rate_is_the_cost_rate_minus_our_margin() {
        let derived = TOKENS_PER_USD_AT_COST as f64 * (1.0 - PROFIT_MARGIN);
        assert!(
            (derived - TOKENS_PER_USD_BILLED as f64).abs() / derived < 0.001,
            "TOKENS_PER_USD_BILLED ({TOKENS_PER_USD_BILLED}) is not TOKENS_PER_USD_AT_COST \
             ({TOKENS_PER_USD_AT_COST}) x (1 - {PROFIT_MARGIN}) = {derived}"
        );
        // and the price has to be above what we pay, or the margin is not a margin
        assert!(PRICE_PER_TOKEN > COST_PER_TOKEN);
    }

    #[test]
    fn tokens_to_usd_roundtrips() {
        assert_eq!(tokens_to_usd(413_223), 1.0);
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
