//! Player accounts, device links, sessions, and the identity a connection
//! resolves to (spec "player-auth"). Pure records and pure validation: this
//! module imports no `tokio`, `axum`, `serde`, or `chess_core`, so the rules
//! are unit-testable in isolation.
//!
//! Timestamp convention (design D7): `*_at_ms` fields are wall-clock unix
//! milliseconds, like `ended_at_ms` in the game recorder — *not* the monotonic
//! `TimeSource`, whose marks are only meaningful as differences. A session
//! expiry has to survive a restart and be comparable against a stored column,
//! so it needs an absolute wall-clock stamp.

/// The shortest accepted username (spec "Player Account Registration").
pub const MIN_USERNAME_LEN: usize = 3;
/// The longest accepted username.
pub const MAX_USERNAME_LEN: usize = 24;
/// The shortest accepted password.
pub const MIN_PASSWORD_LEN: usize = 8;
/// The longest accepted password. Bounding the length *before* hashing is
/// what keeps an oversized input from costing the server unbounded CPU
/// (spec "Credential and Session Data Protection").
pub const MAX_PASSWORD_LEN: usize = 128;
/// The longest accepted display name.
pub const MAX_DISPLAY_NAME_LEN: usize = 32;

/// A registered player account. The identity is the server-generated
/// `account_id`, never anything the client chose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub account_id: String,
    /// The username as the player typed it, for display.
    pub username: String,
    /// The case-folded username: the lookup key that makes uniqueness
    /// case-insensitive (design D3).
    pub username_key: String,
    /// An argon2id PHC string. The plaintext password is never stored.
    pub password_hash: String,
    /// `None` means "report the username as the display name" (spec
    /// "Profile Display Name").
    pub display_name: Option<String>,
    pub created_at_ms: i64,
}

impl Account {
    /// The name to report for this account: its display name when set,
    /// otherwise its username.
    pub fn reported_name(&self) -> &str {
        self.display_name.as_deref().unwrap_or(&self.username)
    }
}

/// A device bound to an account. One device holds at most one of these
/// (design D2: `device_id` is the primary key).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub device_id: String,
    pub account_id: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// An issued bearer session. Only `token_hash` is persisted; the plaintext
/// token exists exactly once, in the response to the login or registration
/// that issued it (design D6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub session_id: String,
    pub account_id: String,
    /// The device that logged in.
    pub device_id: String,
    pub token_hash: String,
    pub created_at_ms: i64,
    pub expires_at_ms: i64,
    /// `Some` once the session has been revoked (design D7).
    pub revoked_at_ms: Option<i64>,
}

impl Session {
    /// Whether the session has expired at `now_ms` (design D7: evaluated at
    /// lookup time, so there is no sweeper task to disagree with the
    /// database).
    pub fn is_expired(&self, now_ms: i64) -> bool {
        now_ms >= self.expires_at_ms
    }

    pub fn is_revoked(&self) -> bool {
        self.revoked_at_ms.is_some()
    }

    /// Whether the session can authenticate right now.
    pub fn is_usable(&self, now_ms: i64) -> bool {
        !self.is_expired(now_ms) && !self.is_revoked()
    }
}

/// Who a connection is (spec "player-auth", design D8). A guest is identified
/// by the device identifier the client has always asserted; an authenticated
/// connection additionally carries the account its session resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Identity {
    Guest {
        device_id: String,
    },
    Account {
        account_id: String,
        device_id: String,
    },
}

impl Identity {
    /// The key this identity is tracked and matched by: the account when
    /// authenticated, the device otherwise. Room occupancy and seat matching
    /// both key off this (design D8/D9).
    pub fn key(&self) -> &str {
        match self {
            Identity::Guest { device_id } => device_id,
            Identity::Account { account_id, .. } => account_id,
        }
    }

    /// The client-asserted device identifier. Ratings and the persisted game
    /// history stay keyed by this even when the player is authenticated
    /// (design D1).
    pub fn device_id(&self) -> &str {
        match self {
            Identity::Guest { device_id } => device_id,
            Identity::Account { device_id, .. } => device_id,
        }
    }

    /// The account this identity authenticated as, if any.
    pub fn account_id(&self) -> Option<&str> {
        match self {
            Identity::Guest { .. } => None,
            Identity::Account { account_id, .. } => Some(account_id),
        }
    }

