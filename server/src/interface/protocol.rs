//! Online Multiplayer Protocol (spec: shared "Online Multiplayer Protocol",
//! design D2): versioned JSON over WebSocket, stable error codes, full-state
//! snapshots as the resync contract.

use serde::{Deserialize, Serialize};

/// Protocol version carried by every message (`"v": 1`).
pub const VERSION: u32 = 1;

/// WebSocket close code reserved for protocol-level failures (malformed
/// JSON, unknown message type, unsupported version). Business-rule errors
/// are answered with an `error` message instead and keep the socket open.
pub const PROTOCOL_CLOSE_CODE: u16 = 4000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Color {
    White,
    Black,
}

impl Color {
    pub fn opponent(self) -> Color {
        match self {
            Color::White => Color::Black,
            Color::Black => Color::White,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Playing,
    Checkmated { winner: Color },
    Drawn,
    Resigned { winner: Color },
    Forfeited { winner: Color },
    /// Flag fall (spec "A flag fall ends the game..."): the `winner` is the
    /// opponent of the player whose clock ran out.
    TimedOut { winner: Color },
}

impl Status {
    pub fn is_terminal(&self) -> bool {
        !matches!(self, Status::Playing)
    }
}

/// Full state snapshot: a client can rebuild the entire game screen from one
/// of these without any other message (spec scenario "A state snapshot
/// rebuilds the game screen").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct State {
    /// Board placement (8 FEN fields) from the authoritative core session.
    pub board_fen: String,
    /// Full move history in UCI, in play order.
    pub move_list: Vec<String>,
    /// Side to move: "w" or "b" (derived from the move-list length).
    pub side_to_move: String,
    pub status: Status,
    pub your_color: Color,
    pub white_rating: f64,
    pub black_rating: f64,
    pub opponent_online: bool,
    /// The room's time control label (e.g. `"15+10"`).
    pub time_control: String,
    /// White's remaining time in milliseconds (server-authoritative).
    pub white_time_ms: u64,
    /// Black's remaining time in milliseconds (server-authoritative).
    pub black_time_ms: u64,
}

/// Stable machine-readable error codes (spec "Errors carry a stable code").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    RoomNotFound,
    RoomFull,
    AlreadyInRoom,
    InvalidRoomCode,
    NotYourTurn,
    IllegalMove,
    GameOver,
    NotConnected,
    Forfeit,
    /// The username is taken, compared case-insensitively (spec "Player
    /// Account Registration").
    UsernameTaken,
    /// The email address is taken, compared case-insensitively (spec "Email
    /// Identity").
    EmailTaken,
    /// The username does not exist or the password is wrong. One code and one
    /// message for both, so a caller cannot learn which usernames are
    /// registered (spec "Login and Session Issuance").
    InvalidCredentials,
    /// The connection has no usable session (spec "Authentication Wire
    /// Messages").
    NotAuthenticated,
    /// The presented session's lifetime has elapsed. Distinct from
    /// `NotAuthenticated` because the client is told the difference between
    /// "never signed in" and "your sign-in aged out", and neither renews it
    /// (spec "Session Lifetime, Reuse, and Revocation").
    SessionExpired,
    /// A field failed its format check. The message names the offending field
    /// (spec "Player Account Registration", "Profile Display Name"), and the
    /// error carries the field's name in a machine-readable field (spec
    /// "Authentication Error Codes").
    InvalidRequest,
    /// The display name is outside 1 to 32 trimmed characters or contains a
    /// control character (spec "Profile Display Name").
    InvalidDisplayName,
    /// No tournament is registered under that id (add-tournaments phase 1).
    TournamentNotFound,
    /// This player is already waiting in or playing a tournament game.
    AlreadyInTournament,
    /// The tournament exists but its window hasn't started or has ended.
    TournamentNotRunning,
    /// The attempt was refused because too many were made (spec
    /// "Authentication Rate Limiting"). The message says when the player may
    /// try again.
    TooManyAttempts,
    /// The password-reset token is unknown, expired, or already used (spec
    /// "Password Reset by Email").
    InvalidResetToken,
    /// The confirmation token is unknown, expired, already used, or belongs
    /// to another address (spec "Email Verification").
    InvalidVerificationToken,
    /// The account's email address is not confirmed (spec "Email
    /// Verification").
    EmailNotVerified,
    /// The message could not be delivered (spec "Pluggable Email Delivery").
    EmailUnavailable,
}

