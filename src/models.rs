use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Jobs / scraper
// ---------------------------------------------------------------------------

/// A stored job. Field names are the database contract: the live collection already
/// holds documents in exactly this shape, so renaming anything here is a migration.
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
    /// Always serialised, including as null. A client cannot tell "no closing date was
    /// listed" from "the field is missing" if the key vanishes, and a job list renders
    /// those two differently.
    #[serde(default)]
    pub deadline: Option<String>,
    #[serde(default)]
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
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

/// What the scraper posts. Mirrors `scraper/src/types.ts` exactly.
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
    #[serde(default)]
    pub deadline: Option<String>,
    #[serde(default)]
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
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

/// A scrape source as `GET /internal/sources` returns it. `_id` is not modelled: the
/// scraper reads `id`, and the ObjectId is a storage detail.
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

// ---------------------------------------------------------------------------
// Users
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct MatchProfile {
    /// Canonical category slugs, validated through `categories::Category`.
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub locations: Vec<String>,
    #[serde(default)]
    pub updated_at: String,
}

/// `PUT /me/match-profile`. Every field defaults, so a caller may send only what it has —
/// but the request REPLACES the stored profile rather than merging into it: the three lists
/// are the whole profile, and an omitted list is an empty one.
#[derive(Deserialize, Clone, Default)]
pub struct MatchProfileRequest {
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub locations: Vec<String>,
}

/// A user's job-alert subscription and the negative signals that shape it.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AlertPrefs {
    #[serde(default)]
    pub enabled: bool,
    /// "instant" or "daily".
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub last_sent_at: String,
    #[serde(default)]
    pub excluded_employers: Vec<String>,
    #[serde(default)]
    pub excluded_categories: Vec<String>,
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
    /// How this account signs in. Defaulted rather than required: every account in the
    /// live collection predates the field, and all of them are password accounts.
    #[serde(default = "default_provider")]
    pub provider: String,
    #[serde(default)]
    pub email_verified: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headline: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
    #[serde(default)]
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub match_profile: Option<MatchProfile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alerts: Option<AlertPrefs>,
    #[serde(default)]
    pub created_at: String,
}

pub fn default_provider() -> String {
    "password".to_string()
}

/// One job saved by one user. A collection rather than an array on the user document:
/// that document is read by every `/auth/me`, and a list that grows with browsing does
/// not belong there.
#[derive(Serialize, Deserialize, Clone)]
pub struct SavedJobDoc {
    #[serde(rename = "_id")]
    pub id: String,
    pub user_id: String,
    pub job_id: String,
    #[serde(default)]
    pub created_at: String,
}

/// One live code per (user, purpose); `_id` = "user_id:purpose", so "only the newest code
/// of this kind can be used" comes from the primary key rather than an extra index.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct EmailOtpDoc {
    #[serde(rename = "_id")]
    pub id: String,
    pub user_id: String,
    #[serde(default = "default_otp_purpose")]
    pub purpose: String,
    pub email: String,
    pub code_hash: String,
    pub expires_at: String,
    #[serde(default)]
    pub attempts: i32,
    pub last_sent_at: String,
    pub created_at: String,
}

fn default_otp_purpose() -> String {
    crate::otp::PURPOSE_VERIFY.to_string()
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
    /// The job this refers to, for matches. Paired with `user_id` in a partial unique
    /// index so the same job is never queued twice for one user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub read: bool,
    /// When the sender handed this over. `read` is not the same thing: the queue has to
    /// be able to ask for what has not been delivered yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed_reason: Option<String>,
    #[serde(default)]
    pub created_at: String,
}

// ---------------------------------------------------------------------------
// Request / response bodies
// ---------------------------------------------------------------------------

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

#[derive(Deserialize)]
pub struct VerifyEmailRequest {
    pub code: String,
}

#[derive(Deserialize)]
pub struct ResendOtpRequest {
    /// Accepted but ignored: the account comes from the token, never from the body, so
    /// this endpoint cannot be aimed at somebody else's mailbox.
    #[serde(default)]
    #[allow(dead_code)]
    pub email: Option<String>,
}

#[derive(Deserialize)]
pub struct ForgotPasswordRequest {
    pub email: String,
}

#[derive(Deserialize)]
pub struct ResetPasswordRequest {
    pub email: String,
    pub code: String,
    pub new_password: String,
}

/// PATCH /auth/me. An absent field is left alone; a present-but-blank field is cleared.
#[derive(Deserialize)]
pub struct UpdateProfileRequest {
    pub name: Option<String>,
    pub phone: Option<String>,
    pub location: Option<String>,
    pub headline: Option<String>,
    /// Accepted only so it can be refused with a reason: email is the sign-in address.
    #[serde(default)]
    pub email: Option<String>,
}

#[derive(Serialize)]
pub struct UserResponse {
    pub id: String,
    pub name: String,
    pub email: String,
    pub role: String,
    pub provider: String,
    pub email_verified: bool,
    pub location: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headline: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub match_profile: Option<MatchProfile>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alerts: Option<AlertPrefs>,
}

impl UserResponse {
    pub fn from_doc(user: &UserDoc) -> Self {
        Self {
            id: user.id.clone(),
            name: user.name.clone(),
            email: user.email.clone(),
            role: user.role.clone(),
            provider: user.provider.clone(),
            email_verified: user.email_verified,
            location: user.location.clone(),
            phone: user.phone.clone(),
            headline: user.headline.clone(),
            avatar_url: user.avatar_url.clone(),
            match_profile: user.match_profile.clone(),
            alerts: user.alerts.clone(),
        }
    }
}
