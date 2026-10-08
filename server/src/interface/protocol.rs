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
    /// (spec "Player Account Registration", "Profile Display Name").
    InvalidRequest,
    /// The display name is outside 1 to 32 trimmed characters or contains a
    /// control character (spec "Profile Display Name").
    InvalidDisplayName,
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
            ErrorCode::InvalidCredentials => "invalid_credentials",
            ErrorCode::NotAuthenticated => "not_authenticated",
            ErrorCode::SessionExpired => "session_expired",
            ErrorCode::InvalidRequest => "invalid_request",
            ErrorCode::InvalidDisplayName => "invalid_display_name",
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
            ErrorCode::InvalidCredentials => Self::GENERIC_CREDENTIAL_MESSAGE,
            ErrorCode::NotAuthenticated => "This connection is not signed in",
            ErrorCode::SessionExpired => "This session has expired; log in again",
            ErrorCode::InvalidRequest => "A field failed validation",
            ErrorCode::InvalidDisplayName => {
                "A display name must be 1 to 32 characters with no control characters"
            }
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
        password: String,
        /// The device to bind to the new account (spec "Device-to-Account
        /// Profile Linkage"). Optional: absent, the device is bound by the
        /// first `create_room`/`join_room` that carries this connection's
        /// token, so a client that authenticates before choosing a device id
        /// still ends up linked.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        device_id: Option<String>,
    },
    /// Exchange a username and password for a session (spec "Login and
    /// Session Issuance").
    Login {
        v: u32,
        username: String,
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
            | ClientMessage::SetProfile { v, .. } => *v,
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
    /// The account's new display name (spec "Profile Display Name").
    ProfileUpdated {
        v: u32,
        account_id: String,
        display_name: String,
    },
}

impl ServerMessage {
    pub fn error(code: ErrorCode) -> Self {
        ServerMessage::Error {
            v: VERSION,
            code: code.as_str().to_string(),
            message: code.message().to_string(),
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
                password: "correct horse battery".into(),
                device_id: Some("device-a".into()),
            },
            ClientMessage::Register {
                v: VERSION,
                username: "Bob".into(),
                password: "correct horse battery".into(),
                device_id: None,
            },
            ClientMessage::Login {
                v: VERSION,
                username: "Ana".into(),
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
                username: "Ana".into(),
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

        // Authentication messages: `type` tag + `v` + fields, inline.
        let value: serde_json::Value =
            serde_json::to_value(&ClientMessage::Register {
                v: VERSION,
                username: "Ana".into(),
                password: "correct horse battery".into(),
                device_id: Some("device-a".into()),
            })
            .unwrap();
        assert_eq!(value["type"], "register");
        assert_eq!(value["username"], "Ana");
        assert_eq!(value["password"], "correct horse battery");
        assert_eq!(value["device_id"], "device-a");

        let value: serde_json::Value =
            serde_json::to_value(&ClientMessage::Register {
                v: VERSION,
                username: "Ana".into(),
                password: "correct horse battery".into(),
                device_id: None,
            })
            .unwrap();
        assert!(
            value.get("device_id").is_none(),
            "the device is optional: a client that has not chosen one omits it"
        );

        let value: serde_json::Value =
            serde_json::to_value(&ClientMessage::Login {
                v: VERSION,
                username: "Ana".into(),
                password: "correct horse battery".into(),
                device_id: Some("device-a".into()),
            })
            .unwrap();
        assert_eq!(value["type"], "login");
        assert_eq!(value["username"], "Ana");
        assert_eq!(value["device_id"], "device-a");

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