    /// The same device, now authenticated as `account_id`. Used when a
    /// connection logs in mid-session (design D8: a `login` can arrive at any
    /// point and is adopted by the next room-affecting action).
    pub fn authenticated_as(&self, account_id: &str) -> Identity {
        Identity::Account {
            account_id: account_id.to_string(),
            device_id: self.device_id().to_string(),
        }
    }
}

/// A bare device identifier means "guest".
///
/// This is what keeps the pre-authentication call sites — and every client
/// that never authenticates — compiling and behaving unchanged: a device id
/// is exactly what the protocol has always carried, and it resolves to
/// precisely the identity it did before accounts existed.
impl From<&str> for Identity {
    fn from(device_id: &str) -> Self {
        Identity::Guest {
            device_id: device_id.to_string(),
        }
    }
}

impl From<String> for Identity {
    fn from(device_id: String) -> Self {
        Identity::Guest { device_id }
    }
}

/// A rejected credential field, so the error can name what the player got
/// wrong (spec: "a validation error naming the offending field").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldError {
    Username,
    Password,
    DisplayName,
}

impl FieldError {
    pub fn as_str(&self) -> &'static str {
        match self {
            FieldError::Username => "username",
            FieldError::Password => "password",
            FieldError::DisplayName => "display_name",
        }
    }
}

/// Why a username was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsernameError {
    /// Outside `[MIN_USERNAME_LEN, MAX_USERNAME_LEN]` after trimming.
    Length,
    /// Contains something other than a letter, digit, `_`, or `-`.
    Charset,
}

/// Validate a username (spec "Player Account Registration"): 3 to 24
/// characters of letters, digits, underscore, or hyphen, and not only
/// whitespace. The value is trimmed before it is measured, so a padded
/// username is accepted and stored trimmed.
pub fn validate_username(raw: &str) -> Result<String, UsernameError> {
    let trimmed = raw.trim();
    let length = trimmed.chars().count();
    if !(MIN_USERNAME_LEN..=MAX_USERNAME_LEN).contains(&length) {
        return Err(UsernameError::Length);
    }
    if trimmed.is_empty() {
        return Err(UsernameError::Length);
    }
    let allowed = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
    if !trimmed.chars().all(allowed) {
        return Err(UsernameError::Charset);
    }
    Ok(trimmed.to_string())
}

/// The case-insensitive lookup key for a username (design D3). SQLite's
/// `NOCASE` collation folds ASCII only, so the fold is done here and stored
/// explicitly rather than left to the index. The Unicode mapping is used so
/// `Änne` and `änne` collide too; the key is a lookup key only, and
/// validation always runs against the username as the player typed it.
pub fn username_key(username: &str) -> String {
    username.to_lowercase()
}

/// Why a password was rejected. The length is measured in characters, and
/// bounded before any hashing happens (spec "Credential and Session Data
/// Protection").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasswordError {
    /// Outside `[MIN_PASSWORD_LEN, MAX_PASSWORD_LEN]`.
    Length,
}

/// Validate a password length. The value itself is never inspected for
/// content: a passphrase's spaces and punctuation are the point.
pub fn validate_password(password: &str) -> Result<(), PasswordError> {
    let length = password.chars().count();
    if (MIN_PASSWORD_LEN..=MAX_PASSWORD_LEN).contains(&length) {
        Ok(())
    } else {
        Err(PasswordError::Length)
    }
}

/// Why a display name was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayNameError {
    /// Empty once surrounding whitespace is trimmed, or longer than
    /// `MAX_DISPLAY_NAME_LEN`.
    Length,
    /// Contains a control character.
    Control,
}

