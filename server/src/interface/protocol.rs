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
        }
    }

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
    },
    JoinRoom {
        v: u32,
        player_id: String,
        room_code: String,
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
}

impl ClientMessage {
    pub fn v(&self) -> u32 {
        match self {
            ClientMessage::CreateRoom { v, .. }
            | ClientMessage::JoinRoom { v, .. }
            | ClientMessage::Move { v, .. }
            | ClientMessage::Resign { v }
            | ClientMessage::Leave { v } => *v,
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
    /// Structured business-rule error (connection stays open).
    Error {
        v: u32,
        code: String,
        message: String,
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
            },
            ClientMessage::CreateRoom {
                v: VERSION,
                player_id: "p2".into(),
                time_control: None,
            },
            ClientMessage::JoinRoom {
                v: VERSION,
                player_id: "p2".into(),
                room_code: "AB23CD".into(),
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
            ServerMessage::error(ErrorCode::NotYourTurn),
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
            })
            .unwrap();
        assert!(value.get("time_control").is_none());

        let value: serde_json::Value =
            serde_json::to_value(&ClientMessage::JoinRoom {
                v: VERSION,
                player_id: "p2".into(),
                room_code: "AB23CD".into(),
            })
            .unwrap();
        assert_eq!(value["type"], "join_room");
        assert_eq!(value["room_code"], "AB23CD");

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

        let value: serde_json::Value =
            serde_json::to_value(ServerMessage::error(ErrorCode::RoomFull)).unwrap();
        assert_eq!(value["type"], "error");
        assert_eq!(value["code"], "room_full");
        assert!(value["message"].is_string());
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
        ] {
            let message = ServerMessage::error(code);
            let json = serde_json::to_string(&message).unwrap();
            assert!(json.contains(format!(r#""code":"{}""#, code.as_str()).as_str()));
        }
    }
}
