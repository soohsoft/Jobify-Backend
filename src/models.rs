use mongodb::bson::DateTime as BsonDateTime;
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
    /// The country the location text resolves to (`"so"`, `"dj"`, …) or null when the text
    /// names no place the mapping knows. Derived once at ingest by `src/country.rs` and
    /// stored, so every list filters on a field instead of re-parsing free text.
    #[serde(default)]
    pub country: Option<String>,
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
    /// When a deadline backfill last visited this posting's page: set whether it found a
    /// closing date or found none. Without it the backfill would re-visit the same undated
    /// rows on every run, since "no deadline" is also what a row it has never looked at says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline_checked_at: Option<String>,
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
    /// Optional on the wire and a ROUTING instruction, not stored data: "tender" sends the
    /// document to the `tenders` collection, anything else (absent, blank, mis-cased, or an
    /// unknown label) to `jobs`. A listing is never rejected over a label.
    #[serde(default)]
    pub kind: Option<String>,
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
    /// Canonical category slugs, validated through `categories::Category`. These are the
    /// matcher's QUERY FILTER, so a slug nothing stores hides every row.
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub locations: Vec<String>,
    /// Where `categories` came from: "llm" when the one completion-time
    /// model call decided them, "rules" when the deterministic field-of-study map did
    /// (no provider, a failed call, or nothing usable back). Empty on documents written
    /// before the backend started deciding, which is why readers must treat "" as unknown
    /// rather than as a third source.
    #[serde(default)]
    pub category_source: String,
    /// Where the member wants to work: any of "ngo", "government", "private".
    ///
    /// A SOFT signal, not a filter: a job row carries no employer-type field, so this can only
    /// nudge the score when the organisation name looks like one of the three. Empty = no
    /// preference recorded.
    #[serde(default)]
    pub employer_types: Vec<String>,
    /// The categories the member asked to be notified about, chosen explicitly after their last
    /// job. Empty means "follow my education" (the derived `categories`). Non-empty WIDENS the
    /// gate to the union, so picking a second field never hides the first.
    #[serde(default)]
    pub positions: Vec<String>,
    /// Fingerprint of the facts the derivation READ (education, last position, years).
    /// Stored so a re-save of a complete profile with unchanged facts reuses the stored
    /// decision instead of paying for the model again on every partial save. Backend
    /// bookkeeping, so it is omitted from responses when empty.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub facts_key: String,
    #[serde(default)]
    pub updated_at: String,
}

impl MatchProfile {
    /// Every category a row may arrive in: the derived ones plus anything the member picked
    /// explicitly as a notification position. `positions` WIDENS the gate — picking a second
    /// field must never hide the first.
    pub fn effective_categories(&self) -> Vec<String> {
        let mut out = self.categories.clone();
        for slug in &self.positions {
            if !out.iter().any(|seen| seen.eq_ignore_ascii_case(slug)) {
                out.push(slug.clone());
            }
        }
        out
    }

    /// True when the member has nothing to match on yet.
    pub fn is_blank(&self) -> bool {
        self.categories.is_empty() && self.positions.is_empty()
    }
}

/// The categories the member was decided to be matched on, for a reader that must not
/// guess. `source` is "llm" or "rules" as stored.
#[derive(Serialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct MatchAreas {
    pub categories: Vec<String>,
    pub source: String,
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
    #[serde(default)]
    pub employer_types: Vec<String>,
    #[serde(default)]
    pub positions: Vec<String>,
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
    /// Set when the member themselves switched alerts off (`/stop`, `/alerts off`).
    ///
    /// Finishing onboarding turns alerts on by default, and re-running the interview must not
    /// undo an explicit "no": this flag is the difference between "never asked" and "said no",
    /// which `enabled: false` alone cannot express. Defaulted, so the documents written before
    /// it existed read as "never said no".
    #[serde(default)]
    pub user_disabled: bool,
    #[serde(default)]
    pub excluded_employers: Vec<String>,
    #[serde(default)]
    pub excluded_categories: Vec<String>,
}

