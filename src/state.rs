use mongodb::Database;

use crate::config::Config;

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
}

impl AppState {
    pub fn new(db: Database, config: Config) -> Self {
        let mailer = crate::mail::Mailer::new(crate::mail::MailConfig::from_env());
        // 15 attempts per 5 minutes per address. A person retrying a link never gets near
        // this; a script walking the space is stopped on its first minute.
        let magic_limiter = std::sync::Arc::new(crate::links::RateLimiter::new(
            std::time::Duration::from_secs(300),
            15,
        ));
        Self {
            db,
            config,
            mailer,
            magic_limiter,
        }
    }

    pub fn jobs(&self) -> mongodb::Collection<crate::models::JobDoc> {
        self.db.collection("jobs")
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
}