/// Validate a display name (spec "Profile Display Name"): trim it, then
/// require 1 to 32 characters and no control characters. The trimmed value
/// is what gets stored.
pub fn validate_display_name(raw: &str) -> Result<String, DisplayNameError> {
    let trimmed = raw.trim();
    let length = trimmed.chars().count();
    if !(1..=MAX_DISPLAY_NAME_LEN).contains(&length) {
        return Err(DisplayNameError::Length);
    }
    if trimmed.chars().any(|c| c.is_control()) {
        return Err(DisplayNameError::Control);
    }
    Ok(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------
    // 2.1 Identity
    // ------------------------------------------------------------------

    #[test]
    fn a_guest_is_keyed_by_its_device() {
        let guest = Identity::Guest {
            device_id: "device-a".into(),
        };
        assert_eq!(guest.key(), "device-a");
        assert_eq!(guest.device_id(), "device-a");
        assert_eq!(guest.account_id(), None);
    }

    #[test]
    fn an_authenticated_identity_is_keyed_by_its_account_but_rated_by_its_device() {
        let identity = Identity::Account {
            account_id: "account-1".into(),
            device_id: "device-a".into(),
        };
        assert_eq!(identity.key(), "account-1", "occupancy and seats key off the account");
        assert_eq!(
            identity.device_id(),
            "device-a",
            "ratings stay keyed by the device (design D1)"
        );
        assert_eq!(identity.account_id(), Some("account-1"));
    }

    #[test]
    fn authenticating_keeps_the_same_device() {
        let guest = Identity::Guest {
            device_id: "device-a".into(),
        };
        let upgraded = guest.authenticated_as("account-1");
        assert_eq!(
            upgraded,
            Identity::Account {
                account_id: "account-1".into(),
                device_id: "device-a".into(),
            }
        );
        // Authenticating a guest yields exactly the guest's device, so the
        // seat it already occupies is the one that gets upgraded.
        assert_eq!(upgraded.device_id(), guest.device_id());
    }

    // ------------------------------------------------------------------
    // 2.2 Usernames and passwords
    // ------------------------------------------------------------------

    #[test]
    fn a_well_formed_username_is_accepted_and_trimmed() {
        assert_eq!(validate_username("Ana_99-x").unwrap(), "Ana_99-x");
        assert_eq!(validate_username("  Ana  ").unwrap(), "Ana", "padding is trimmed");
    }

    #[test]
    fn username_length_boundaries_are_enforced() {
        assert_eq!(validate_username("ab"), Err(UsernameError::Length), "2 is too short");
        assert_eq!(validate_username("abc").unwrap(), "abc", "3 is the minimum");
        let longest = "a".repeat(MAX_USERNAME_LEN);
        assert_eq!(validate_username(&longest).unwrap(), longest, "24 is accepted");
        let too_long = "a".repeat(MAX_USERNAME_LEN + 1);
        assert_eq!(validate_username(&too_long), Err(UsernameError::Length));
    }

    #[test]
    fn a_whitespace_only_username_is_rejected() {
        assert_eq!(validate_username("   "), Err(UsernameError::Length));
        assert_eq!(validate_username("\t\t"), Err(UsernameError::Length));
    }

    #[test]
    fn a_username_with_unsupported_characters_is_rejected() {
        for bad in ["Ana maria", "ana@host", "ana.mx", "añe!", "ana+1", "an*a"] {
            assert_eq!(
                validate_username(bad),
                Err(UsernameError::Charset),
                "`{bad}` must be rejected"
            );
        }
    }

    #[test]
    fn the_username_fold_makes_case_variants_collide() {
        assert_eq!(username_key("Ana"), username_key("ana"));
        assert_eq!(username_key("ANA"), username_key("ana"));
        assert_eq!(username_key("aNa-99_X"), username_key("ana-99_x"));
        // Non-ASCII uppercase folds as well, which SQLite's NOCASE index
        // would not do (design D3).
        assert_eq!(username_key("Änne"), username_key("änne"));
        // Distinct usernames stay distinct.
        assert_ne!(username_key("ana"), username_key("ann"));
        // Digits and marks are untouched by folding.
        assert_eq!(username_key("A1-_9"), "a1-_9");
    }

    #[test]
    fn password_length_boundaries_are_enforced() {
        let shortest = "a".repeat(MIN_PASSWORD_LEN);
        assert_eq!(validate_password(&shortest), Ok(()), "8 is the minimum");
        assert_eq!(validate_password("short"), Err(PasswordError::Length));
        let longest = "a".repeat(MAX_PASSWORD_LEN);
        assert_eq!(validate_password(&longest), Ok(()), "128 is accepted");
        let too_long = "a".repeat(MAX_PASSWORD_LEN + 1);
        assert_eq!(
            validate_password(&too_long),
            Err(PasswordError::Length),
            "an oversized password is refused before it reaches the hasher"
        );
    }

    #[test]
    fn a_passwords_content_is_never_inspected() {
        // Spaces and punctuation are the point of a passphrase.
        assert_eq!(validate_password("  a b c d  "), Ok(()));
        assert_eq!(validate_password("¡correct horse!"), Ok(()));
    }

    // ------------------------------------------------------------------
    // 2.3 Display names
    // ------------------------------------------------------------------

    #[test]
    fn a_well_formed_display_name_is_trimmed_and_accepted() {
        assert_eq!(validate_display_name("  Ana Torres ").unwrap(), "Ana Torres");
        assert_eq!(validate_display_name("x").unwrap(), "x");
        let longest = "n".repeat(MAX_DISPLAY_NAME_LEN);
        assert_eq!(validate_display_name(&longest).unwrap(), longest, "32 is accepted");
    }

    #[test]
    fn a_display_name_empty_after_trimming_is_rejected() {
        assert_eq!(validate_display_name(""), Err(DisplayNameError::Length));
        assert_eq!(validate_display_name("   "), Err(DisplayNameError::Length));
        assert_eq!(validate_display_name("\t\n"), Err(DisplayNameError::Length));
    }

    #[test]
    fn an_overlong_display_name_is_rejected() {
        let too_long = "n".repeat(MAX_DISPLAY_NAME_LEN + 1);
        assert_eq!(validate_display_name(&too_long), Err(DisplayNameError::Length));
    }

    #[test]
    fn a_display_name_with_a_control_character_is_rejected() {
        for bad in ["Ana\nTorres", "Ana\tTorres", "Ana\u{7}Torres", "Ana\u{0}"] {
            assert_eq!(
                validate_display_name(bad),
                Err(DisplayNameError::Control),
                "`{bad:?}` must be rejected"
            );
        }
    }

    // ------------------------------------------------------------------
    // Records
    // ------------------------------------------------------------------

    #[test]
    fn an_account_without_a_display_name_reports_its_username() {
        let mut account = Account {
            account_id: "account-1".into(),
            username: "Ana".into(),
            username_key: "ana".into(),
            password_hash: "$argon2id$...".into(),
            display_name: None,
            created_at_ms: 1,
        };
        assert_eq!(account.reported_name(), "Ana");
        account.display_name = Some("Ana T.".into());
        assert_eq!(account.reported_name(), "Ana T.");
    }

    #[test]
    fn session_usability_combines_expiry_and_revocation() {
        let session = Session {
            session_id: "session-1".into(),
            account_id: "account-1".into(),
            device_id: "device-a".into(),
            token_hash: "hash".into(),
            created_at_ms: 1000,
            expires_at_ms: 2000,
            revoked_at_ms: None,
        };
        assert!(session.is_usable(1500), "before the expiry it authenticates");
        assert!(!session.is_expired(1999));
        assert!(session.is_expired(2000), "expiry is inclusive of the mark");
        assert!(!session.is_usable(2000), "an expired session is unusable");
        assert!(!session.is_usable(9999));
    }

    #[test]
    fn a_revoked_session_is_never_usable_even_before_it_expires() {
        let mut session = Session {
            session_id: "session-1".into(),
            account_id: "account-1".into(),
            device_id: "device-a".into(),
            token_hash: "hash".into(),
            created_at_ms: 1000,
            expires_at_ms: 9_999_999,
            revoked_at_ms: None,
        };
        assert!(!session.is_revoked());
        assert!(session.is_usable(1200), "a live session authenticates");

        // Revoked at 1200, well before the expiry.
        session.revoked_at_ms = Some(1200);
        assert!(session.is_revoked());
        assert!(
            !session.is_usable(1200),
            "revocation takes effect immediately"
        );
        assert!(
            !session.is_usable(1500),
            "revocation wins over a live expiry"
        );
    }

    #[test]
    fn field_errors_name_the_offending_field() {
        assert_eq!(FieldError::Username.as_str(), "username");
        assert_eq!(FieldError::Password.as_str(), "password");
        assert_eq!(FieldError::DisplayName.as_str(), "display_name");
    }
}
