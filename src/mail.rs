//! Outbound email over SMTP.
//!
//! Two modes, chosen by configuration rather than a build flag: with `SMTP_HOST` set the
//! mail goes out over SMTP (Gmail: `smtp.gmail.com`, 587, STARTTLS, app password); with it
//! empty every message is written to the log instead. Console mode is not a stub — it is
//! what lets the signup flow be exercised end to end before credentials exist.

use lettre::message::header::ContentType;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmtpSecurity {
    /// STARTTLS on the submission port (587) — what Gmail expects.
    StartTls,
    /// Implicit TLS from the first byte (465).
    ImplicitTls,
    /// No encryption. Dev-only, and it says so in the log.
    Plain,
}

impl SmtpSecurity {
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "tls" | "ssl" | "implicit" => SmtpSecurity::ImplicitTls,
            "none" | "plain" => SmtpSecurity::Plain,
            _ => SmtpSecurity::StartTls,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MailConfig {
    pub from: String,
    pub from_name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub security: SmtpSecurity,
}

impl MailConfig {
    pub fn from_env() -> Self {
        let host = std::env::var("SMTP_HOST").unwrap_or_default();
        let username = std::env::var("SMTP_USERNAME").unwrap_or_default();
        let port = std::env::var("SMTP_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(587);
        let from = std::env::var("SMTP_FROM")
            .ok()
            .filter(|v| !v.trim().is_empty())
            // Gmail refuses a From that is not the authenticated account, so falling
            // back to the username is what makes a half-filled .env work instead of bounce.
            .unwrap_or_else(|| username.clone());
        Self {
            from,
            from_name: std::env::var("SMTP_FROM_NAME").unwrap_or_else(|_| "Jobify".to_string()),
            host,
            port,
            username,
            password: std::env::var("SMTP_PASSWORD").unwrap_or_default(),
            security: SmtpSecurity::parse(
                &std::env::var("SMTP_TLS").unwrap_or_else(|_| "starttls".to_string()),
            ),
        }
    }

    /// Live SMTP needs host *and* credentials. Requiring all three stops a half-filled
    /// .env from switching the app into a mode where every send fails silently — the only
    /// symptom would be a code that never arrives and is never logged. Anything missing
    /// keeps console mode, where the code is at least readable.
    pub fn enabled(&self) -> bool {
        !self.host.trim().is_empty()
            && !self.username.trim().is_empty()
            && !self.password.trim().is_empty()
    }

    /// True when a host is configured but the credentials are not — almost always an
    /// unfinished .env rather than a deliberate choice.
    pub fn awaiting_credentials(&self) -> bool {
        !self.host.trim().is_empty() && !self.enabled()
    }
}

#[derive(Debug, Clone)]
pub struct Mailer {
    config: MailConfig,
}

impl Mailer {
    pub fn new(config: MailConfig) -> Self {
        Self { config }
    }

    pub fn is_live(&self) -> bool {
        self.config.enabled()
    }

    fn transport(&self) -> Result<AsyncSmtpTransport<Tokio1Executor>, String> {
        let builder = match self.config.security {
            SmtpSecurity::StartTls => {
                AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&self.config.host)
                    .map_err(|e| format!("smtp relay {}: {e}", self.config.host))?
            }
            SmtpSecurity::ImplicitTls => {
                AsyncSmtpTransport::<Tokio1Executor>::relay(&self.config.host)
                    .map_err(|e| format!("smtp relay {}: {e}", self.config.host))?
            }
            SmtpSecurity::Plain => {
                AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&self.config.host)
            }
        };

        let builder = builder.port(self.config.port);
        let builder = if self.config.username.trim().is_empty() {
            builder
        } else {
            builder.credentials(Credentials::new(
                self.config.username.clone(),
                self.config.password.clone(),
            ))
        };
        Ok(builder.build())
    }

