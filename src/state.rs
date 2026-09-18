use mongodb::Database;

use crate::config::Config;

#[derive(Clone)]
pub struct AppState {
    pub db: Database,
    pub config: Config,
    /// Built once at startup from the environment. With `SMTP_HOST` empty it logs instead of
    /// sending, which is what keeps the signup flow testable before credentials exist.
    pub mailer: crate::mail::Mailer,
}

impl AppState {
    pub fn new(db: Database, config: Config) -> Self {
        let mailer = crate::mail::Mailer::new(crate::mail::MailConfig::from_env());
        Self { db, config, mailer }
    }

    pub fn jobs(&self) -> mongodb::Collection<crate::models::JobDoc> {
        self.db.collection::<crate::models::JobDoc>("jobs")
    }

    pub fn users(&self) -> mongodb::Collection<crate::models::UserDoc> {
        self.db.collection::<crate::models::UserDoc>("users")
    }

    pub fn resumes(&self) -> mongodb::Collection<crate::models::ResumeDoc> {
        self.db.collection::<crate::models::ResumeDoc>("resumes")
    }

    pub fn credits(&self) -> mongodb::Collection<crate::models::CreditDoc> {
        self.db.collection::<crate::models::CreditDoc>("credits")
    }

    pub fn token_usage(&self) -> mongodb::Collection<crate::models::TokenUsageDoc> {
        self.db
            .collection::<crate::models::TokenUsageDoc>("token_usage")
    }

    pub fn payments(&self) -> mongodb::Collection<crate::models::PaymentDoc> {
        self.db.collection::<crate::models::PaymentDoc>("payments")
    }

    pub fn notifications(&self) -> mongodb::Collection<crate::models::NotificationDoc> {
        self.db
            .collection::<crate::models::NotificationDoc>("notifications")
    }

    pub fn chats(&self) -> mongodb::Collection<crate::models::ChatDoc> {
        self.db.collection::<crate::models::ChatDoc>("chats")
    }

    pub fn user_memory(&self) -> mongodb::Collection<crate::models::UserMemoryDoc> {
        self.db
            .collection::<crate::models::UserMemoryDoc>("user_memory")
    }

    pub fn saved_jobs(&self) -> mongodb::Collection<crate::models::SavedJobDoc> {
        self.db
            .collection::<crate::models::SavedJobDoc>("saved_jobs")
    }

    pub fn email_otps(&self) -> mongodb::Collection<crate::models::EmailOtpDoc> {
        self.db
            .collection::<crate::models::EmailOtpDoc>("email_otps")
    }

    pub fn sources(&self) -> mongodb::Collection<crate::models::SourceConfig> {
        self.db.collection::<crate::models::SourceConfig>("sources")
    }
}
