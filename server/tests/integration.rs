//! Real-socket integration tests (task 1.7): spawn the axum server on
//! 127.0.0.1:0, drive the full protocol over WebSocket with two
//! tokio-tungstenite clients, and cover the shared "Online Multiplayer
//! Protocol" scenarios end to end.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use chess_server::app::App;
use chess_server::config::Config;
use chess_server::protocol::{ClientMessage, Color, ServerMessage, State, Status};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::{Message, Utf8Bytes};
use tokio_tungstenite::WebSocketStream;

type WsClient = WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn spawn_server(grace_secs: u64) -> (SocketAddr, Arc<App>) {
    let config = Config::for_test(
        "127.0.0.1".parse().unwrap(),
        0,
        Duration::from_secs(grace_secs),
    );
    let app = Arc::new(App::new(config));
    let router = axum::Router::new()
        .route("/healthz", axum::routing::get(chess_server::ws::healthz))
        .route("/ws", axum::routing::get(chess_server::ws::ws_upgrade))
        .with_state(Arc::clone(&app));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test server");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        axum::serve(listener, router).await.expect("serve");
    });
    (addr, app)
}

async fn connect(addr: SocketAddr) -> WsClient {
    let url = format!("ws://{addr}/ws");
    let (stream, _response) = tokio_tungstenite::connect_async(url)
        .await
        .expect("connect to /ws");
    stream
}

fn client_message(json: &str) -> Message {
    Message::Text(Utf8Bytes::from(json.to_string()))
}

fn action(message: &ClientMessage) -> Message {
    let json = serde_json::to_string(message).expect("serialize");
    client_message(&json)
}

/// The next Text or Close frame (skips control frames like Pong).
async fn next_message(client: &mut WsClient) -> Message {
    loop {
        match client.next().await {
            Some(Ok(message)) if matches!(message, Message::Text(_) | Message::Close(_)) => {
                return message
            }
            Some(Ok(_)) => continue,
            Some(Err(err)) => panic!("socket read error: {err}"),
            None => panic!("server closed the socket unexpectedly"),
        }
    }
}

async fn next_server_message(client: &mut WsClient) -> ServerMessage {
    match next_message(client).await {
        Message::Text(text) => serde_json::from_str(text.as_str()).expect("parse server message"),
        Message::Close(frame) => panic!("server closed with {frame:?}"),
        other => panic!("unexpected frame {other:?}"),
    }
}

async fn next_state(client: &mut WsClient) -> State {
    match next_server_message(client).await {
        ServerMessage::RoomReady { state, .. } | ServerMessage::State { state, .. } => state,
        ServerMessage::Error { code, .. } => panic!("expected a snapshot, got error `{code}`"),
    }
}

async fn next_error_code(client: &mut WsClient) -> String {
    match next_server_message(client).await {
        ServerMessage::Error { code, .. } => code,
        other => panic!("expected an error, got {other:?}"),
    }
}

async fn create_room(client: &mut WsClient, player_id: &str) -> String {
    client
        .send(action(&ClientMessage::CreateRoom {
            v: 1,
            player_id: player_id.into(),
        }))
        .await
        .expect("send create");
    match next_server_message(client).await {
        ServerMessage::RoomReady { room_code, .. } => room_code,
        other => panic!("expected room_ready, got {other:?}"),
    }
}

async fn join_room(client: &mut WsClient, player_id: &str, code: String) {
    client
        .send(action(&ClientMessage::JoinRoom {
            v: 1,
            player_id: player_id.into(),
            room_code: code,
        }))
        .await
        .expect("send join");
}

async fn send_move(client: &mut WsClient, uci: &str) {
    client
        .send(action(&ClientMessage::Move {
            v: 1,
            uci: uci.into(),
        }))
        .await
        .expect("send move");
}

// ---------------------------------------------------------------------------
// Health endpoint
// ---------------------------------------------------------------------------

#[tokio::test]
async fn healthz_reports_readiness() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (addr, _app) = spawn_server(30).await;
    let mut stream = tokio::net::TcpStream::connect(addr).await.expect("tcp connect");
    stream
        .write_all(
            b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .await
        .expect("write request");
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.expect("read response");
    let text = String::from_utf8(response).expect("utf8 response");
    assert!(text.starts_with("HTTP/1.1 200"), "healthz must answer 200: {text}");
    assert!(text.contains("\"status\":\"ok\""));
}

// ---------------------------------------------------------------------------
// Full game over the wire (versioned messages, snapshots, ratings)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn full_game_over_sockets_with_ratings_in_final_snapshot() {
    let (addr, _app) = spawn_server(30).await;
    let mut a = connect(addr).await;
    let mut b = connect(addr).await;

    let code = create_room(&mut a, "device-a").await;
    join_room(&mut b, "device-b", code).await;
    let state_b = next_state(&mut b).await;
    assert_eq!(state_b.your_color, Color::Black);
    assert!(state_b.opponent_online);
    let state_a = next_state(&mut a).await;
    assert!(state_a.opponent_online);

    // Scholar's Mate: 1. e4 e5 2. Qh5 Nc6 3. Bc4 Nf6 4. Qxf7#
    let line = [
        ("a", "e2e4"),
        ("b", "e7e5"),
        ("a", "d1h5"),
        ("b", "b8c6"),
        ("a", "f1c4"),
        ("b", "g8f6"),
        ("a", "h5f7"),
    ];
    let mut last_a: Option<State> = None;
    let mut last_b: Option<State> = None;
    for (who, uci) in line {
        match who {
            "a" => send_move(&mut a, uci).await,
            _ => send_move(&mut b, uci).await,
        }
        last_a = Some(next_state(&mut a).await);
        last_b = Some(next_state(&mut b).await);
    }
    let state_a = last_a.expect("final snapshot");
    let state_b = last_b.expect("final snapshot");
    assert!(
        matches!(state_a.status, Status::Checkmated { .. }),
        "the final snapshot must report checkmate: {:?}",
        state_a.status
    );
    assert_eq!(state_a.status, state_b.status, "snapshots must agree");
    assert!(state_a.white_rating > 1500.0, "winner rating in final snapshot");
    assert!(state_a.black_rating < 1500.0, "loser rating in final snapshot");
    assert_eq!(state_a.move_list.len(), 7);

    // The connection stays open after the game ends; further moves are
    // answered with the stable game_over code.
    send_move(&mut a, "a2a4").await;
    assert_eq!(next_error_code(&mut a).await, "game_over");
}