/// One degree or certificate, in the order the member answered. `field` is the canonical label
/// from `onboarding::FIELDS` — or `Other: <the member's own words>` — and is empty for a level
/// that is not asked about a field (`none`, `primary`, `secondary`, `diploma`).
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct EducationEntry {
    #[serde(default)]
    pub level: String,
    #[serde(default)]
    pub field: String,
}

/// The employer/vendor side of an account. Absent for a job seeker; set through
/// `PATCH /me/company`, and cleared by sending an empty name.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct CompanyProfile {
    pub name: String,
    #[serde(default)]
    pub services: Vec<String>,
    /// Where the company is based, chosen from the Somali city list in the bot. Tender matching
    /// does not require it, but employers expect it on the profile the portal shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub website: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub education_level: Option<String>,
    /// The interview's answer set: levels with the field of study, in the order given (max 4).
    /// Defaulted, so the ten accounts written before this existed read as an empty list.
    #[serde(default)]
    pub education: Vec<EducationEntry>,
    /// The most recent job title, as the member typed it. Feeds the matcher's keywords.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_position: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub experience_years: Option<u8>,
    /// Canonical category slugs, the same vocabulary `match_profile.categories` uses.
    #[serde(default)]
    pub sectors: Vec<String>,
    #[serde(default)]
    pub skills: Vec<String>,
    /// What this member asked to be alerted about: "jobs", "tenders" or "both". Absent on
    /// every account that predates the field, which reads as "both" — so nothing needs
    /// migrating and the web accounts keep their current behaviour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wants: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub company: Option<CompanyProfile>,
    /// The immutable Discord snowflake this account belongs to. Absent on the nine
    /// password accounts that predate the bot; `discord_links` is the reverse index.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discord_id: Option<String>,
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
// Discord identity, portal links, subscriptions, payments
// ---------------------------------------------------------------------------

/// One Discord account, bound to exactly one Jobify user. `discord_id` is the immutable
/// snowflake the bot sends; it is also mirrored onto the user document for reads.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DiscordLinkDoc {
    #[serde(rename = "_id")]
    pub id: String,
    pub discord_id: String,
    pub user_id: String,
    #[serde(default)]
    pub username: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub global_name: Option<String>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

/// One live portal link. `_id` is the SHA-256 hex of the raw token, never the token
/// itself: the raw value is returned to the bot exactly once and cannot be read back.
///
/// The timestamps are BSON dates rather than RFC3339 strings because the TTL index on
/// `expires_at` only expires Date values — a string would leave dead rows forever. The
/// exchange query also filters `expires_at`, so single use never depends on the TTL
/// monitor having run.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct LinkTokenDoc {
    #[serde(rename = "_id")]
    pub id: String,
    pub user_id: String,
    pub discord_id: String,
    pub created_at: BsonDateTime,
    pub expires_at: BsonDateTime,
    #[serde(default)]
    pub used_at: Option<BsonDateTime>,
}

pub const SUBSCRIPTION_ACTIVE: &str = "active";
pub const SUBSCRIPTION_EXPIRED: &str = "expired";
pub const SUBSCRIPTION_NONE: &str = "none";

/// $1/month, one row per user (`_id` = user id). Absent means `status: "none"`; the
/// route reports that rather than creating a row nobody asked for.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SubscriptionDoc {
    #[serde(rename = "_id")]
    pub user_id: String,
    #[serde(default = "default_subscription_status")]
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub period_start: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub period_end: Option<String>,
    #[serde(default = "default_subscription_amount")]
    pub amount_usd: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default)]
    pub auto_renew: bool,
    #[serde(default)]
    pub updated_at: String,
}

pub const SUBSCRIPTION_AMOUNT_USD: f64 = 1.0;

fn default_subscription_status() -> String {
    SUBSCRIPTION_NONE.to_string()
}