    pub async fn send(&self, to: &str, subject: &str, body: &str) -> Result<(), String> {
        if !self.config.enabled() {
            // Deliberately loud and complete: in dev the log IS the delivery mechanism.
            tracing::info!(
                to = %to,
                subject = %subject,
                awaiting_credentials = self.config.awaiting_credentials(),
                "mail (console mode) — body follows on the next line"
            );
            for line in body.lines() {
                tracing::info!("mail> {}", line);
            }
            return Ok(());
        }

        let from = format!("{} <{}>", self.config.from_name, self.config.from)
            .parse()
            .map_err(|e| format!("invalid from address: {e}"))?;
        let message = Message::builder()
            .from(from)
            .to(to
                .parse()
                .map_err(|e| format!("invalid recipient {to}: {e}"))?)
            .subject(subject)
            .header(ContentType::TEXT_PLAIN)
            .body(body.to_string())
            .map_err(|e| format!("build message: {e}"))?;

        match self.transport()?.send(message).await {
            Ok(_) => {
                // Outside production the body is echoed even on success: a developer has
                // no way to read the mailbox the mail went to. In production nothing is
                // logged — the code is a secret only the recipient should see.
                if std::env::var("RUST_ENV").unwrap_or_default() != "production" {
                    tracing::info!(to = %to, "mail sent (dev: body logged below)");
                    for line in body.lines() {
                        tracing::info!("mail-dev> {}", line);
                    }
                }
                Ok(())
            }
            Err(err) => {
                // Outside production the code still has to be findable.
                if std::env::var("RUST_ENV").unwrap_or_default() != "production" {
                    tracing::warn!(
                        to = %to,
                        error = %err,
                        "SMTP send failed — logging the message so development is not blocked"
                    );
                    for line in body.lines() {
                        tracing::warn!("mail-fallback> {}", line);
                    }
                }
                Err(format!("smtp send to {to}: {err}"))
            }
        }
    }
}

/// The signup code mail. Plain text on purpose — an OTP that needs an HTML renderer is
/// one more thing that can arrive broken.
pub fn verification_email(code: &str, name: &str) -> (String, String) {
    let subject = format!("Your Jobify code: {code}");
    let greeting = if name.trim().is_empty() {
        "Hi,".to_string()
    } else {
        format!("Hi {},", name.trim())
    };
    let body = format!(
        "{greeting}\n\n\
         Your Jobify verification code is:\n\n\
         {code}\n\n\
         Enter it in the app to confirm this email address. The code expires in \
         {ttl} minutes and can be used once.\n\n\
         If you did not create a Jobify account, ignore this email — nothing was activated.\n\n\
         — Jobify",
        ttl = crate::otp::OTP_TTL_MINUTES
    );
    (subject, body)
}

/// The password-reset mail. Says a reset was requested and nothing about the account:
/// this mail can land in a shared inbox.
pub fn password_reset_email(code: &str, name: &str) -> (String, String) {
    let subject = format!("Reset your Jobify password: {code}");
    let greeting = if name.trim().is_empty() {
        "Hi,".to_string()
    } else {
        format!("Hi {},", name.trim())
    };
    let body = format!(
        "{greeting}\n\n\
         Someone asked to reset the password for your Jobify account.\n\n\
         Your reset code is:\n\n\
         {code}\n\n\
         Enter it in the app with your new password. The code expires in {ttl} minutes, can be \
         used once, and nothing has changed yet — your current password still works until you \
         finish.\n\n\
         If this was not you, ignore this email and consider changing your password; no one can \
         use this code without access to this inbox.\n\n\
         — Jobify",
        ttl = crate::otp::RESET_TTL_MINUTES
    );
    (subject, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(host: &str) -> MailConfig {
        MailConfig {
            from: "jobify@example.com".into(),
            from_name: "Jobify".into(),
            host: host.into(),
            port: 587,
            username: "jobify@example.com".into(),
            password: "app-password".into(),
            security: SmtpSecurity::StartTls,
        }
    }

    #[test]
    fn live_smtp_needs_host_and_credentials() {
        assert!(config("smtp.gmail.com").enabled());
        assert!(!config("").enabled());
        let mut no_password = config("smtp.gmail.com");
        no_password.password = String::new();
        assert!(!no_password.enabled());
        assert!(no_password.awaiting_credentials());
    }

    #[test]
    fn emails_carry_the_code() {
        let (subject, body) = verification_email("123456", "Halima");
        assert!(subject.contains("123456"));
        assert!(body.contains("123456"));
        assert!(body.contains("Hi Halima,"));
        let (subject, body) = password_reset_email("654321", "");
        assert!(subject.contains("654321"));
        assert!(body.contains("654321"));
    }
}
