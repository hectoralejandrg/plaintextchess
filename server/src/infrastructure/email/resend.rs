//! The provider transport (spec "Pluggable Email Delivery").
//!
//! Resend's API takes a JSON body and a bearer credential. The credential
//! lives in a file, never in the container's environment (design D5), and the
//! provider rejects a request that carries no `User-Agent` (it answers 403
//! with error 1010), so the header is always sent.
//!
//! The call is blocking, so it runs in a blocking task: the server's
//! event loop never waits for the provider.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use serde_json::json;

use crate::application::ports::{EmailError, EmailMessage, EmailSender};

/// The provider's send endpoint.
const ENDPOINT: &str = "https://api.resend.com/emails";

/// The provider requires a `User-Agent` on every request.
const USER_AGENT: &str = concat!("plaintextchess-server/", env!("CARGO_PKG_VERSION"));

/// How long a send may take before it is given up on.
const TIMEOUT: Duration = Duration::from_secs(15);

/// Sends messages to the provider.
pub struct ResendEmailSender {
    /// The provider's credential. `None` when the transport is selected but
    /// no credential could be read: the server keeps running and the
    /// transport reports `NotConfigured` instead.
    api_key: Option<String>,
    from: String,
    from_name: Option<String>,
}

impl ResendEmailSender {
    /// Build the transport. The credential is optional on purpose: a
    /// deployment that has not configured a provider yet still starts.
    pub fn new(
        api_key: Option<String>,
        from: String,
        from_name: Option<String>,
    ) -> Self {
        Self {
            api_key,
            from,
            from_name,
        }
    }

    /// The JSON body the provider expects. Pure, so a test can assert it
    /// without a network.
    pub fn request_body(&self, message: &EmailMessage) -> serde_json::Value {
        let from = match self.from_name.as_deref() {
            Some(name) if !name.trim().is_empty() => format!("{name} <{}>", self.from),
            _ => self.from.clone(),
        };
        json!({
            "from": from,
            "to": [message.to],
            "subject": message.subject,
            "text": message.text_body,
            "html": message.html_body,
        })
    }

    /// The credential, when there is one.
    pub fn is_configured(&self) -> bool {
        self.api_key.is_some()
    }
}

impl EmailSender for ResendEmailSender {
    fn send<'a>(
        &'a self,
        message: &'a EmailMessage,
    ) -> Pin<Box<dyn Future<Output = Result<(), EmailError>> + Send + 'a>> {
        Box::pin(async move {
            let Some(api_key) = self.api_key.clone() else {
                return Err(EmailError::NotConfigured);
            };
            let body = self.request_body(message);
            let agent = ureq::AgentBuilder::new()
                .timeout(TIMEOUT)
                .build();
            tokio::task::spawn_blocking(move || send_blocking(&agent, &api_key, body))
                .await
                .map_err(|err| EmailError::Transport(format!("the mail task failed: {err}")))?
        })
    }
}

/// The blocking part of a send: one POST, one answer.
fn send_blocking(
    agent: &ureq::Agent,
    api_key: &str,
    body: serde_json::Value,
) -> Result<(), EmailError> {
    let outcome = agent
        .post(ENDPOINT)
        .set("Authorization", &format!("Bearer {api_key}"))
        .set("User-Agent", USER_AGENT)
        .set("Content-Type", "application/json")
        .send_json(&body);
    match outcome {
        Ok(_) => Ok(()),
        Err(ureq::Error::Status(code, _)) => {
            Err(EmailError::Transport(format!("the provider answered {code}")))
        }
        Err(err) => Err(EmailError::Transport(err.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::email::templates::reset_email;

    fn sender() -> ResendEmailSender {
        ResendEmailSender::new(
            Some("re_test".to_string()),
            "no-reply@play.example.com".to_string(),
            Some("PlainTextChess".to_string()),
        )
    }

    #[test]
    fn the_request_body_carries_the_message() {
        let message = reset_email("ana@example.com", "ana", "https://x/reset-password?token=t");
        let body = sender().request_body(&message);
        assert_eq!(body["from"], "PlainTextChess <no-reply@play.example.com>");
        assert_eq!(body["to"][0], "ana@example.com");
        assert_eq!(body["subject"], "Reset your password");
        assert!(body["text"].as_str().unwrap().contains("token=t"));
        assert!(body["html"].as_str().unwrap().contains("token=t"));
    }

    #[test]
    fn a_sender_without_a_name_sends_the_bare_address() {
        let sender = ResendEmailSender::new(
            Some("re_test".to_string()),
            "no-reply@play.example.com".to_string(),
            None,
        );
        let message = reset_email("ana@example.com", "ana", "https://x/reset-password?token=t");
        assert_eq!(
            sender.request_body(&message)["from"],
            "no-reply@play.example.com"
        );
    }

    #[tokio::test]
    async fn a_transport_without_a_credential_reports_not_configured() {
        let sender = ResendEmailSender::new(
            None,
            "no-reply@play.example.com".to_string(),
            None,
        );
        assert!(!sender.is_configured());
        let message = reset_email("ana@example.com", "ana", "https://x/reset-password?token=t");
        assert_eq!(
            sender.send(&message).await.expect_err("no credential"),
            EmailError::NotConfigured
        );
    }
}