fn default_subscription_amount() -> f64 {
    SUBSCRIPTION_AMOUNT_USD
}

impl SubscriptionDoc {
    /// What a caller with no row sees: an honest `none`, not a 404.
    pub fn none_for(user_id: &str) -> Self {
        Self {
            user_id: user_id.to_string(),
            status: SUBSCRIPTION_NONE.to_string(),
            period_start: None,
            period_end: None,
            amount_usd: SUBSCRIPTION_AMOUNT_USD,
            provider: None,
            auto_renew: false,
            updated_at: String::new(),
        }
    }
}

/// A payment attempt in the shape the collection already holds (nine legacy rows). New
/// rows add `provider`; legacy rows have no such field, so it is optional and every
/// existing document is left exactly as it is.
#[derive(Serialize, Deserialize, Clone, Debug)]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

// ---------------------------------------------------------------------------
// Credits and the assistant
// ---------------------------------------------------------------------------

/// The smart-query ledger for one account.
///
/// Units are QUERIES, not tokens: a smart question costs one, and nobody has to reason
/// about token pricing to understand their balance.
///
/// `_id` is deliberately NOT modelled. The nine legacy rows in the live collection have a
/// string uuid as `_id`, but any row created by a raw `updateOne(..., upsert)` carries an
/// ObjectId — and `CreditDoc.id: String` cannot deserialize a map. Every read and write
/// here is keyed on `user_id`, which is the same on both shapes, and `$setOnInsert` still
/// gives a new row a string id. The legacy `tokens` field is left untouched by every write.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct CreditDoc {
    #[serde(default)]
    pub user_id: String,
    /// Purchased queries that survive a period rollover.
    #[serde(default)]
    pub queries: i64,
    /// Queries spent from this period's free allowance. Reset to zero when the 30-day
    /// period ends; the paid balance is not touched by a rollover.
    #[serde(default)]
    pub allowance_used: i64,
    #[serde(default)]
    pub period_start: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub period_end: Option<String>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

/// One row per answered smart query. A separate collection rather than the legacy
/// `token_usage` shape: that one is a per-user-per-day token aggregate with `charged_usd`,
/// and inventing a second meaning for its fields would make both unreadable. This row
/// answers "what did this question cost, and in which bucket" and nothing else.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AssistantUsageDoc {
    #[serde(rename = "_id")]
    pub id: String,
    pub user_id: String,
    pub at: String,
    /// `search`, `help` or `status`. `unsupported` is never charged, so it is not recorded
    /// as usage.
    pub intent: String,
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default)]
    pub charged_queries: i64,
    #[serde(default)]
    pub outcome: String,
}

/// `POST /assistant/query`. The only input is what the member said; identity comes from
/// the token, never from the body.
#[derive(Deserialize)]
pub struct AssistantQueryRequest {
    pub message: String,
}

/// `POST /internal/assistant/query` — the bot's variant, since a Discord member has no
/// bearer token.
#[derive(Deserialize)]
pub struct DiscordAssistantQueryRequest {
    pub discord_id: String,
    pub message: String,
}

