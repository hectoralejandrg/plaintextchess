//! A long-running second online client for local end-to-end checks
//! (add-online-multiplayer D11): it creates (or joins) a room, prints the
//! room code, answers its own turns with scripted moves, and logs every
//! snapshot, error, and terminal status it receives.
//!
//! Environment:
//!   BUDDY_URL     server URL (default ws://127.0.0.1:8765/ws)
//!   BUDDY_ID      device id (default "buddy")
//!   BUDDY_MODE    "create" (default) or "join"
//!   BUDDY_ROOM    room code (required when BUDDY_MODE=join)
//!   BUDDY_MOVES   space-separated UCI replies for the buddy's own turns
//!   BUDDY_TIME_CONTROL  time control to create with (e.g. "3+2"; default 15+10)
//!
//! Usage:
//!   BUDDY_MOVES="d1h5 g8f6" cargo run --manifest-path server/Cargo.toml \
//!     --example online_buddy

use std::env;

use chess_server::interface::protocol::{ClientMessage, Color, ServerMessage, Status};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::{Message, Utf8Bytes};
use tokio_tungstenite::WebSocketStream;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run())
}

#[allow(clippy::print_stderr)]
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let url = env::var("BUDDY_URL").unwrap_or_else(|_| "ws://127.0.0.1:8765/ws".into());
    let device = env::var("BUDDY_ID").unwrap_or_else(|_| "buddy".into());
    let mode = env::var("BUDDY_MODE").unwrap_or_else(|_| "create".into());
    let room = env::var("BUDDY_ROOM").ok();
    let reply_moves: Vec<String> = env::var("BUDDY_MOVES")
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_string)
        .collect();
    let time_control = env::var("BUDDY_TIME_CONTROL").ok();

    let first = if mode == "create" {
        ClientMessage::CreateRoom {
            v: 1,
            player_id: device.clone(),
            time_control: time_control.clone(),
            // A guest run: no token, so no account and no session (spec
            // "Guest Play Fallback"). `BUDDY_TOKEN` signs the buddy in instead.
            token: env::var("BUDDY_TOKEN").ok(),
        }
    } else if mode == "join" {
        let code = room.clone().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "BUDDY_MODE=join needs BUDDY_ROOM=<code>",
            )
        })?;
        ClientMessage::JoinRoom {
            v: 1,
            player_id: device.clone(),
            room_code: code,
            token: env::var("BUDDY_TOKEN").ok(),
        }
    } else {
        return Err(format!("buddy: unknown BUDDY_MODE `{mode}'").into());
    };

    let mut socket: WebSocketStream<_> =
        tokio_tungstenite::connect_async(&url)
            .await
            .map_err(|error| format!("buddy: connect to {url} failed: {error}"))?
            .0;
    socket
        .send(Message::Text(Utf8Bytes::from(
            serde_json::to_string(&first)?,
        )))
        .await
        .map_err(|error| format!("buddy: send failed: {error}"))?;

    let mut my_color: Option<Color> = None;
    let mut reply_index = 0;
    let mut move_count = 0;

    while let Some(frame) = socket.next().await {
        let frame = frame.map_err(|error| format!("buddy: socket: {error}"))?;
        let server: ServerMessage = match frame {
            Message::Text(text) => serde_json::from_str(text.as_str())
                .map_err(|error| format!("buddy: parse: {error}"))?,
            Message::Close(frame) => {
                println!("buddy: server closed socket: {frame:?}");
                break;
            }
            _ => continue,
        };

        match server {
            ServerMessage::RoomReady {
                room_code,
                your_color,
                state,
                ..
            } => {
                my_color = Some(your_color);
                println!("buddy: ready code={room_code} color={your_color:?}");
                handle_state(
                    &state, &my_color, &reply_moves, &mut reply_index, &mut move_count, &mut socket,
                ).await?;
            }
            ServerMessage::State { state, .. } => {
                // A re-attach (same device id) is answered with a `state`
                // resync only, so pick up the seat color from it when this
                // process has not seen a `room_ready` yet.
                if my_color.is_none() {
                    my_color = Some(state.your_color);
                    println!(
                        "buddy: re-attached as {:?} ({} move(s))",
                        state.your_color,
                        state.move_list.len()
                    );
                }
                handle_state(
                    &state, &my_color, &reply_moves, &mut reply_index, &mut move_count, &mut socket,
                ).await?;
            }
            ServerMessage::Error {
                code,
                message,
                ..
            } => {
                println!("buddy: error {code}: {message}");
            }
            // Authentication answers, which a guest run never receives. The
            // token is deliberately not printed: it is a credential.
            ServerMessage::Session {
                account_id,
                username,
                display_name,
                expires_at_ms,
                ..
            } => {
                println!(
                    "buddy: signed in as {username} ({display_name}, account {account_id}), session expires at {expires_at_ms}"
                );
            }
            ServerMessage::SessionOk {
                account_id,
                username,
                ..
            } => {
                println!("buddy: signed out of {username} ({account_id})");
            }
            ServerMessage::ProfileUpdated {
                account_id,
                display_name,
                ..
            } => {
                println!("buddy: {account_id} is now displayed as {display_name}");
            }
        }
    }
    println!("buddy: done");
    Ok(())
}

/// Log the snapshot; if it is the buddy's turn, send the next scripted move.
async fn handle_state(
    state: &chess_server::interface::protocol::State,
    my_color: &Option<Color>,
    reply_moves: &[String],
    reply_index: &mut usize,
    move_count: &mut usize,
    socket: &mut WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> Result<(), Box<dyn std::error::Error>> {
    if state.move_list.len() > *move_count {
        let new_moves = &state.move_list[*move_count..];
        println!("buddy: saw move(s): {}", new_moves.join(" "));
        *move_count = state.move_list.len();
    }
    match &state.status {
        Status::Playing => {
            if !state.opponent_online {
                println!("buddy: opponent went offline");
            }
            let my_turn = matches!(
                (*my_color, state.side_to_move.as_str()),
                (Some(Color::White), "w") | (Some(Color::Black), "b")
            );
            // Only answer once the game has started (both seats filled): the
            // server rejects lobby moves with `not_connected`.
            if my_turn && state.opponent_online && *reply_index < reply_moves.len() {
                let uci = reply_moves[*reply_index].clone();
                *reply_index += 1;
                println!("buddy: playing {uci}");
                let message = ClientMessage::Move { v: 1, uci };
                socket
                    .send(Message::Text(Utf8Bytes::from(
                        serde_json::to_string(&message)?,
                    )))
                    .await
                    .map_err(|error| format!("buddy: send failed: {error}"))?;
            }
        }
        other => {
            println!(
                "buddy: terminal: {other:?} ({} moves)",
                state.move_list.len()
            );
        }
    }
    Ok(())
}
