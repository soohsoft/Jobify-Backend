use mongodb::Database;

use crate::config::Config;

#[derive(Clone)]
pub struct AppState {
    pub db: Database,
    pub config: Config,
    /// Built once at startup from the environment. With `SMTP_HOST` empty it logs
    /// instead of sending, which keeps the signup flow testable before credentials exist.
    pub mailer: crate::mail::Mailer,
}

impl AppState {
    pub fn new(db: Database, config: Config) -> Self {
        let mailer = crate::mail::Mailer::new(crate::mail::MailConfig::from_env());
        Self { db, config, mailer }
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
}
