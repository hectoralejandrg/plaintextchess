//! Transactional email (spec "Pluggable Email Delivery").
//!
//! The account, verification, and reset flows never learn which transport
//! they are talking to: they build a message and hand it to the
//! [`EmailSender`] port. The transports live here:
//!
//! - [`log::LogEmailSender`] writes the message to the server log, which is
//!   what makes the flows complete and testable before a mail provider or a
//!   domain exists. It is refused in production mode (design D19).
//! - [`resend::ResendEmailSender`] sends the message to a provider.
//!
//! The bodies ([`templates`]) and the links ([`links`]) are pure functions:
//! no clock, no network, no configuration.

pub mod domain;
pub mod links;
pub mod log;
pub mod resend;
pub mod templates;

use std::sync::Arc;

use crate::infrastructure::config::Config;

pub use links::{reset_link, verification_link};
pub use log::LogEmailSender;
pub use resend::ResendEmailSender;
pub use templates::{reset_email, verification_email};

/// How long a password-reset token stays usable. Short on purpose: the
/// token is a one-shot key to an account (spec "Password Reset by Email").
pub const RESET_TOKEN_TTL_SECS: i64 = 15 * 60;

/// How long an email-verification token stays usable. Longer than a reset
/// token, because a verification can wait for the player to open their mail.
pub const VERIFICATION_TOKEN_TTL_SECS: i64 = 24 * 60 * 60;

/// Which transport the deployment delivers mail with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmailTransport {
    /// Write the message to the server log. Development only.
    Log,
    /// Send the message to the provider API.
    Resend,
    /// Deliver nothing. Used by tests that do not exercise mail.
    None,
}

impl EmailTransport {
    /// Read the transport named by the configuration. `None` means the
    /// configuration holds a value this build does not know.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_lowercase().as_str() {
            "log" => Some(EmailTransport::Log),
            "resend" => Some(EmailTransport::Resend),
            "none" => Some(EmailTransport::None),
            _ => None,
        }
    }
}

/// Build the transport the configuration selects.
///
/// The log transport is refused in production mode: it writes the whole
/// message, including the token, so a production deployment must not be able
/// to select it by accident (design D19). A provider transport with no
/// readable credential is *not* an error here: the server keeps running and
/// the transport reports `NotConfigured` when a message is handed to it, so
/// the flows that do not need mail still work (spec "Server Health and
/// Configuration").
pub fn from_config(config: &Config) -> Result<Arc<dyn crate::application::ports::EmailSender>, String> {
    let transport = EmailTransport::parse(&config.email_transport)
        .ok_or_else(|| format!("EMAIL_TRANSPORT={} is invalid; must be log, resend, or none", config.email_transport))?;

    match transport {
        EmailTransport::Log => {
            if config.production_mode {
                return Err(
                    "EMAIL_TRANSPORT=log cannot run in production mode; set it to a provider transport"
                        .to_string(),
                );
            }
            Ok(Arc::new(LogEmailSender))
        }
        EmailTransport::Resend => {
            let credential = config.email_api_key();
            Ok(Arc::new(ResendEmailSender::new(
                credential,
                config.email_from.clone(),
                config.email_from_name.clone(),
            )))
        }
        EmailTransport::None => Ok(Arc::new(log::NullEmailSender)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::config::Config;

    fn config() -> Config {
        Config::for_test("127.0.0.1".parse().expect("address"), 0, std::time::Duration::from_secs(1))
    }

    #[test]
    fn the_transport_is_read_from_the_configuration() {
        for (name, expected) in [
            ("log", EmailTransport::Log),
            ("resend", EmailTransport::Resend),
            ("none", EmailTransport::None),
        ] {
            assert_eq!(EmailTransport::parse(name), Some(expected));
        }
        assert_eq!(EmailTransport::parse("LOG"), Some(EmailTransport::Log));
        assert_eq!(EmailTransport::parse("pigeon"), None);
    }

    #[tokio::test]
    async fn the_default_configuration_selects_the_log_transport() {
        let config = config();
        let sender = from_config(&config).expect("the log transport is allowed in development");
        // The log transport needs no credential and always delivers, which is
        // what tells it apart from an unconfigured provider.
        let message = crate::application::ports::EmailMessage {
            to: "ana@example.com".to_string(),
            subject: "Confirm your email address".to_string(),
            text_body: "body".to_string(),
            html_body: "<p>body</p>".to_string(),
        };
        assert!(sender.send(&message).await.is_ok());
    }

    #[test]
    fn the_log_transport_is_refused_in_production_mode() {
        let mut config = config();
        config.production_mode = true;
        match from_config(&config) {
            Ok(_) => panic!("production must not use the log transport"),
            Err(err) => {
                assert!(
                    err.contains("production") && err.contains("EMAIL_TRANSPORT"),
                    "the error must name the problem: {err}"
                );
            }
        }
    }

    #[test]
    fn a_provider_transport_without_a_credential_still_builds() {
        let mut config = config();
        config.email_transport = "resend".to_string();
        config.email_from = "no-reply@play.example.com".to_string();
        let sender = from_config(&config).expect("an unconfigured provider is not a start-up error");
        drop(sender);
    }
}
