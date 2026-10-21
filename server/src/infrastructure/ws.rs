//! HTTP health endpoint and the WebSocket connection handler (spec:
//! "Server Health and Configuration", "Online Multiplayer Protocol",
//! "Authentication Wire Messages").
//!
//! This layer enforces *protocol* rules — malformed JSON, unknown message
//! types, unsupported versions close the socket with `PROTOCOL_CLOSE_CODE`
//! (4000) — and it answers authentication requests. Every game-rule decision
//! is made by the room actor, whose answers are relayed to the client.
//!
//! Auth messages are dispatched in their own arm, before the room dispatch,
//! so an auth failure can never fall through into `create_room`/`join_room`
//! handling (design D11). An auth failure never closes the socket either
//! (spec "Guest Play Fallback").

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use axum::Json;
use futures_util::StreamExt;
use tokio::sync::{mpsc, Notify};

use crate::application::room_actor::RoomMsg;
use crate::domain::time_control::TimeControl;
use crate::infrastructure::server::{App, Conn};
use crate::infrastructure::transport::{is_state_class, write_loop, Outbound, Outbox};
use crate::interface::protocol::{
    decode_incoming, ClientMessage, ErrorCode, ServerMessage, VERSION,
};

pub async fn healthz() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "protocol_version": VERSION,
    }))
}

/// The readiness body with the authentication features this server supports
/// (spec "Server Health and Configuration").
///
/// The names are part of the contract: a client branches on them, so they
/// cannot change once published.
pub async fn healthz_with_features(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "protocol_version": VERSION,
        "auth_features": app.auth_features(),
    }))
}

/// Upgrade a WebSocket connection for the single-tier (`all`) server.
///
/// Keep the buffer sizes in step with the split transport tier: tungstenite
/// defaults to 128 KiB read/write buffers allocated eagerly per connection,
/// which dominates the per-connection memory budget. Frames here are small
/// JSON, so shrink the initial buffers (they grow on demand as needed).
pub async fn ws_upgrade(
    ws: WebSocketUpgrade,
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    peer: PeerAddress,
) -> impl IntoResponse {
    // The forwarding header is a claim, not a fact: it is only believed when
    // the peer is one of the configured trusted proxies, which the App
    // decides (spec "Authentication Rate Limiting").
    let forwarded = headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let peer = peer.0;
    // The address is resolved once, here: it cannot change during the
    // connection, and resolving it per frame would put an allocation in the
    // path of every move.
    let address: Arc<str> = Arc::from(app.client_address(peer, forwarded.as_deref()).as_str());
    ws.read_buffer_size(8 * 1024)
        .write_buffer_size(8 * 1024)
        .max_write_buffer_size(64 * 1024)
        .on_upgrade(move |socket| handle_socket(socket, app, address))
}

/// The address the connection came from, when the server was served with
/// connect info.
#[derive(Debug, Clone, Copy)]
pub struct PeerAddress(pub Option<std::net::SocketAddr>);

impl<S> axum::extract::FromRequestParts<S> for PeerAddress
where
    S: Send + Sync,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        Ok(PeerAddress(
            parts
                .extensions
                .get::<axum::extract::connect_info::ConnectInfo<std::net::SocketAddr>>()
                .map(|connect_info| connect_info.0),
        ))
    }
}