/// `POST /credits/topup` (internal key only until WaafiPay is wired). Exactly one of
/// `discord_id` / `user_id` identifies the account.
#[derive(Deserialize)]
pub struct CreditTopupRequest {
    #[serde(default)]
    pub discord_id: Option<String>,
    #[serde(default)]
    pub user_id: Option<String>,
    pub queries: i64,
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
    /// The 0–100 fit the job scored when it was queued, so the digest can show the same
    /// percentage the list did. Absent on rows queued before the score existed (and on
    /// payment notifications), which is why the reader treats it as optional.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub match_score: Option<i64>,
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

// ---------------------------------------------------------------------------
// Discord / portal / company requests
// ---------------------------------------------------------------------------

/// `POST /internal/discord/users`. `username` is the Discord handle; `global_name` is the
/// display name and is what a person expects to see.
#[derive(Deserialize)]
pub struct DiscordUserRequest {
    pub discord_id: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub global_name: Option<String>,
}

/// `POST /internal/discord/profile`. Every field is optional so the bot can send one
/// answer at a time; the match profile is derived from whatever arrives.
///
/// `education` is `Option<Vec<..>>` rather than a plain list on purpose: `None` means "this
/// answer was not part of the payload" and `Some(list)` means "this is the whole history",
/// so a partial save cannot wipe the degrees already recorded.
#[derive(Deserialize, Clone)]
pub struct DiscordProfileRequest {
    pub discord_id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub education_level: Option<String>,
    #[serde(default)]
    pub education: Option<Vec<EducationEntry>>,
    /// The last job title. A blank value clears it; an absent one leaves it alone.
    #[serde(default)]
    pub last_position: Option<String>,
    #[serde(default)]
    pub experience_years: Option<u8>,
    /// No longer asked by the interview (Jobify covers all of Somalia) and no longer part of
    /// the derived match profile, but still accepted and stored so an older client keeps working.
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub sectors: Vec<String>,
    /// "ngo" / "government" / "private", from the question asked after the last job.
    #[serde(default)]
    pub employer_types: Option<Vec<String>>,
    /// The categories the member chose for notifications. `None` keeps what is on file;
    /// `Some([])` means "all of my education categories".
    #[serde(default)]
    pub positions: Option<Vec<String>>,
    #[serde(default)]
    pub skills: Vec<String>,
    /// "jobs", "tenders" or "both". Omitted by the web client, which keeps "both".
    #[serde(default)]
    pub wants: Option<String>,
}

/// `POST /internal/discord/link-token`.
#[derive(Deserialize)]
pub struct LinkTokenRequest {
    pub discord_id: String,
    /// Defaults to 900 and is clamped to 300..=3600: a portal link is a convenience, not
    /// a password, and a week-long one is a standing key.
    #[serde(default)]
    pub ttl_seconds: Option<i64>,
}

/// `POST /internal/discord/company`. The same fields `PATCH /me/company` takes, with the
/// snowflake in place of the bearer token; both routes write through `auth::apply_company`.
#[derive(Deserialize)]
pub struct DiscordCompanyRequest {
    pub discord_id: String,
    /// An empty name clears the company, exactly as it does on the portal.
    pub name: String,
    #[serde(default)]
    pub services: Vec<String>,
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub website: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

/// `POST /internal/discord/alerts`. `/alerts` is Bearer-only, so the bot's toggle needs its
/// own identity-bearing route; both write through `alerts::apply_alert_prefs`.
#[derive(Deserialize)]
pub struct DiscordAlertsRequest {
    pub discord_id: String,
    pub enabled: bool,
    #[serde(default)]
    pub mode: Option<String>,
}

/// `POST /auth/magic`.
#[derive(Deserialize)]
pub struct MagicLinkRequest {
    pub token: String,
}

/// `PATCH /me/company`. An empty `name` clears the company.
#[derive(Deserialize)]
pub struct CompanyRequest {
    pub name: String,
    #[serde(default)]
    pub services: Vec<String>,
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub website: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub education_level: Option<String>,
    /// Always serialised (an empty list is meaningful: this account has no degrees recorded),
    /// so the bot can render the interview's answers back to the member.
    pub education: Vec<EducationEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_position: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub experience_years: Option<u8>,
    pub sectors: Vec<String>,
    pub skills: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wants: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub company: Option<CompanyProfile>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discord_id: Option<String>,
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
            education_level: user.education_level.clone(),
            education: user.education.clone(),
            last_position: user.last_position.clone(),
            experience_years: user.experience_years,
            sectors: user.sectors.clone(),
            skills: user.skills.clone(),
            wants: user.wants.clone(),
            company: user.company.clone(),
            discord_id: user.discord_id.clone(),
        }
    }
}
