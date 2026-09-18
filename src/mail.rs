//! Outbound email for jobify, over SMTP.
//!
//! Two modes, chosen by configuration rather than by a build flag: with `SMTP_HOST` set the
//! mail goes out over SMTP (Gmail: `smtp.gmail.com`, port 587, STARTTLS, and an app password
//! — a normal account password is rejected); with it empty every message is written to the
//! log instead. The console mode is not a stub to be replaced later: it is what lets the
//! signup flow be exercised end to end before any mailbox credentials exist, and it keeps
//! the test suite free of a network dependency.
//!
//! A refusal from the mail server must not roll back a signup that already succeeded, so
//! `send_*` returns a Result the caller logs rather than propagates.

use lettre::message::header::ContentType;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmtpSecurity {
    /// STARTTLS on the submission port (587) — what Gmail expects.
    StartTls,
    /// Implicit TLS from the first byte (465).
    ImplicitTls,
    /// No encryption at all. Dev-only, and it says so in the log.
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
    /// Public origin used to build links in the mail (not used by the OTP text, which is
    /// code-only, but kept for the templates that will follow).
    pub app_base_url: String,
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
            // Gmail refuses a From that is not the authenticated account, so falling back
            // to the username is what makes a half-filled .env work instead of bouncing.
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
            app_base_url: std::env::var("APP_BASE_URL")
                .unwrap_or_else(|_| "http://localhost:5173".to_string()),
        }
    }

    /// Live SMTP needs host *and* credentials. Requiring all three is what stops a
    /// half-filled .env — host stored, password not yet — from switching the app into a mode
    /// where every send fails: signup would then mail nothing and log nothing, and the only
    /// symptom would be a code that never arrives. Anything missing keeps console mode,
    /// where the code is at least readable in the log.
    pub fn enabled(&self) -> bool {
        !self.host.trim().is_empty()
            && !self.username.trim().is_empty()
            && !self.password.trim().is_empty()
    }

    /// True when a host is configured but the credentials are not — a state worth saying out
    /// loud, because it is almost always an unfinished .env rather than a deliberate choice.
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

    pub fn config(&self) -> &MailConfig {
        &self.config
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
            // Deliberately loud and complete: in dev the code IS the delivery mechanism,
            // and a truncated log line would make the flow untestable.
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
                // Outside production the body is echoed to the log even on success, because
                // a developer testing the signup flow has no way to read the mailbox the
                // mail went to. In production nothing is logged: the code is a secret and
                // only the recipient should see it.
                if std::env::var("RUST_ENV").unwrap_or_default() != "production" {
                    tracing::info!(to = %to, "mail sent (dev: body logged below)");
                    for line in body.lines() {
                        tracing::info!("mail-dev> {}", line);
                    }
                }
                Ok(())
            }
            Err(err) => {
                // Outside production, the code still has to be findable — otherwise a wrong
                // password means nobody can sign up and nothing says why. The body goes to
                // the log in the same shape as console mode, clearly marked as a fallback.
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

/// The signup code mail. Plain text on purpose — an OTP that needs an HTML renderer is one
/// more thing that can arrive broken.
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

/// The password-reset mail. Deliberately says a reset was requested and nothing about the
/// account: this mail can land in a shared inbox, and "someone tried to reset your Jobify
/// password" is a warning, not a disclosure.
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
            password: "secret".into(),
            security: SmtpSecurity::StartTls,
            app_base_url: "http://localhost:5173".into(),
        }
    }

    #[test]
    fn security_strings_map_to_the_right_mode() {
        assert_eq!(SmtpSecurity::parse("starttls"), SmtpSecurity::StartTls);
        assert_eq!(SmtpSecurity::parse("TLS"), SmtpSecurity::ImplicitTls);
        assert_eq!(SmtpSecurity::parse("ssl"), SmtpSecurity::ImplicitTls);
        assert_eq!(SmtpSecurity::parse("none"), SmtpSecurity::Plain);
        // Anything unrecognised must land on the encrypted default, never on plaintext.
        assert_eq!(SmtpSecurity::parse("banana"), SmtpSecurity::StartTls);
    }

    #[test]
    fn no_host_means_console_mode() {
        assert!(!Mailer::new(config("")).is_live());
        assert!(Mailer::new(config("smtp.gmail.com")).is_live());
    }

    #[test]
    fn host_without_credentials_stays_in_console_mode() {
        // The exact half-filled state a .env passes through: host stored, app password not
        // yet. Staying in console mode is what keeps signup working while that is true.
        let mut no_password = config("smtp.gmail.com");
        no_password.password = String::new();
        let mailer = Mailer::new(no_password.clone());
        assert!(!mailer.is_live(), "a blank password must not enable SMTP");
        assert!(mailer.config().awaiting_credentials());

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            assert!(mailer.send("a@b.com", "s", "code 123456").await.is_ok());
        });

        let mut no_username = config("smtp.gmail.com");
        no_username.username = String::new();
        assert!(!Mailer::new(no_username).is_live());
    }

    #[test]
    fn awaiting_credentials_is_false_for_both_finished_states() {
        assert!(!Mailer::new(config("")).config().awaiting_credentials());
        assert!(
            !Mailer::new(config("smtp.gmail.com"))
                .config()
                .awaiting_credentials()
        );
    }

    #[test]
    fn console_mode_reports_success_rather_than_failing_the_signup() {
        let mailer = Mailer::new(config(""));
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            assert!(
                mailer
                    .send("a@b.com", "s", "line one\nline two")
                    .await
                    .is_ok()
            );
        });
    }

    #[test]
    fn verification_mail_carries_the_code_and_the_ttl() {
        let (subject, body) = verification_email("123456", "Halima");
        assert!(
            subject.contains("123456"),
            "the code must be in the subject: {subject}"
        );
        assert!(body.contains("123456"));
        assert!(body.contains("Halima"));
        assert!(
            body.contains("10 minutes"),
            "the TTL must be stated: {body}"
        );
    }

    #[test]
    fn reset_mail_does_not_disclose_the_account_and_states_nothing_changed() {
        let (subject, body) = password_reset_email("654321", "");
        assert!(subject.contains("654321"));
        assert!(body.contains("654321"));
        assert!(
            body.contains("30 minutes"),
            "the reset TTL must be stated: {body}"
        );
        assert!(
            body.contains("nothing has changed yet"),
            "the user must know it is not live yet: {body}"
        );
        assert!(
            !body.contains("https"),
            "no links: a link cannot be attempt-limited like a code"
        );
    }

    #[test]
    fn verification_mail_greets_an_unnamed_user_without_an_empty_hole() {
        let (_, body) = verification_email("000000", "   ");
        assert!(body.starts_with("Hi,"), "no dangling comma: {body}");
    }
}
