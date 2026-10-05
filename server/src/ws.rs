//! HTTP health endpoint and the WebSocket connection handler (spec:
//! "Server Health and Configuration", "Online Multiplayer Protocol").
//!
//! This layer only enforces *protocol* rules: malformed JSON, unknown
//! message types, and unsupported versions close the socket with
//! `PROTOCOL_CLOSE_CODE` (4000). Every game-rule decision is made by the
//! room actor, whose answers are relayed to the client.

use std::sync::Arc;

use axum::extract::ws::{CloseCode, CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use axum::Json;

/// Protocol-level close frame: the server must close only for malformed
/// JSON, unknown message types, or unsupported versions.
fn protocol_close() -> Message {
    Message::Close(Some(CloseFrame {
        code: CloseCode::from(PROTOCOL_CLOSE_CODE),
        reason: "protocol error".into(),
    }))
}

use crate::app::{App, Conn};
use crate::protocol::{
    decode_incoming, ClientMessage, ErrorCode, ServerMessage, PROTOCOL_CLOSE_CODE, VERSION,
};
use crate::room::RoomMsg;

/// `GET /healthz`: 200 with a machine-readable body while the server accepts
/// WebSocket connections.
pub async fn healthz() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "protocol_version": VERSION,
    }))
}

pub async fn ws_upgrade(
    ws: WebSocketUpgrade,
    State(app): State<Arc<App>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, app))
}

async fn handle_socket(mut socket: WebSocket, app: Arc<App>) {
    let mut conn: Option<Conn> = None;

    loop {
        tokio::select! {
            out = next_outbound(&mut conn) => match out {
                Some(message) => {
                    if !send_json(&mut socket, &message).await {
                        break;
                    }
                }
                // The room actor ended while we still thought we were in a
                // room: the room is gone, so end this session too.
                None => break,
            },
            frame = socket.recv() => match frame {
                Some(Ok(Message::Text(text))) => {
                    if !handle_text(&mut socket, text.as_str(), &mut conn, &app).await {
                        break;
                    }
                }
                Some(Ok(Message::Ping(payload))) => {
                    if socket.send(Message::Pong(payload)).await.is_err() {
                        break;
                    }
                }
                Some(Ok(Message::Close(_))) | None => break,
                // Binary frames and read errors are protocol failures.
                Some(Ok(_)) | Some(Err(_)) => {
                    let _ = socket.send(protocol_close()).await;
                    break;
                }
            },
        }
    }

    if let Some(conn) = conn.take() {
        // Socket closed: hold the seat for the grace window (the room actor
        // decides whether that means anything).
        let _ = conn
            .mailbox
            .send(RoomMsg::Detach {
                player_id: conn.player_id,
            });
    }
}

/// The room's next outbound message, or `None` when the room channel is
/// closed; pending forever while the connection is not in a room.
async fn next_outbound(conn: &mut Option<Conn>) -> Option<ServerMessage> {
    match conn {
        Some(conn) => conn.out_rx.recv().await,
        None => std::future::pending().await,
    }
}

/// Handle one client text frame. Returns `false` when the connection must
/// end (protocol failure).
async fn handle_text(
    socket: &mut WebSocket,
    text: &str,
    conn: &mut Option<Conn>,
    app: &Arc<App>,
) -> bool {
    let message = match decode_incoming(text) {
        Ok(message) => message,
        Err(_) => {
            let _ = socket.send(protocol_close()).await;
            return false;
        }
    };

    match message {
        ClientMessage::CreateRoom { player_id, .. } => {
            if conn.is_some() {
                let _ = send_json(
                    socket,
                    &ServerMessage::error(ErrorCode::AlreadyInRoom),
                )
                .await;
            } else {
                match app.create_room(&player_id) {
                    Ok(new_conn) => *conn = Some(new_conn),
                    Err(err) => {
                        let _ = send_json(socket, &err).await;
                    }
                }
            }
        }
        ClientMessage::JoinRoom {
            player_id,
            room_code,
            ..
        } => {
            if conn.is_some() {
                let _ = send_json(
                    socket,
                    &ServerMessage::error(ErrorCode::AlreadyInRoom),
                )
                .await;
            } else {
                match app.join_room(&player_id, &room_code) {
                    Ok(new_conn) => *conn = Some(new_conn),
                    Err(err) => {
                        let _ = send_json(socket, &err).await;
                    }
                }
            }
        }
        ClientMessage::Move { uci, .. } => {
            match conn {
                Some(conn) => {
                    let _ = conn
                        .mailbox
                        .send(RoomMsg::Move {
                            player_id: conn.player_id.clone(),
                            uci,
                        });
                }
                None => {
                    let _ = send_json(
                        socket,
                        &ServerMessage::error(ErrorCode::NotConnected),
                    )
                    .await;
                }
            }
        }
        ClientMessage::Resign { .. } => match conn {
            Some(conn) => {
                let _ = conn.mailbox.send(RoomMsg::Resign {
                    player_id: conn.player_id.clone(),
                });
            }
            None => {
                let _ = send_json(
                    socket,
                    &ServerMessage::error(ErrorCode::NotConnected),
                )
                .await;
            }
        },
        ClientMessage::Leave { .. } => {
            if let Some(conn) = conn.take() {
                let _ = conn
                    .mailbox
                    .send(RoomMsg::Leave {
                        player_id: conn.player_id,
                    });
            }
        }
    }
    true
}

async fn send_json(socket: &mut WebSocket, message: &ServerMessage) -> bool {
    match serde_json::to_string(message) {
        Ok(json) => socket.send(Message::Text(json.into())).await.is_ok(),
        Err(_) => true,
    }
}