impl ErrorCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            ErrorCode::RoomNotFound => "room_not_found",
            ErrorCode::RoomFull => "room_full",
            ErrorCode::AlreadyInRoom => "already_in_room",
            ErrorCode::InvalidRoomCode => "invalid_room_code",
            ErrorCode::NotYourTurn => "not_your_turn",
            ErrorCode::IllegalMove => "illegal_move",
            ErrorCode::GameOver => "game_over",
            ErrorCode::NotConnected => "not_connected",
            ErrorCode::Forfeit => "forfeit",
            ErrorCode::UsernameTaken => "username_taken",
            ErrorCode::EmailTaken => "email_taken",
            ErrorCode::InvalidCredentials => "invalid_credentials",
            ErrorCode::NotAuthenticated => "not_authenticated",
            ErrorCode::SessionExpired => "session_expired",
            ErrorCode::InvalidRequest => "invalid_request",
            ErrorCode::InvalidDisplayName => "invalid_display_name",
            ErrorCode::TournamentNotFound => "tournament_not_found",
            ErrorCode::AlreadyInTournament => "already_in_tournament",
            ErrorCode::TournamentNotRunning => "tournament_not_running",
            ErrorCode::TooManyAttempts => "too_many_attempts",
            ErrorCode::InvalidResetToken => "invalid_reset_token",
            ErrorCode::InvalidVerificationToken => "invalid_verification_token",
            ErrorCode::EmailNotVerified => "email_not_verified",
            ErrorCode::EmailUnavailable => "email_unavailable",
        }
    }

    /// The generic credential-failure message. Deliberately says nothing about
    /// which half of the credential pair was wrong, and reveals nothing about
    /// whether the username exists (spec "Login and Session Issuance").
    pub const GENERIC_CREDENTIAL_MESSAGE: &'static str =
        "That username and password do not match an account";

    pub fn message(&self) -> &'static str {
        match self {
            ErrorCode::RoomNotFound => "No open room has that code",
            ErrorCode::RoomFull => "That room already has two players",
            ErrorCode::AlreadyInRoom => "This player is already in a room",
            ErrorCode::InvalidRoomCode => "Room codes are 6 characters from A-Z and 2-9 (no I or O)",
            ErrorCode::NotYourTurn => "It is not your turn",
            ErrorCode::IllegalMove => "That move is not legal in the current position",
            ErrorCode::GameOver => "The game is already over",
            ErrorCode::NotConnected => "The game has not started yet",
            ErrorCode::Forfeit => "The opponent did not reconnect in time",
            ErrorCode::UsernameTaken => "That username is already registered",
            ErrorCode::EmailTaken => "That email address is already registered",
            ErrorCode::InvalidCredentials => Self::GENERIC_CREDENTIAL_MESSAGE,
            ErrorCode::NotAuthenticated => "This connection is not signed in",
            ErrorCode::SessionExpired => "This session has expired; log in again",
            ErrorCode::InvalidRequest => "A field failed validation",
            ErrorCode::InvalidDisplayName => {
                "A display name must be 1 to 32 characters with no control characters"
            }
            ErrorCode::TournamentNotFound => "No tournament has that id",
            ErrorCode::AlreadyInTournament => "This player is already in a tournament",
            ErrorCode::TournamentNotRunning => "This tournament is not currently running",
            ErrorCode::TooManyAttempts => "Too many attempts; try again later",
            ErrorCode::InvalidResetToken => "That reset link is no longer valid",
            ErrorCode::InvalidVerificationToken => "That confirmation link is no longer valid",
            ErrorCode::EmailNotVerified => "This email address is not confirmed yet",
            ErrorCode::EmailUnavailable => "The message could not be sent; try again later",
        }
    }

    /// An error with a custom message, for the validation codes whose whole
    /// job is to say which field and why (spec "a validation error naming the
    /// offending field").
    pub fn with_message(self, message: impl Into<String>) -> ServerMessage {
        ServerMessage::Error {
            v: VERSION,
            code: self.as_str().to_string(),
            message: message.into(),
            field: None,
        }
    }

    /// An error with a custom message *and* the name of the field that was
    /// refused, so a client can mark the right input without reading the
    /// message text (spec "Authentication Error Codes").
    pub fn with_field(self, field: &str, message: impl Into<String>) -> ServerMessage {
        ServerMessage::Error {
            v: VERSION,
            code: self.as_str().to_string(),
            message: message.into(),
            field: Some(field.to_string()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    CreateRoom {
        v: u32,
        player_id: String,
        /// The chosen time-control label (design D5): absent or unknown
        /// means the default control, so older clients keep working.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        time_control: Option<String>,
        /// A session token stored from an earlier login (spec "Guest Play
        /// Fallback"): present resolves the connection to its account,
        /// absent leaves it a guest. Optional so a client that never
        /// authenticates sends nothing and behaves exactly as before.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token: Option<String>,
    },
    JoinRoom {
        v: u32,
        player_id: String,
        room_code: String,
        /// A stored session token, with the same meaning as on
        /// `CreateRoom`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token: Option<String>,
    },
    Move {
        v: u32,
        uci: String,
    },
    Resign {
        v: u32,
    },
    Leave {
        v: u32,
    },
    /// Create an account and sign in to it (spec "Player Account
    /// Registration").
    Register {
        v: u32,
        username: String,
        /// The account's private identity (spec "Email Identity"). Optional
        /// on the wire so a client built before the email existed is
        /// answered with a validation error naming the field, rather than
        /// having its connection closed for a malformed frame.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        email: Option<String>,
        password: String,
        /// The device to bind to the new account (spec "Device-to-Account
        /// Profile Linkage"). Optional: absent, the device is bound by the
        /// first `create_room`/`join_room` that carries this connection's
        /// token, so a client that authenticates before choosing a device id
        /// still ends up linked.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        device_id: Option<String>,
    },
    /// Exchange an identifier and a password for a session (spec "Login and
    /// Session Issuance"). The identifier is the account's username or its
    /// email address.
    Login {
        v: u32,
        /// The legacy field: the account's username. Kept so a client built
        /// before the identifier existed still signs in.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        username: Option<String>,
        /// What the player typed: a username or an email address. The
        /// server resolves it against both.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        identifier: Option<String>,
        password: String,
        /// The device to bind to the account, with the same meaning and the
        /// same fallback as on `Register`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        device_id: Option<String>,
    },
    /// Revoke the presented session, and only it (spec "Session Lifetime,
    /// Reuse, and Revocation"). The device-to-account link survives.
    Logout {
        v: u32,
        token: String,
    },
    /// Set the account's display name (spec "Profile Display Name").
    SetProfile {
        v: u32,
        display_name: String,
        /// The session being used; a display-name change is
        /// authenticated-only.
        token: String,
    },
    /// Resume a session the client stored earlier (spec "Session
    /// Resumption").
    ResumeSession {
        v: u32,
        /// The stored session token.
        token: String,
    },
    /// Ask for a password-reset link (spec "Password Reset by Email").
    ForgotPassword {
        v: u32,
        /// The address the player typed.
        email: String,
    },
    /// Set a new password with a reset token (spec "Password Reset by
    /// Email").
    ResetPassword {
        v: u32,
        /// The reset token the player received.
        token: String,
        /// The new password.
        password: String,
    },
    /// Confirm an email address with the token the player received (spec
    /// "Email Verification").
    VerifyEmail {
        v: u32,
        token: String,
    },
    /// Ask for another verification message (spec "Email Verification").
    ResendVerification {
        v: u32,
        /// The session of the account whose address is unconfirmed.
        token: String,
    },
    /// Ask for a different email address (spec "Email Change").
    ChangeEmail {
        v: u32,
        /// The session of the account whose address is changing.
        token: String,
        /// The new address.
        email: String,
    },
    /// Join an Arena tournament's waiting pool (add-tournaments phase 1): the
    /// server pairs this connection with another waiting player and delivers
    /// `tournament_pairing` when a game starts.
    JoinTournament {
        v: u32,
        player_id: String,
        tournament_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token: Option<String>,
    },
    /// Leave the tournament's waiting pool. Does not forfeit a game already
    /// in progress (leaving an in-progress game uses `leave`, same as a
    /// casual room).
    LeaveTournament {
        v: u32,
        tournament_id: String,
    },
    /// List tournaments the server knows about (add-tournaments phase 1: no
    /// lobby UI yet, this is the discovery API for testing).
    ListTournaments {
        v: u32,
    },
}

impl ClientMessage {
    pub fn v(&self) -> u32 {
        match self {
            ClientMessage::CreateRoom { v, .. }
            | ClientMessage::JoinRoom { v, .. }
            | ClientMessage::Move { v, .. }
            | ClientMessage::Resign { v }
            | ClientMessage::Leave { v }
            | ClientMessage::Register { v, .. }
            | ClientMessage::Login { v, .. }
            | ClientMessage::Logout { v, .. }
            | ClientMessage::SetProfile { v, .. }
            | ClientMessage::ResumeSession { v, .. }
            | ClientMessage::ForgotPassword { v, .. }
            | ClientMessage::ResetPassword { v, .. }
            | ClientMessage::VerifyEmail { v, .. }
            | ClientMessage::ResendVerification { v, .. }
            | ClientMessage::ChangeEmail { v, .. }
            | ClientMessage::JoinTournament { v, .. }
            | ClientMessage::LeaveTournament { v, .. }
            | ClientMessage::ListTournaments { v, .. } => *v,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    /// Sent to a player when their seat is created: the room code, their
    /// color, and the initial state snapshot.
    RoomReady {
        v: u32,
        room_code: String,
        your_color: Color,
        state: State,
    },
    /// Full state snapshot after any accepted action and on re-attach.
    State { v: u32, state: State },
    /// Incremental update after an accepted action (spec "Server Game
    /// Authority"): the applied move, when there is one, plus the fields that
    /// can change. Clients replay `uci` into their mirror session instead of
    /// rebuilding the whole screen from a snapshot; the full `State` is
    /// reserved for connect/re-attach.
    Update {
        v: u32,
        /// The UCI move just applied; `None` for a terminal result with no
        /// move (resignation, forfeit, flag fall).
        uci: Option<String>,
        side_to_move: String,
        status: Status,
        white_rating: f64,
        black_rating: f64,
        opponent_online: bool,
        white_time_ms: u64,
        black_time_ms: u64,
    },
    /// Structured business-rule error (connection stays open).
    Error {
        v: u32,
        code: String,
        message: String,
        /// The field that was refused, when the refusal was about one field
        /// (spec "Authentication Error Codes"). `None` for a refusal that is
        /// not about a field.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        field: Option<String>,
    },
    /// A newly issued session (spec "Login and Session Issuance"). Carries
    /// the token exactly once: the server stores only its hash, so this
    /// response is the sole chance to hand it to the client.
    Session {
        v: u32,
        account_id: String,
        username: String,
        /// The account's display name, defaulting to the username.
        display_name: String,
        token: String,
        expires_at_ms: i64,
        /// Whether the account's email address is confirmed (spec "Email
        /// Verification").
        email_verified: bool,
    },
    /// A session that ended without a replacement (spec "Session Lifetime,
    /// Reuse, and Revocation"). Carries no token so a client cannot mistake
    /// it for a fresh sign-in.
    SessionOk {
        v: u32,
        account_id: String,
        username: String,
        display_name: String,
        expires_at_ms: i64,
    },
    /// The acknowledgement of a password-reset request. The answer is the
    /// same whether or not the address belongs to an account, so it reveals
    /// nothing (spec "Password Reset by Email").
    PasswordResetRequested {
        v: u32,
    },
    /// The password was changed. It carries no session: a reset does not
    /// sign the player in (spec "Password Reset by Email").
    PasswordReset {
        v: u32,
        account_id: String,
    },
    /// The acknowledgement of a verification request (spec "Email
    /// Verification").
    EmailVerificationRequested {
        v: u32,
    },
    /// The account's email address is confirmed (spec "Email Verification").
    EmailVerified {
        v: u32,
        account_id: String,
        email_verified: bool,
    },
    /// The session a stored token resumed (spec "Session Resumption").
    ///
    /// It carries no token: resuming issues nothing new and extends
    /// nothing.
    SessionResumed {
        v: u32,
        account_id: String,
        username: String,
        /// The account's display name, defaulting to the username.
        display_name: String,
        expires_at_ms: i64,
        /// Whether the account's email address is confirmed.
        email_verified: bool,
    },
    /// The account's new display name (spec "Profile Display Name").
    ProfileUpdated {
        v: u32,
        account_id: String,
        display_name: String,
    },
    /// Registered in a tournament's waiting pool (add-tournaments phase 1).
    TournamentJoined {
        v: u32,
        tournament_id: String,
    },
    /// This connection was paired into a new tournament game: a room was
    /// created and this seat installed, the same as `room_ready` but without
    /// a client-sent `create_room`/`join_room`.
    TournamentPairing {
        v: u32,
        tournament_id: String,
        room_code: String,
        your_color: Color,
        state: State,
    },
    /// A tournament's current leaderboard, in descending point order.
    TournamentStandings {
        v: u32,
        tournament_id: String,
        entries: Vec<StandingEntry>,
    },
    /// The tournament's window closed; `final_standings` is the last
    /// leaderboard snapshot.
    TournamentEnded {
        v: u32,
        tournament_id: String,
        final_standings: Vec<StandingEntry>,
    },
    /// The tournaments the server currently knows about (no lobby UI yet;
    /// this is the discovery API for testing, add-tournaments phase 1).
    TournamentList {
        v: u32,
        tournaments: Vec<TournamentSummary>,
    },
}

/// One player's row in a tournament leaderboard. `points` is doubled (win=2,
/// draw=1, loss=0) to stay integer on the wire, same as the domain model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StandingEntry {
    pub player_id: String,
    pub points: u32,
    pub games_played: u32,
}

/// A tournament's static info, for `tournament_list` (add-tournaments phase 1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TournamentSummary {
    pub tournament_id: String,
    pub status: String,
    pub time_control: String,
    pub starts_at_ms: i64,
    pub duration_ms: i64,
}

impl ServerMessage {
    pub fn error(code: ErrorCode) -> Self {
        ServerMessage::Error {
            v: VERSION,
            code: code.as_str().to_string(),
            message: code.message().to_string(),
            field: None,
        }
    }
}

/// Classification of a failed incoming frame (design D2): all of these are
/// protocol-level, so the server closes with `PROTOCOL_CLOSE_CODE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    Malformed,
    UnknownType,
    UnsupportedVersion,
}

const KNOWN_TYPES: &[&str] = &[
    "create_room", "join_room", "move", "resign", "leave",
    "register", "login", "logout", "set_profile",
    "resume_session", "forgot_password", "reset_password",
    "verify_email", "resend_verification", "change_email",
    "join_tournament", "leave_tournament", "list_tournaments",
];

/// Decode one incoming text frame. Business rules (turn, legality, room
/// state) are validated by the room actor, not here.
pub fn decode_incoming(data: &str) -> Result<ClientMessage, DecodeError> {
    let value: serde_json::Value =
        serde_json::from_str(data).map_err(|_| DecodeError::Malformed)?;
    let object = value.as_object().ok_or(DecodeError::Malformed)?;
    let type_ = object
        .get("type")
        .and_then(|v| v.as_str())
        .ok_or(DecodeError::Malformed)?;
    if !KNOWN_TYPES.contains(&type_) {
        return Err(DecodeError::UnknownType);
    }
    let message: ClientMessage =
        serde_json::from_value(value).map_err(|_| DecodeError::Malformed)?;
    if message.v() != VERSION {
        return Err(DecodeError::UnsupportedVersion);
    }
    Ok(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_messages_round_trip() {
        for message in [
            ClientMessage::CreateRoom {
                v: VERSION,
                player_id: "p1".into(),
                time_control: Some("15+10".into()),
                token: None,
            },
            ClientMessage::CreateRoom {
                v: VERSION,
                player_id: "p2".into(),
                time_control: None,
                token: None,
            },
            ClientMessage::JoinRoom {
                v: VERSION,
                player_id: "p2".into(),
                room_code: "AB23CD".into(),
                token: None,
            },
            ClientMessage::Move {
                v: VERSION,
                uci: "e2e4".into(),
            },
            ClientMessage::Move {
                v: VERSION,
                uci: "g7h8q".into(),
            },
            ClientMessage::Resign { v: VERSION },
            ClientMessage::Leave { v: VERSION },
        ] {
            let json = serde_json::to_string(&message).unwrap();
            let back = decode_incoming(&json).unwrap();
            assert_eq!(back, message);
        }
    }

    /// The authentication messages round-trip too (design D11).
    #[test]
    fn auth_client_messages_round_trip() {
        for message in [
            ClientMessage::Register {
                v: VERSION,
                username: "Ana".into(),
                email: Some("ana@example.com".into()),
                password: "correct horse battery".into(),
                device_id: Some("device-a".into()),
            },
            ClientMessage::Register {
                v: VERSION,
                username: "Bob".into(),
                email: None,
                password: "correct horse battery".into(),
                device_id: None,
            },
            ClientMessage::Login {
                v: VERSION,
                username: Some("Ana".into()),
                identifier: None,
                password: "correct horse battery".into(),
                device_id: None,
            },
            ClientMessage::Login {
                v: VERSION,
                username: None,
                identifier: Some("ana@example.com".into()),
                password: "correct horse battery".into(),
                device_id: None,
            },
            ClientMessage::Logout {
                v: VERSION,
                token: "a-token".into(),
            },
            ClientMessage::SetProfile {
                v: VERSION,
                display_name: "Ana T.".into(),
                token: "a-token".into(),
            },
            ClientMessage::ResumeSession {
                v: VERSION,
                token: "a-token".into(),
            },
            ClientMessage::ForgotPassword {
                v: VERSION,
                email: "ana@example.com".into(),
            },
            ClientMessage::ResetPassword {
                v: VERSION,
                token: "a-token".into(),
                password: "a new password".into(),
            },
            ClientMessage::VerifyEmail {
                v: VERSION,
                token: "a-token".into(),
            },
            ClientMessage::ResendVerification {
                v: VERSION,
                token: "a-token".into(),
            },
            ClientMessage::ChangeEmail {
                v: VERSION,
                token: "a-token".into(),
                email: "new@example.com".into(),
            },
        ] {
            let json = serde_json::to_string(&message).unwrap();
            assert_eq!(decode_incoming(&json).unwrap(), message);
        }
    }

    #[test]
    fn server_messages_round_trip() {
        let state = State {
            board_fen: "rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR".into(),
            move_list: vec!["e2e4".into()],
            side_to_move: "b".into(),
            status: Status::Playing,
            your_color: Color::White,
            white_rating: 1500.0,
            black_rating: 1500.0,
            opponent_online: true,
            time_control: "3+2".into(),
            white_time_ms: 181_000,
            black_time_ms: 180_000,
        };
        for message in [
            ServerMessage::RoomReady {
                v: VERSION,
                room_code: "AB23CD".into(),
                your_color: Color::Black,
                state: state.clone(),
            },
            ServerMessage::State {
                v: VERSION,
                state: state.clone(),
            },
            ServerMessage::Update {
                v: VERSION,
                uci: Some("e2e4".into()),
                side_to_move: "b".into(),
                status: Status::Playing,
                white_rating: 1500.0,
                black_rating: 1499.0,
                opponent_online: true,
                white_time_ms: 180_000,
                black_time_ms: 180_000,
            },
            ServerMessage::Update {
                v: VERSION,
                uci: None,
                side_to_move: "b".into(),
                status: Status::Resigned {
                    winner: Color::Black,
                },
                white_rating: 1490.0,
                black_rating: 1510.0,
                opponent_online: true,
                white_time_ms: 180_000,
                black_time_ms: 181_000,
            },
            ServerMessage::error(ErrorCode::NotYourTurn),
            ServerMessage::Session {
                v: VERSION,
                account_id: "account-1".into(),
                username: "Ana".into(),
                display_name: "Ana".into(),
                token: "a-token".into(),
                expires_at_ms: 1_700_000_000_000,
                email_verified: false,
            },
            ServerMessage::SessionResumed {
                v: VERSION,
                account_id: "account-1".into(),
                username: "Ana".into(),
                display_name: "Ana".into(),
                expires_at_ms: 1_700_000_000_000,
                email_verified: true,
            },
            ServerMessage::PasswordResetRequested { v: VERSION },
            ServerMessage::PasswordReset {
                v: VERSION,
                account_id: "account-1".into(),
            },
            ServerMessage::EmailVerificationRequested { v: VERSION },
            ServerMessage::EmailVerified {
                v: VERSION,
                account_id: "account-1".into(),
                email_verified: true,
            },
            ServerMessage::SessionOk {
                v: VERSION,
                account_id: "account-1".into(),
                username: "Ana".into(),
                display_name: "Ana T.".into(),
                expires_at_ms: 1_700_000_000_000,
            },
            ServerMessage::ProfileUpdated {
                v: VERSION,
                account_id: "account-1".into(),
                display_name: "Ana T.".into(),
            },
        ] {
            let json = serde_json::to_string(&message).unwrap();
            let back: ServerMessage = serde_json::from_str(&json).unwrap();
            assert_eq!(back, message);
        }
    }

    #[test]
    fn malformed_frames_are_classified() {
        assert_eq!(decode_incoming("not json"), Err(DecodeError::Malformed));
        assert_eq!(
            decode_incoming(r#"{"type":"create_room","v":1}"#),
            Err(DecodeError::Malformed),
            "missing player_id is malformed"
        );
        assert_eq!(
            decode_incoming(r#"{"type":"teleport","v":1}"#),
            Err(DecodeError::UnknownType)
        );
        assert_eq!(
            decode_incoming(r#"{"type":"move","v":2,"uci":"e2e4"}"#),
            Err(DecodeError::UnsupportedVersion)
        );
        assert_eq!(
            decode_incoming(r#"["type","move"]"#),
            Err(DecodeError::Malformed),
            "non-object frame is malformed"
        );
    }

    /// Every authentication message type decodes, and a type the server does
    /// not know is still refused with 4000 rather than silently ignored
    /// (task 5.3).
    #[test]
    fn the_auth_message_types_decode_and_unknown_types_are_still_refused() {
        assert_eq!(
            decode_incoming(r#"{"type":"register","v":1,"username":"Ana","password":"passw0rd"}"#),
            Ok(ClientMessage::Register {
                v: VERSION,
                username: "Ana".into(),
                email: None,
                password: "passw0rd".into(),
                device_id: None,
            })
        );
        assert_eq!(
            decode_incoming(
                r#"{"type":"login","v":1,"username":"Ana","password":"passw0rd","device_id":"d1"}"#
            ),
            Ok(ClientMessage::Login {
                v: VERSION,
                username: Some("Ana".into()),
                identifier: None,
                password: "passw0rd".into(),
                device_id: Some("d1".into()),
            })
        );
        assert_eq!(
            decode_incoming(r#"{"type":"logout","v":1,"token":"t"}"#),
            Ok(ClientMessage::Logout {
                v: VERSION,
                token: "t".into(),
            })
        );
        assert_eq!(
            decode_incoming(
                r#"{"type":"set_profile","v":1,"display_name":"Ana T.","token":"t"}"#
            ),
            Ok(ClientMessage::SetProfile {
                v: VERSION,
                display_name: "Ana T.".into(),
                token: "t".into(),
            })
        );

        // Still refused: an unknown type, and a bad version on a new type.
        assert_eq!(
            decode_incoming(r#"{"type":"sign_in","v":1,"username":"Ana"}"#),
            Err(DecodeError::UnknownType),
            "`sign_in` is not a type this protocol speaks"
        );
        assert_eq!(
            decode_incoming(r#"{"type":"login","v":2,"username":"Ana","password":"passw0rd"}"#),
            Err(DecodeError::UnsupportedVersion),
            "the new types obey the version check like every other message"
        );
        assert_eq!(
            decode_incoming(r#"{"type":"login","v":1,"username":"Ana"}"#),
            Err(DecodeError::Malformed),
            "a login without a password is malformed"
        );
    }

    /// An unknown username and a wrong password are indistinguishable on the
    /// wire: same code, same message (spec "Login and Session Issuance").
    #[test]
    fn a_credential_failure_is_the_same_wire_message_every_time() {
        let unknown_username = ServerMessage::error(ErrorCode::InvalidCredentials);
        let wrong_password = ServerMessage::error(ErrorCode::InvalidCredentials);
        assert_eq!(unknown_username, wrong_password);

        let value: serde_json::Value = serde_json::to_value(&unknown_username).unwrap();
        assert_eq!(value["code"], "invalid_credentials");
        assert_eq!(
            value["message"],
            ErrorCode::GENERIC_CREDENTIAL_MESSAGE,
            "the message must not hint at which half was wrong"
        );
        let generic = ErrorCode::GENERIC_CREDENTIAL_MESSAGE.to_lowercase();
        for word in ["username", "taken", "exists", "password", "wrong"] {
            assert!(
                !generic.contains(&format!("{word} is"))
                    && !generic.contains(&format!("no such {word}"))
                    && !generic.contains(&format!("incorrect {word}"))
                    && !generic.contains(&format!("wrong {word}")),
                "the generic message must not disclose which field failed: {generic}"
            );
        }
    }

    #[test]
    fn a_validation_error_carries_its_own_message() {
        let message = ErrorCode::InvalidRequest.with_message("username must be 3 to 24 characters");
        let value: serde_json::Value = serde_json::to_value(&message).unwrap();
        assert_eq!(value["code"], "invalid_request");
        assert_eq!(value["message"], "username must be 3 to 24 characters");
    }

    /// Pins the exact JSON shape of the wire format (the iOS/Android codecs
    /// are written against these field names, so a change here is a protocol
    /// change and must fail this test).
    #[test]
    fn wire_form_is_stable() {
        // Client messages: `type` tag + `v` + variant fields, inline.
        let value: serde_json::Value =
            serde_json::to_value(&ClientMessage::CreateRoom {
                v: VERSION,
                player_id: "p1".into(),
                time_control: Some("15+10".into()),
                token: None,
            })
            .unwrap();
        assert_eq!(value["type"], "create_room");
        assert_eq!(value["v"], 1);
        assert_eq!(value["player_id"], "p1");
        assert_eq!(value["time_control"], "15+10");

        // A create_room with no control omits the field entirely, so the
        // wire stays compatible with clients that never send one (D5).
        let value: serde_json::Value =
            serde_json::to_value(&ClientMessage::CreateRoom {
                v: VERSION,
                player_id: "p2".into(),
                time_control: None,
                token: None,
            })
            .unwrap();
        assert!(value.get("time_control").is_none());

        // Same rule for the token: absent means "guest", and the field is
        // omitted rather than sent as null, so a client built before
        // authentication existed produces a byte-identical frame.
        assert!(
            value.get("token").is_none(),
            "a guest must not have to send a token field at all"
        );
        let value: serde_json::Value =
            serde_json::to_value(&ClientMessage::CreateRoom {
                v: VERSION,
                player_id: "p3".into(),
                time_control: None,
                token: Some("a-token".into()),
            })
            .unwrap();
        assert_eq!(value["token"], "a-token");

        let value: serde_json::Value =
            serde_json::to_value(&ClientMessage::JoinRoom {
                v: VERSION,
                player_id: "p2".into(),
                room_code: "AB23CD".into(),
                token: None,
            })
            .unwrap();
        assert_eq!(value["type"], "join_room");
        assert_eq!(value["room_code"], "AB23CD");
        assert!(value.get("token").is_none());

        // Tournament messages (add-tournaments phase 1): `type` tag + `v` +
        // fields, inline, same rule as every other client message.
        let value: serde_json::Value =
            serde_json::to_value(&ClientMessage::JoinTournament {
                v: VERSION,
                player_id: "p1".into(),
                tournament_id: "t1".into(),
                token: None,
            })
            .unwrap();
        assert_eq!(value["type"], "join_tournament");
        assert_eq!(value["tournament_id"], "t1");
        assert!(value.get("token").is_none());

        let value: serde_json::Value =
            serde_json::to_value(&ClientMessage::LeaveTournament {
                v: VERSION,
                tournament_id: "t1".into(),
            })
            .unwrap();
        assert_eq!(value["type"], "leave_tournament");
        assert_eq!(value["tournament_id"], "t1");

        let value: serde_json::Value =
            serde_json::to_value(&ClientMessage::ListTournaments { v: VERSION }).unwrap();
        assert_eq!(value["type"], "list_tournaments");

        // Authentication messages: `type` tag + `v` + fields, inline.
        let value: serde_json::Value =
            serde_json::to_value(&ClientMessage::Register {
                v: VERSION,
                username: "Ana".into(),
                email: Some("ana@example.com".into()),
                password: "correct horse battery".into(),
                device_id: Some("device-a".into()),
            })
            .unwrap();
        assert_eq!(value["type"], "register");
        assert_eq!(value["username"], "Ana");
        assert_eq!(value["email"], "ana@example.com");
        assert_eq!(value["password"], "correct horse battery");
        assert_eq!(value["device_id"], "device-a");

        let value: serde_json::Value =
            serde_json::to_value(&ClientMessage::Register {
                v: VERSION,
                username: "Ana".into(),
                email: None,
                password: "correct horse battery".into(),
                device_id: None,
            })
            .unwrap();
        assert!(
            value.get("device_id").is_none(),
            "the device is optional: a client that has not chosen one omits it"
        );
        assert!(
            value.get("email").is_none(),
            "the email is optional on the wire, so a client built before it existed is answered with a validation error instead of a closed connection"
        );

        // The sign-in message carries the identifier the player typed. The
        // legacy `username` field stays on the wire, so a client built
        // before the identifier existed still signs in.
        let value: serde_json::Value =
            serde_json::to_value(&ClientMessage::Login {
                v: VERSION,
                username: Some("Ana".into()),
                identifier: None,
                password: "correct horse battery".into(),
                device_id: Some("device-a".into()),
            })
            .unwrap();
        assert_eq!(value["type"], "login");
        assert_eq!(value["username"], "Ana");
        assert_eq!(value["device_id"], "device-a");

        let value: serde_json::Value =
            serde_json::to_value(&ClientMessage::Login {
                v: VERSION,
                username: None,
                identifier: Some("ana@example.com".into()),
                password: "correct horse battery".into(),
                device_id: None,
            })
            .unwrap();
        assert_eq!(value["identifier"], "ana@example.com");
        assert!(
            value.get("username").is_none(),
            "a client that sends the identifier omits the legacy field"
        );

        // The email flows are ordinary client messages: `type` tag + `v` +
        // fields, inline.
        let value: serde_json::Value =
            serde_json::to_value(&ClientMessage::ResumeSession {
                v: VERSION,
                token: "a-token".into(),
            })
            .unwrap();
        assert_eq!(value["type"], "resume_session");
        let value: serde_json::Value =
            serde_json::to_value(&ClientMessage::ForgotPassword {
                v: VERSION,
                email: "ana@example.com".into(),
            })
            .unwrap();
        assert_eq!(value["type"], "forgot_password");
        let value: serde_json::Value =
            serde_json::to_value(&ClientMessage::ResetPassword {
                v: VERSION,
                token: "a-token".into(),
                password: "a new password".into(),
            })
            .unwrap();
        assert_eq!(value["type"], "reset_password");
        let value: serde_json::Value =
            serde_json::to_value(&ClientMessage::VerifyEmail {
                v: VERSION,
                token: "a-token".into(),
            })
            .unwrap();
        assert_eq!(value["type"], "verify_email");
        let value: serde_json::Value =
            serde_json::to_value(&ClientMessage::ResendVerification {
                v: VERSION,
                token: "a-token".into(),
            })
            .unwrap();
        assert_eq!(value["type"], "resend_verification");
        let value: serde_json::Value =
            serde_json::to_value(&ClientMessage::ChangeEmail {
                v: VERSION,
                token: "a-token".into(),
                email: "new@example.com".into(),
            })
            .unwrap();
        assert_eq!(value["type"], "change_email");

        let value: serde_json::Value = serde_json::to_value(&ClientMessage::Logout {
            v: VERSION,
            token: "a-token".into(),
        })
        .unwrap();
        assert_eq!(value["type"], "logout");
        assert_eq!(value["token"], "a-token");

        let value: serde_json::Value =
            serde_json::to_value(&ClientMessage::SetProfile {
                v: VERSION,
                display_name: "Ana T.".into(),
                token: "a-token".into(),
            })
            .unwrap();
        assert_eq!(value["type"], "set_profile");
        assert_eq!(value["display_name"], "Ana T.");
        assert_eq!(value["token"], "a-token");

        let value: serde_json::Value =
            serde_json::to_value(&ClientMessage::Move {
                v: VERSION,
                uci: "e2e4".into(),
            })
            .unwrap();
        assert_eq!(value["type"], "move");
        assert_eq!(value["uci"], "e2e4");

        let value: serde_json::Value =
            serde_json::to_value(&ClientMessage::Resign { v: VERSION }).unwrap();
        assert_eq!(value["type"], "resign");
        assert!(value.get("uci").is_none());

        // State snapshots: every field present, status inline-tagged.
        let state = State {
            board_fen: "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR".into(),
            move_list: vec!["e2e4".into()],
            side_to_move: "b".into(),
            status: Status::Playing,
            your_color: Color::White,
            white_rating: 1500.0,
            black_rating: 1499.0,
            opponent_online: true,
            time_control: "5+0".into(),
            white_time_ms: 300_000,
            black_time_ms: 295_500,
        };
        let value: serde_json::Value = serde_json::to_value(&state).unwrap();
        assert_eq!(value["board_fen"], "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR");
        assert_eq!(value["move_list"][0], "e2e4");
        assert_eq!(value["side_to_move"], "b");
        // Status is externally tagged: unit variants are plain strings...
        assert_eq!(value["status"], "playing");
        assert_eq!(value["your_color"], "white");
        assert_eq!(value["white_rating"], 1500.0);
        assert_eq!(value["black_rating"], 1499.0);
        assert_eq!(value["opponent_online"], true);
        assert_eq!(value["time_control"], "5+0");
        assert_eq!(value["white_time_ms"], 300_000);
        assert_eq!(value["black_time_ms"], 295_500);

        // ...struct variants are `{"<variant>": {"winner": <color>}}`.
        for (status, variant, winner) in [
            (Status::Checkmated { winner: Color::Black }, "checkmated", Some("black")),
            (Status::Drawn, "drawn", None),
            (Status::Resigned { winner: Color::White }, "resigned", Some("white")),
            (Status::Forfeited { winner: Color::Black }, "forfeited", Some("black")),
            (Status::TimedOut { winner: Color::White }, "timed_out", Some("white")),
        ] {
            let value: serde_json::Value = serde_json::to_value(status).unwrap();
            match winner {
                Some(w) => {
                    assert_eq!(value[variant]["winner"], w, "status `{variant}`");
                }
                None => assert_eq!(value, variant, "unit status `{variant}`"),
            }
        }

        // Server messages: `type` tag + `v` + variant fields, inline.
        let value: serde_json::Value =
            serde_json::to_value(&ServerMessage::RoomReady {
                v: VERSION,
                room_code: "AB23CD".into(),
                your_color: Color::Black,
                state,
            })
            .unwrap();
        assert_eq!(value["type"], "room_ready");
        assert_eq!(value["room_code"], "AB23CD");
        assert_eq!(value["your_color"], "black");
        assert_eq!(value["state"]["side_to_move"], "b");

        let value: serde_json::Value =
            serde_json::to_value(&ServerMessage::State {
                v: VERSION,
                state: State {
                    board_fen: "x".into(),
                    move_list: vec![],
                    side_to_move: "w".into(),
                    status: Status::Playing,
                    your_color: Color::Black,
                    white_rating: 1500.0,
                    black_rating: 1500.0,
                    opponent_online: false,
                    time_control: "1+0".into(),
                    white_time_ms: 60_000,
                    black_time_ms: 60_000,
                },
            })
            .unwrap();
        assert_eq!(value["type"], "state");
        assert_eq!(value["state"]["board_fen"], "x");

        let value: serde_json::Value = serde_json::to_value(&ServerMessage::Update {
            v: VERSION,
            uci: Some("e7e5".into()),
            side_to_move: "w".into(),
            status: Status::Playing,
            white_rating: 1500.0,
            black_rating: 1500.0,
            opponent_online: true,
            white_time_ms: 179_000,
            black_time_ms: 180_000,
        })
        .unwrap();
        assert_eq!(value["type"], "update");
        assert_eq!(value["uci"], "e7e5");
        assert_eq!(value["side_to_move"], "w");
        assert_eq!(value["status"], "playing");
        assert_eq!(value["white_time_ms"], 179_000);
        assert_eq!(value["black_time_ms"], 180_000);

        let value: serde_json::Value = serde_json::to_value(&ServerMessage::Update {
            v: VERSION,
            uci: None,
            side_to_move: "w".into(),
            status: Status::Resigned {
                winner: Color::White,
            },
            white_rating: 1510.0,
            black_rating: 1490.0,
            opponent_online: false,
            white_time_ms: 100_000,
            black_time_ms: 100_000,
        })
        .unwrap();
        assert_eq!(value["type"], "update");
        assert!(value["uci"].is_null(), "a resignation carries no move");
        assert_eq!(value["status"]["resigned"]["winner"], "white");
        assert_eq!(value["opponent_online"], false);

        let value: serde_json::Value =
            serde_json::to_value(ServerMessage::error(ErrorCode::RoomFull)).unwrap();
        assert_eq!(value["type"], "error");
        assert_eq!(value["code"], "room_full");
        assert!(value["message"].is_string());

        // The issued session is the only place a token appears, so its wire
        // form is pinned here like every other shape.
        let value: serde_json::Value = serde_json::to_value(&ServerMessage::Session {
            v: VERSION,
            account_id: "account-1".into(),
            username: "Ana".into(),
            display_name: "Ana".into(),
            token: "a-token".into(),
            expires_at_ms: 1_700_000_000_000i64,
            email_verified: false,
        })
        .unwrap();
        assert_eq!(value["type"], "session");
        assert_eq!(value["account_id"], "account-1");
        assert_eq!(value["username"], "Ana");
        assert_eq!(value["display_name"], "Ana");
        assert_eq!(value["token"], "a-token");
        assert_eq!(value["expires_at_ms"], 1_700_000_000_000i64);

        let value: serde_json::Value = serde_json::to_value(&ServerMessage::SessionOk {
            v: VERSION,
            account_id: "account-1".into(),
            username: "Ana".into(),
            display_name: "Ana T.".into(),
            expires_at_ms: 1_700_000_000_000i64,
        })
        .unwrap();
        assert_eq!(value["type"], "session_ok");
        assert_eq!(value["display_name"], "Ana T.");
        assert!(
            value.get("token").is_none(),
            "a logout must not look like a fresh sign-in"
        );

        let value: serde_json::Value = serde_json::to_value(&ServerMessage::ProfileUpdated {
            v: VERSION,
            account_id: "account-1".into(),
            display_name: "Ana T.".into(),
        })
        .unwrap();
        assert_eq!(value["type"], "profile_updated");
        assert_eq!(value["account_id"], "account-1");
        assert_eq!(value["display_name"], "Ana T.");

        // Tournament messages (add-tournaments phase 1).
        let value: serde_json::Value = serde_json::to_value(&ServerMessage::TournamentJoined {
            v: VERSION,
            tournament_id: "t1".into(),
        })
        .unwrap();
        assert_eq!(value["type"], "tournament_joined");
        assert_eq!(value["tournament_id"], "t1");

        let value: serde_json::Value = serde_json::to_value(&ServerMessage::TournamentPairing {
            v: VERSION,
            tournament_id: "t1".into(),
            room_code: "AB23CD".into(),
            your_color: Color::White,
            state: State {
                board_fen: "x".into(),
                move_list: vec![],
                side_to_move: "w".into(),
                status: Status::Playing,
                your_color: Color::White,
                white_rating: 1500.0,
                black_rating: 1500.0,
                opponent_online: true,
                time_control: "5+0".into(),
                white_time_ms: 300_000,
                black_time_ms: 300_000,
            },
        })
        .unwrap();
        assert_eq!(value["type"], "tournament_pairing");
        assert_eq!(value["room_code"], "AB23CD");
        assert_eq!(value["your_color"], "white");
        assert_eq!(value["state"]["time_control"], "5+0");

        let value: serde_json::Value = serde_json::to_value(&ServerMessage::TournamentStandings {
            v: VERSION,
            tournament_id: "t1".into(),
            entries: vec![StandingEntry {
                player_id: "device-a".into(),
                points: 4,
                games_played: 2,
            }],
        })
        .unwrap();
        assert_eq!(value["type"], "tournament_standings");
        assert_eq!(value["entries"][0]["player_id"], "device-a");
        assert_eq!(value["entries"][0]["points"], 4);

        let value: serde_json::Value = serde_json::to_value(&ServerMessage::TournamentEnded {
            v: VERSION,
            tournament_id: "t1".into(),
            final_standings: vec![StandingEntry {
                player_id: "device-a".into(),
                points: 6,
                games_played: 3,
            }],
        })
        .unwrap();
        assert_eq!(value["type"], "tournament_ended");
        assert_eq!(value["final_standings"][0]["points"], 6);

        let value: serde_json::Value = serde_json::to_value(&ServerMessage::TournamentList {
            v: VERSION,
            tournaments: vec![TournamentSummary {
                tournament_id: "t1".into(),
                status: "running".into(),
                time_control: "5+0".into(),
                starts_at_ms: 1_700_000_000_000,
                duration_ms: 3_600_000,
            }],
        })
        .unwrap();
        assert_eq!(value["type"], "tournament_list");
        assert_eq!(value["tournaments"][0]["status"], "running");
    }

    #[test]
    fn every_error_code_has_a_stable_wire_form() {
        for code in [
            ErrorCode::RoomNotFound,
            ErrorCode::RoomFull,
            ErrorCode::AlreadyInRoom,
            ErrorCode::InvalidRoomCode,
            ErrorCode::NotYourTurn,
            ErrorCode::IllegalMove,
            ErrorCode::GameOver,
            ErrorCode::NotConnected,
            ErrorCode::Forfeit,
            ErrorCode::UsernameTaken,
            ErrorCode::InvalidCredentials,
            ErrorCode::NotAuthenticated,
            ErrorCode::SessionExpired,
            ErrorCode::InvalidRequest,
            ErrorCode::InvalidDisplayName,
            ErrorCode::TournamentNotFound,
            ErrorCode::AlreadyInTournament,
            ErrorCode::TournamentNotRunning,
        ] {
            let message = ServerMessage::error(code);
            let json = serde_json::to_string(&message).unwrap();
            assert!(json.contains(format!(r#""code":"{}""#, code.as_str()).as_str()));
        }
    }

    /// The protocol version did not move (design D11): a bump would close
    /// every deployed socket with 4000, so authentication had to be purely
    /// additive.
    #[test]
    fn the_protocol_version_is_unchanged() {
        assert_eq!(VERSION, 1);
        for json in [
            r#"{"type":"create_room","v":1,"player_id":"p"}"#,
            r#"{"type":"join_room","v":1,"player_id":"p","room_code":"AB23CD"}"#,
            r#"{"type":"move","v":1,"uci":"e2e4"}"#,
            r#"{"type":"resign","v":1}"#,
            r#"{"type":"leave","v":1}"#,
            r#"{"type":"register","v":1,"username":"Ana","password":"passw0rd"}"#,
            r#"{"type":"login","v":1,"username":"Ana","password":"passw0rd"}"#,
            r#"{"type":"logout","v":1,"token":"t"}"#,
            r#"{"type":"set_profile","v":1,"display_name":"Ana","token":"t"}"#,
        ] {
            assert!(
                decode_incoming(json).is_ok(),
                "`{json}` must decode at the current version"
            );
        }
    }

    /// A client that never authenticates sends exactly the frames it sent
    /// before (spec scenario "Existing clients are unaffected").
    #[test]
    fn a_guest_only_client_sends_the_pre_existing_frames() {
        for json in [
            r#"{"type":"create_room","v":1,"player_id":"p1","time_control":"15+10"}"#,
            r#"{"type":"create_room","v":1,"player_id":"p1"}"#,
            r#"{"type":"join_room","v":1,"player_id":"p2","room_code":"AB23CD"}"#,
            r#"{"type":"move","v":1,"uci":"e2e4"}"#,
            r#"{"type":"resign","v":1}"#,
            r#"{"type":"leave","v":1}"#,
        ] {
            assert!(
                decode_incoming(json).is_ok(),
                "`{json}` decoded before authentication and must still decode"
            );
        }
    }
}