async fn handle_socket(
    socket: WebSocket,
    app: Arc<App>,
    address: Arc<str>,
) {
    // Read and write are separate so a slow write cannot stall inbound framing,
    // and the writer drains a bounded, coalescing outbox.
    let (sink, mut stream) = socket.split();
    let outbox = Arc::new(Outbox::default());
    let alive = Arc::new(AtomicBool::new(true));
    let finished = Arc::new(Notify::new());
    let writer = tokio::spawn(write_loop(
        sink,
        Arc::clone(&outbox),
        Arc::clone(&alive),
        Arc::clone(&finished),
    ));

    let mut conn: Option<Conn> = None;
    // Set once the room has seated this connection, which is the only thing
    // that can produce a `room_ready` or a state snapshot on the way out.
    let mut seated = false;
    // Set while this connection is waiting in a tournament's pool
    // (add-tournaments phase 1): `(tournament_id, player_id)`, so `Leave` and
    // the pairing arm below know which tournament without the client having
    // to resend its own identity.
    let mut tournament_wait: Option<mpsc::UnboundedReceiver<Conn>> = None;
    let mut pending_tournament: Option<(String, String)> = None;

    loop {
        tokio::select! {
            out = next_outbound(&mut conn) => match out {
                Some(message) => {
                    if is_state_class(&message) {
                        seated = true;
                    }
                    outbox.push(Outbound::Text(message));
                }
                None if !seated => {
                    // The room channel closed without ever seating this
                    // connection: the refusal (`room_full`) was its last
                    // frame and the player is still on the socket. A refused
                    // action leaves the connection open (shared spec "Errors
                    // carry a stable code"), so this connection is simply not
                    // in a room until it asks again.
                    conn = None;
                }
                // The room actor ended while we still thought we were in a
                // room: the room is gone, so end this session too.
                None => break,
            },
            // A tournament actor paired this (idle, waiting) connection into
            // a new room (add-tournaments phase 1): no client frame triggers
            // this arm, the server initiated the room itself. Restate the
            // room's own `Connect` reply as `tournament_pairing` so the
            // client doesn't have to infer a bare `room_ready` came from a
            // pairing, then install the `Conn` exactly as `CreateRoom`/
            // `JoinRoom` do.
            paired = next_paired(&mut tournament_wait) => match paired {
                Some(mut new_conn) => {
                    let tournament_id = pending_tournament.take().map(|(id, _)| id).unwrap_or_default();
                    if let Some(ServerMessage::RoomReady { room_code, your_color, state, .. }) =
                        new_conn.out_rx.recv().await
                    {
                        seated = true;
                        outbox.push(Outbound::Text(ServerMessage::TournamentPairing {
                            v: VERSION,
                            tournament_id,
                            room_code,
                            your_color,
                            state,
                        }));
                    }
                    conn = Some(new_conn);
                }
                // The tournament actor dropped the sender without ever
                // pairing us (e.g. the tournament ended while we waited):
                // stop polling this channel instead of spinning on `None`.
                None => tournament_wait = None,
            },
            frame = stream.next() => match frame {
                Some(Ok(Message::Text(text))) => {
                    alive.store(true, Ordering::SeqCst);
                    if !handle_text(
                        &outbox,
                        text.as_str(),
                        &mut conn,
                        &mut tournament_wait,
                        &mut pending_tournament,
                        &app,
                        &address,
                    ).await {
                        break;
                    }
                }
                Some(Ok(Message::Ping(payload))) => {
                    alive.store(true, Ordering::SeqCst);
                    outbox.push(Outbound::Pong(payload.to_vec()));
                }
                Some(Ok(Message::Pong(_))) => {
                    alive.store(true, Ordering::SeqCst);
                }
                Some(Ok(Message::Close(_))) | None => break,
                // Binary frames and read errors are protocol failures.
                Some(Ok(_)) | Some(Err(_)) => {
                    outbox.push(Outbound::Close);
                    break;
                }
            },
            // The writer gave up (keepalive timeout or a dead socket): end too.
            _ = finished.notified() => break,
        }
    }

    outbox.close();
    let _ = writer.await;

    if let Some(conn) = conn.take() {
        // Socket closed: hold the seat for the grace window (the room actor
        // decides whether that means anything).
        let _ = conn
            .mailbox
            .send(RoomMsg::Detach {
                identity: conn.identity,
            });
    }
}

async fn next_outbound(conn: &mut Option<Conn>) -> Option<ServerMessage> {
    match conn {
        Some(conn) => conn.out_rx.recv().await,
        None => std::future::pending().await,
    }
}

