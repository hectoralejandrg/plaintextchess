//! The links a message carries (design D6).
//!
//! The base URL is the deployment's public address, so the same server build
//! emits a `plaintextchess://app/reset-password?token=…` link in development
//! and an `https://play.example.com/reset-password?token=…` link once a
//! domain exists. The token inside the link is the same in both, so a link
//! already delivered keeps working after the switch.

/// The path a password-reset link points at.
pub const RESET_PATH: &str = "/reset-password";

/// The path an email-verification link points at.
pub const VERIFICATION_PATH: &str = "/verify-email";

/// The link a password-reset message carries.
pub fn reset_link(base_url: &str, token: &str) -> String {
    build(base_url, RESET_PATH, token)
}

/// The link an email-verification message carries.
pub fn verification_link(base_url: &str, token: &str) -> String {
    build(base_url, VERIFICATION_PATH, token)
}

/// Join the base address and the path. The base is trimmed of a trailing
/// slash so a configured `https://host/` and `https://host` produce the same
/// link, and the token is appended as a query parameter.
fn build(base_url: &str, path: &str, token: &str) -> String {
    let base = base_url.trim_end_matches('/');
    format!("{base}{path}?token={token}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reset_link_carries_the_token_once() {
        let link = reset_link("https://play.example.com", "abc123");
        assert_eq!(link, "https://play.example.com/reset-password?token=abc123");
        assert_eq!(link.matches("abc123").count(), 1);
    }

    #[test]
    fn a_verification_link_carries_the_token_once() {
        let link = verification_link("https://play.example.com", "xyz789");
        assert_eq!(link, "https://play.example.com/verify-email?token=xyz789");
        assert_eq!(link.matches("xyz789").count(), 1);
    }

    #[test]
    fn a_custom_scheme_link_opens_the_app() {
        let link = reset_link("plaintextchess://app", "abc123");
        assert_eq!(link, "plaintextchess://app/reset-password?token=abc123");
    }

    #[test]
    fn a_trailing_slash_in_the_base_is_ignored() {
        assert_eq!(
            reset_link("https://play.example.com/", "abc123"),
            "https://play.example.com/reset-password?token=abc123"
        );
    }

    #[test]
    fn the_two_paths_differ() {
        assert_ne!(RESET_PATH, VERIFICATION_PATH);
    }
}
