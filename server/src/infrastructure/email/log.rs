//! The development transport (spec "Pluggable Email Delivery").
//!
//! It writes the whole message to the server log, including the link it
//! carries. That is what makes the reset and verification flows complete and
//! testable before a mail provider or a domain exists — and it is why the
//! transport is refused in production mode (design D19): the link holds a
//! token, and a token in a production log is a credential in a log.

use std::future::Future;
use std::pin::Pin;

use crate::application::ports::{EmailError, EmailMessage, EmailSender};

/// Writes every message to the server log.
pub struct LogEmailSender;

impl EmailSender for LogEmailSender {
    fn send<'a>(
        &'a self,
        message: &'a EmailMessage,
    ) -> Pin<Box<dyn Future<Output = Result<(), EmailError>> + Send + 'a>> {
        Box::pin(async move {
            // The recipient and the body are the message itself, which is
            // the one documented exception to "no secret in a log line".
            tracing::info!(
                transport = "log",
                to = %message.to,
                subject = %message.subject,
                "email delivered to the log transport"
            );
            tracing::info!(body = %message.text_body, "email body (development log transport)");
            Ok(())
        })
    }
}

/// Drops every message. Used by the tests that do not exercise mail.
#[derive(Debug, Default)]
pub struct NullEmailSender;

impl EmailSender for NullEmailSender {
    fn send<'a>(
        &'a self,
        _message: &'a EmailMessage,
    ) -> Pin<Box<dyn Future<Output = Result<(), EmailError>> + Send + 'a>> {
        Box::pin(async move { Ok(()) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::email::templates::reset_email;
    use std::io::Write;
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::fmt::MakeWriter;

    /// A writer that keeps what was written, so a test can read the log.
    #[derive(Clone, Default)]
    struct Buffer(Arc<Mutex<Vec<u8>>>);

    impl Write for Buffer {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().expect("the buffer is not poisoned").extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for Buffer {
        type Writer = Buffer;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    #[tokio::test]
    async fn the_log_transport_writes_the_message_to_the_log() {
        let buffer = Buffer::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(buffer.clone())
            .with_ansi(false)
            .finish();
        let guard = tracing::subscriber::set_default(subscriber);

        let message = reset_email("ana@example.com", "ana", "https://x/reset-password?token=t");
        let outcome = LogEmailSender.send(&message).await;
        assert!(outcome.is_ok(), "the log transport always delivers");

        drop(guard);
        let written = String::from_utf8(buffer.0.lock().expect("the buffer is not poisoned").clone())
            .expect("the log is text");
        assert!(written.contains("ana@example.com"), "the recipient is written: {written}");
        assert!(
            written.contains("https://x/reset-password?token=t"),
            "the body is written: {written}"
        );
    }

    #[tokio::test]
    async fn the_null_transport_delivers_nothing_and_does_not_fail() {
        let message = reset_email("ana@example.com", "ana", "https://x/reset-password?token=t");
        assert!(NullEmailSender.send(&message).await.is_ok());
    }
}
