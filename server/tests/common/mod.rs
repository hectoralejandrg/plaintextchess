//! Shared harness for the room-level tests: drives the room actors through
//! `App` + mpsc channels (no sockets), so registry, authority, rating, and
//! disconnect behavior can be asserted deterministically.

#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

pub use chess_server::app::{App, Conn};
use chess_server::config::Config;
use chess_server::protocol::{ServerMessage, State};
use tokio::sync::mpsc;

pub const P_A: &str = "device-a";
pub const P_B: &str = "device-b";
pub const P_C: &str = "device-c";

pub fn app_with_grace(grace_secs: u64) -> Arc<App> {
    let config = Config::for_test(
        "127.0.0.1".parse().unwrap(),
        0,
        Duration::from_secs(grace_secs),
    );
    Arc::new(App::new(config))
}

#[allow(clippy::result_large_err)]
pub fn create(app: &Arc<App>, player_id: &str) -> Conn {
    app.create_room(player_id).expect("create room")
}

#[allow(clippy::result_large_err)]
pub fn join(app: &Arc<App>, player_id: &str, code: &str) -> Result<Conn, ServerMessage> {
    app.join_room(player_id, code)
}

pub async fn next_message(rx: &mut mpsc::UnboundedReceiver<ServerMessage>) -> ServerMessage {
    rx.recv()
        .await
        .expect("outbound channel closed while a message was expected")
}

/// The next message that carries a state snapshot (skips errors by failing
/// loudly, so an unexpected error is never swallowed).
pub async fn next_state(rx: &mut mpsc::UnboundedReceiver<ServerMessage>) -> State {
    match next_message(rx).await {
        ServerMessage::RoomReady { state, .. } | ServerMessage::State { state, .. } => state,
        ServerMessage::Error { code, .. } => panic!("expected a snapshot, got error `{code}`"),
    }
}

pub fn state_of(message: &ServerMessage) -> &State {
    match message {
        ServerMessage::RoomReady { state, .. } | ServerMessage::State { state, .. } => state,
        ServerMessage::Error { .. } => panic!("expected a snapshot, got {message:?}"),
    }
}

pub fn error_code(message: &ServerMessage) -> String {
    match message {
        ServerMessage::Error { code, .. } => code.clone(),
        other => panic!("expected an error, got {other:?}"),
    }
}

/// Awaits a snapshot whose move list has `expected_moves` entries.
pub async fn next_state_with_moves(
    rx: &mut mpsc::UnboundedReceiver<ServerMessage>,
    expected_moves: usize,
) -> State {
    loop {
        let state = next_state(rx).await;
        if state.move_list.len() == expected_moves {
            return state;
        }
    }
}

/// Asserts that no outbound message arrives within `window` (used to prove
/// that rejected actions send nothing).
pub async fn no_message_within(
    rx: &mut mpsc::UnboundedReceiver<ServerMessage>,
    window: Duration,
) {
    match tokio::time::timeout(window, rx.recv()).await {
        Ok(Some(message)) => panic!("expected silence, got {message:?}"),
        Ok(None) => panic!("outbound channel closed unexpectedly"),
        Err(_) => {}
    }
}
