//! The bodies of the transactional messages (spec "Pluggable Email
//! Delivery").
//!
//! Pure functions: no clock, no network, no configuration. The link is
//! passed in, so the same template serves the development link shape and the
//! production one.

use crate::application::ports::EmailMessage;

use super::{RESET_TOKEN_TTL_SECS, VERIFICATION_TOKEN_TTL_SECS};

/// The message that asks a player to confirm the address they registered.
pub fn verification_email(to: &str, username: &str, link: &str) -> EmailMessage {
    let hours = VERIFICATION_TOKEN_TTL_SECS / 3600;
    let text_body = format!(
        "Hello {username},\n\n\
         Confirm your email address to finish setting up your PlainTextChess account:\n\n\
         {link}\n\n\
         The link expires in {hours} hours.\n\n\
         If you did not create this account, you can ignore this message.\n"
    );
    let html_body = format!(
        "<p>Hello {username},</p>\
         <p>Confirm your email address to finish setting up your PlainTextChess account:</p>\
         <p><a href=\"{link}\">Confirm my email address</a></p>\
         <p>The link expires in {hours} hours.</p>\
         <p>If you did not create this account, you can ignore this message.</p>\n"
    );
    EmailMessage {
        to: to.to_string(),
        subject: "Confirm your email address".to_string(),
        text_body,
        html_body,
    }
}

/// The message that carries a password-reset link.
pub fn reset_email(to: &str, username: &str, link: &str) -> EmailMessage {
    let minutes = RESET_TOKEN_TTL_SECS / 60;
    let text_body = format!(
        "Hello {username},\n\n\
         Someone asked to reset the password of your PlainTextChess account.\n\
         If it was you, open this link to choose a new password:\n\n\
         {link}\n\n\
         The link expires in {minutes} minutes and works only once.\n\n\
         If it was not you, you can ignore this message; your password has not changed.\n"
    );
    let html_body = format!(
        "<p>Hello {username},</p>\
         <p>Someone asked to reset the password of your PlainTextChess account. \
         If it was you, open this link to choose a new password:</p>\
         <p><a href=\"{link}\">Choose a new password</a></p>\
         <p>The link expires in {minutes} minutes and works only once.</p>\
         <p>If it was not you, you can ignore this message; your password has not changed.</p>\n"
    );
    EmailMessage {
        to: to.to_string(),
        subject: "Reset your password".to_string(),
        text_body,
        html_body,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_verification_message_carries_the_link() {
        let message = verification_email("ana@example.com", "ana", "plaintextchess://app/verify-email?token=t1");
        assert_eq!(message.to, "ana@example.com");
        assert_eq!(message.subject, "Confirm your email address");
        assert!(message.text_body.contains("plaintextchess://app/verify-email?token=t1"));
        assert!(message.html_body.contains("plaintextchess://app/verify-email?token=t1"));
        assert!(message.text_body.contains("ana"));
    }

    #[test]
    fn the_reset_message_carries_the_link() {
        let message = reset_email("ana@example.com", "ana", "https://play.example.com/reset-password?token=t2");
        assert_eq!(message.subject, "Reset your password");
        assert!(message.text_body.contains("https://play.example.com/reset-password?token=t2"));
        assert!(message.html_body.contains("https://play.example.com/reset-password?token=t2"));
    }

    #[test]
    fn the_reset_message_does_not_claim_the_password_changed() {
        let message = reset_email("ana@example.com", "ana", "https://x/y?token=t");
        assert!(message.text_body.contains("your password has not changed"));
    }

    #[test]
    fn the_two_messages_have_different_subjects() {
        let verification = verification_email("a@b.c", "ana", "l");
        let reset = reset_email("a@b.c", "ana", "l");
        assert_ne!(verification.subject, reset.subject);
    }
}