// ---------------------------------------------------------------------------
// Structured errors keep the connection open
// ---------------------------------------------------------------------------

#[tokio::test]
async fn out_of_turn_move_is_rejected_and_connection_survives() {
    let (addr, _app) = spawn_server(30).await;
    let mut a = connect(addr).await;
    let mut b = connect(addr).await;

    let code = create_room(&mut a, "device-a").await;
    join_room(&mut b, "device-b", code).await;
    let _ = next_state(&mut b).await;
    let _ = next_state(&mut a).await;

    // Black moves first: rejected with the stable code, socket stays open.
    send_move(&mut b, "e7e5").await;
    assert_eq!(next_error_code(&mut b).await, "not_your_turn");

    // Both rooms are still fully functional.
    send_move(&mut a, "e2e4").await;
    let state_a = next_state(&mut a).await;
    assert_eq!(state_a.move_list, vec!["e2e4".to_string()]);
    let state_b = next_state(&mut b).await;
    assert_eq!(state_b.move_list, state_a.move_list);

    // Unknown room codes are answered (not closed) with room_not_found.
    let mut c = connect(addr).await;
    join_room(&mut c, "device-c", "ZZZZZZ".into()).await;
    assert_eq!(next_error_code(&mut c).await, "room_not_found");
}

// ---------------------------------------------------------------------------
// Protocol failures close with 4000
// ---------------------------------------------------------------------------

#[tokio::test]
async fn malformed_json_closes_the_connection_with_4000() {
    let (addr, _app) = spawn_server(30).await;
    let mut client = connect(addr).await;
    client.send(client_message("this is not json")).await.expect("send garbage");
    match next_message(&mut client).await {
        Message::Close(Some(frame)) => {
            assert_eq!(u16::from(frame.code), 4000, "protocol close code");
        }
        other => panic!("expected a close frame, got {other:?}"),
    }
}

#[tokio::test]
async fn unknown_message_type_closes_the_connection_with_4000() {
    let (addr, _app) = spawn_server(30).await;
    let mut client = connect(addr).await;
    client
        .send(client_message(r#"{"type":"teleport","v":1}"#))
        .await
        .expect("send unknown type");
    match next_message(&mut client).await {
        Message::Close(Some(frame)) => {
            assert_eq!(u16::from(frame.code), 4000, "protocol close code");
        }
        other => panic!("expected a close frame, got {other:?}"),
    }
}

#[tokio::test]
async fn unsupported_version_closes_the_connection_with_4000() {
    let (addr, _app) = spawn_server(30).await;
    let mut client = connect(addr).await;
    client
        .send(client_message(r#"{"type":"move","v":2,"uci":"e2e4"}"#))
        .await
        .expect("send wrong version");
    match next_message(&mut client).await {
        Message::Close(Some(frame)) => {
            assert_eq!(u16::from(frame.code), 4000, "protocol close code");
        }
        other => panic!("expected a close frame, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Reconnect resyncs with a fresh snapshot (over the wire)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn reconnect_resyncs_with_a_fresh_snapshot() {
    let (addr, _app) = spawn_server(30).await;
    let mut a = connect(addr).await;
    let mut b = connect(addr).await;

    let code = create_room(&mut a, "device-a").await;
    join_room(&mut b, "device-b", code.clone()).await;
    let _ = next_state(&mut b).await;
    let _ = next_state(&mut a).await;

    send_move(&mut a, "e2e4").await;
    let _ = next_state(&mut a).await;
    let _ = next_state(&mut b).await;

    // A drops; B is told the opponent is offline.
    drop(a);
    let state_b = next_state(&mut b).await;
    assert!(!state_b.opponent_online, "B must be told A dropped");

    // Same device identifier re-attaches within the grace window.
    let mut a2 = connect(addr).await;
    join_room(&mut a2, "device-a", code).await;
    let state_a2 = next_state(&mut a2).await;
    assert_eq!(state_a2.move_list, vec!["e2e4".to_string()]);
    assert_eq!(state_a2.your_color, Color::White);
    assert!(state_a2.opponent_online);
    let state_b = next_state(&mut b).await;
    assert!(state_b.opponent_online, "the re-attached player is back online");

    // The game continues from the server state.
    send_move(&mut b, "e7e5").await;
    let state_a2 = next_state(&mut a2).await;
    assert_eq!(
        state_a2.move_list,
        vec!["e2e4".to_string(), "e7e5".to_string()],
        "the move list is intact after resync"
    );
}
