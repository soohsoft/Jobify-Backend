use mongodb::Database;

use crate::config::Config;
use crate::llm::LlmClient;

#[derive(Clone)]
pub struct AppState {
    pub db: Database,
    pub config: Config,
    /// Built once at startup from the environment. With `SMTP_HOST` empty it logs
    /// instead of sending, which keeps the signup flow testable before credentials exist.
    pub mailer: crate::mail::Mailer,
    /// Coarse per-IP attempt limit for the public magic-link exchange. In memory on
    /// purpose: it guards one endpoint against guessing, and a token is single-use anyway.
    pub magic_limiter: std::sync::Arc<crate::links::RateLimiter>,
    /// The provider call behind the assistant. A trait object so tests inject a stub and
    /// never touch the network.
    pub llm: std::sync::Arc<dyn LlmClient>,
    /// 20 smart queries per user per 10 minutes. In memory for the same reason as the
    /// magic-link limit: it is a spend guard, not an account policy, and a restart only
    /// resets a window that already refills in ten minutes.
    pub assistant_limiter: std::sync::Arc<crate::links::RateLimiter>,
}

impl AppState {
    pub fn new(db: Database, config: Config) -> Self {
        let llm = std::sync::Arc::new(crate::llm::HttpLlmClient::from_config(&config));
        Self::with_llm(db, config, llm)
    }

    /// The same state with an injected LLM client. Only tests use this.
    pub fn with_llm(db: Database, config: Config, llm: std::sync::Arc<dyn LlmClient>) -> Self {
        let mailer = crate::mail::Mailer::new(crate::mail::MailConfig::from_env());
        // 15 attempts per 5 minutes per address. A person retrying a link never gets near
        // this; a script walking the space is stopped on its first minute.
        let magic_limiter = std::sync::Arc::new(crate::links::RateLimiter::new(
            std::time::Duration::from_secs(300),
            15,
        ));
        let assistant_limiter = std::sync::Arc::new(crate::links::RateLimiter::new(
            crate::assistant::RATE_WINDOW,
            crate::assistant::RATE_MAX,
        ));
        Self {
            db,
            config,
            mailer,
            magic_limiter,
            llm,
            assistant_limiter,
        }
    }

    pub fn jobs(&self) -> mongodb::Collection<crate::models::JobDoc> {
        self.db.collection("jobs")
    }

    /// Tenders live in their own collection with the SAME document shape as `jobs`; which
    /// collection a row is in is the only thing that now says what it is. Nothing stored
    /// carries a `kind` field.
    pub fn tenders(&self) -> mongodb::Collection<crate::models::JobDoc> {
        self.db.collection("tenders")
    }

    pub fn users(&self) -> mongodb::Collection<crate::models::UserDoc> {
        self.db.collection("users")
    }

    pub fn notifications(&self) -> mongodb::Collection<crate::models::NotificationDoc> {
        self.db.collection("notifications")
    }

    pub fn saved_jobs(&self) -> mongodb::Collection<crate::models::SavedJobDoc> {
        self.db.collection("saved_jobs")
    }

    pub fn email_otps(&self) -> mongodb::Collection<crate::models::EmailOtpDoc> {
        self.db.collection("email_otps")
    }

    pub fn sources(&self) -> mongodb::Collection<crate::models::SourceConfig> {
        self.db.collection("sources")
    }

    /// Discord snowflake -> Jobify user. One row per Discord account.
    pub fn discord_links(&self) -> mongodb::Collection<crate::models::DiscordLinkDoc> {
        self.db.collection("discord_links")
    }

    /// One-time portal links, `_id` = SHA-256 of the raw token.
    pub fn link_tokens(&self) -> mongodb::Collection<crate::models::LinkTokenDoc> {
        self.db.collection("link_tokens")
    }

    /// `_id` = user id, so one subscription per account comes from the primary key.
    pub fn subscriptions(&self) -> mongodb::Collection<crate::models::SubscriptionDoc> {
        self.db.collection("subscriptions")
    }

    /// Legacy-shaped payment rows; the pre-bot collection already holds nine of them.
    pub fn payments(&self) -> mongodb::Collection<crate::models::PaymentDoc> {
        self.db.collection("payments")
    }

    pub fn wallets(&self) -> mongodb::Collection<crate::models::WalletDoc> {
        self.db.collection("wallets")
    }

    /// The query ledger. Read and written by `user_id` only — the collection holds both
    /// string and ObjectId `_id`s and this model never deserializes `_id`.
    pub fn credits(&self) -> mongodb::Collection<crate::models::CreditDoc> {
        self.db.collection("credits")
    }

    /// One row per answered smart query.
    pub fn assistant_usage(&self) -> mongodb::Collection<crate::models::AssistantUsageDoc> {
        self.db.collection("assistant_usage")
    }
}