/// Add-tournaments phase 1: waits for the tournament actor to pair this
/// connection into a room, or never resolves while not waiting on any
/// tournament (mirrors `next_outbound`'s "no channel yet" shape).
async fn next_paired(rx: &mut Option<mpsc::UnboundedReceiver<Conn>>) -> Option<Conn> {
    match rx {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

/// Handle one client text frame. Returns `false` when the connection must
/// end (protocol failure).
async fn handle_text(
    outbox: &Outbox,
    text: &str,
    conn: &mut Option<Conn>,
    tournament_wait: &mut Option<mpsc::UnboundedReceiver<Conn>>,
    pending_tournament: &mut Option<(String, String)>,
    app: &Arc<App>,
    address: &str,
) -> bool {
    let message = match decode_incoming(text) {
        Ok(message) => message,
        Err(_) => {
            outbox.push(Outbound::Close);
            return false;
        }
    };

    // The device this connection is known by, if a room message has already
    // named one. A `register`/`login` that omits `device_id` falls back to
    // this, so both message orders end up linking the device.
    let pinned_device = conn.as_ref().map(|conn| conn.player_id().to_string());
    let device = |declared: Option<&str>| -> Option<String> {
        declared
            .map(str::to_string)
            .or_else(|| pinned_device.clone())
    };

    // Authentication first (design D11): these arms are handled and return,
    // so an auth failure can never fall through into the room dispatch.
    match message {
        ClientMessage::Register {
            username,
            email,
            password,
            device_id,
            ..
        } => {
            let device = device(device_id.as_deref());
            let client = auth_client(address, device.clone());
            let reply = app
                .register(&username, email.as_deref(), &password, device, &client)
                .await;
            adopt_identity(&reply, conn, app);
            outbox.push(Outbound::Text(reply));
            return true;
        }
        ClientMessage::Login {
            username,
            identifier,
            password,
            device_id,
            ..
        } => {
            // The identifier is what the player typed: a username or an
            // email address (spec "Login and Session Issuance"). The legacy
            // field is the fallback.
            let identifier = identifier
                .clone()
                .or_else(|| username.clone())
                .unwrap_or_default();
            let device = device(device_id.as_deref());
            let client = auth_client(address, device.clone());
            let reply = app.login(&identifier, &password, device, &client).await;
            adopt_identity(&reply, conn, app);
            outbox.push(Outbound::Text(reply));
            return true;
        }
        ClientMessage::Logout { token, .. } => {
            outbox.push(Outbound::Text(app.logout(&token).await));
            return true;
        }
        ClientMessage::SetProfile {
            display_name, token, ..
        } => {
            outbox.push(Outbound::Text(app.set_profile(&display_name, &token).await));
            return true;
        }
        ClientMessage::ResumeSession { token, .. } => {
            let reply = app.resume_session(&token);
            // A resumed session is the account the connection belongs to, so
            // the one-room-per-account rule applies to it from now on (spec
            // "Device-to-Account Profile Linkage").
            adopt_identity(&reply, conn, app);
            outbox.push(Outbound::Text(reply));
            return true;
        }
        ClientMessage::ForgotPassword { email, .. } => {
            let client = auth_client(address, None);
            outbox.push(Outbound::Text(app.forgot_password(&email, &client).await));
            return true;
        }
        ClientMessage::ResetPassword { token, password, .. } => {
            outbox.push(Outbound::Text(app.reset_password(&token, &password).await));
            return true;
        }
        ClientMessage::VerifyEmail { token, .. } => {
            outbox.push(Outbound::Text(app.verify_email(&token).await));
            return true;
        }
        ClientMessage::ResendVerification { token, .. } => {
            let client = auth_client(address, None);
            outbox.push(Outbound::Text(app.resend_verification(&token, &client).await));
            return true;
        }
        ClientMessage::ChangeEmail { token, email, .. } => {
            let client = auth_client(address, None);
            outbox.push(Outbound::Text(app.change_email(&token, &email, &client).await));
            return true;
        }
        ClientMessage::CreateRoom {
            player_id,
            time_control,
            token,
            ..
        } => {
            if conn.is_some() {
                outbox.push(Outbound::Text(ServerMessage::error(ErrorCode::AlreadyInRoom)));
            } else {
                // An absent or unrecognized control falls back to the default
                // (design D5), so older clients keep working against this
                // server.
                let time_control = time_control
                    .as_deref()
                    .and_then(TimeControl::parse)
                    .unwrap_or(TimeControl::DEFAULT);
                let identity = app.resolve_identity(&player_id, token.as_deref());
                // The first time a device is seen alongside an account, bind
                // them (spec "Device-to-Account Profile Linkage").
                app.link_identity(&identity);
                match app.create_room_with_time_control(identity, time_control) {
                    Ok(new_conn) => *conn = Some(new_conn),
                    Err(err) => {
                        outbox.push(Outbound::Text(err));
                    }
                }
            }
        }
        ClientMessage::JoinRoom {
            player_id,
            room_code,
            token,
            ..
        } => {
            if conn.is_some() {
                outbox.push(Outbound::Text(ServerMessage::error(ErrorCode::AlreadyInRoom)));
            } else {
                let identity = app.resolve_identity(&player_id, token.as_deref());
                app.link_identity(&identity);
                match app.join_room(identity, &room_code) {
                    Ok(new_conn) => *conn = Some(new_conn),
                    Err(err) => {
                        outbox.push(Outbound::Text(err));
                    }
                }
            }
        }
        ClientMessage::Move { uci, .. } => {
            // The connection's *pinned* identity, not a re-resolution: a token
            // that expired mid-game must not change who is playing (design D8).
            match conn {
                Some(conn) => {
                    let _ = conn.mailbox.send(RoomMsg::Move {
                        identity: conn.identity.clone(),
                        uci,
                    });
                }
                None => {
                    outbox.push(Outbound::Text(ServerMessage::error(ErrorCode::NotConnected)));
                }
            }
        }
        ClientMessage::Resign { .. } => match conn {
            Some(conn) => {
                let _ = conn.mailbox.send(RoomMsg::Resign {
                    identity: conn.identity.clone(),
                });
            }
            None => {
                outbox.push(Outbound::Text(ServerMessage::error(ErrorCode::NotConnected)));
            }
        },
        ClientMessage::Leave { .. } => {
            if let Some(conn) = conn.take() {
                // Wait for the actor to confirm the leave. It releases the seat
                // and unbinds the account/device before it signals, so a
                // CreateRoom/JoinRoom sent right after on this same connection
                // cannot observe the stale room and be refused with
                // `already_in_room` (design D8/D10).
                let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
                let _ = conn.mailbox.send(RoomMsg::Leave {
                    identity: conn.identity,
                    ack: Some(ack_tx),
                });
                // A closed mailbox (room already gone) drops the sender, so this
                // resolves immediately rather than hanging.
                let _ = ack_rx.await;
            }
        }
        ClientMessage::JoinTournament {
            player_id,
            tournament_id,
            token,
            ..
        } => {
            if conn.is_some() || tournament_wait.is_some() {
                outbox.push(Outbound::Text(ServerMessage::error(ErrorCode::AlreadyInTournament)));
            } else {
                let identity = app.resolve_identity(&player_id, token.as_deref());
                app.link_identity(&identity);
                let (paired_tx, paired_rx) = mpsc::unbounded_channel();
                match app.join_tournament(identity, &tournament_id, paired_tx) {
                    Ok(()) => {
                        *tournament_wait = Some(paired_rx);
                        *pending_tournament = Some((tournament_id.clone(), player_id));
                        outbox.push(Outbound::Text(ServerMessage::TournamentJoined {
                            v: VERSION,
                            tournament_id,
                        }));
                    }
                    Err(err) => outbox.push(Outbound::Text(err)),
                }
            }
        }
        ClientMessage::LeaveTournament { tournament_id, .. } => {
            *tournament_wait = None;
            if let Some((_, player_id)) = pending_tournament.take() {
                app.leave_tournament(&player_id, &tournament_id);
            }
        }
        ClientMessage::ListTournaments { .. } => {
            outbox.push(Outbound::Text(ServerMessage::TournamentList {
                v: VERSION,
                tournaments: app.list_tournaments(),
            }));
        }
    }
    true
}

// -------------------------------------------------------------------------
// Authentication (spec "player-auth")
//
// Every handler answers with a `ServerMessage` and never closes the socket.
// No log line here carries a password or a token: only the outcome and the
// account or device involved (spec "Credential and Session Data Protection").
// -------------------------------------------------------------------------

/// Adopt the account a successful sign-in resolved (design D8).
///
/// A login that succeeds mid-connection is adopted by the next
/// room-affecting action: the pinned identity is upgraded now, so the account
/// room limit applies and the seat this connection already holds is
/// upgraded.
fn adopt_identity(reply: &ServerMessage, conn: &mut Option<Conn>, app: &Arc<App>) {
    if let ServerMessage::Session { account_id, .. } = reply {
        if let Some(conn) = conn.as_mut() {
            let account_id = account_id.clone();
            let code = conn.code.clone();
            conn.identity = conn.identity.authenticated_as(&account_id);
            if !code.is_empty() {
                // The room was tracked under the guest identity; record it
                // under the account too, so the one-room-per-account rule
                // holds from this moment (design D8/D10).
                app.adopt_identity(&conn.identity, &code);
            }
        }
    }
}

/// The rate-limit key for an attempt on this connection (spec
/// "Authentication Rate Limiting").
///
/// The address was resolved when the connection was upgraded, so nothing is
/// parsed per attempt.
fn auth_client(
    address: &str,
    device_id: Option<String>,
) -> crate::infrastructure::server::AuthClient {
    crate::infrastructure::server::AuthClient {
        address: address.to_string(),
        device_id,
    }
}

