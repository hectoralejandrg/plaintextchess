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

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use axum::extract::ws::{CloseCode, CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use axum::Json;
use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::Notify;

use crate::application::ports::{
    Accounts, AuthRecorder, RecorderError, SessionLookup, Sessions,
};
use crate::application::room_actor::RoomMsg;
use crate::domain::account::{
    username_key, validate_display_name, validate_password, validate_username, Account,
    PasswordError, UsernameError,
};
use crate::domain::time_control::TimeControl;
use crate::infrastructure::password::{expires_at, generate_token, hash_token, now_ms};
use crate::infrastructure::server::{App, Conn};
use crate::interface::protocol::{
    decode_incoming, ClientMessage, ErrorCode, ServerMessage, PROTOCOL_CLOSE_CODE, VERSION,
};

/// Protocol-level close frame: the server must close only for malformed
/// JSON, unknown message types, or unsupported versions.
fn protocol_close() -> Message {
    Message::Close(Some(CloseFrame {
        code: CloseCode::from(PROTOCOL_CLOSE_CODE),
        reason: "protocol error".into(),
    }))
}

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

/// How long between server keepalive pings: well under the reconnect grace, so
/// a dead peer is noticed before the window elapses.
const KEEPALIVE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(20);

/// One item queued for the connection's writer.
#[derive(Debug)]
enum Outbound {
    /// A server message (JSON text).
    Text(ServerMessage),
    /// A keepalive ping.
    Ping,
    /// A response to a client ping.
    Pong(Vec<u8>),
    /// A protocol close frame.
    Close,
}

/// Whether a frame is a state-class snapshot/update: these coalesce so a slow
/// client always converges to the newest authoritative state, while one-shot
/// control frames are never dropped.
fn is_state_class(message: &ServerMessage) -> bool {
    matches!(
        message,
        ServerMessage::RoomReady { .. } | ServerMessage::State { .. } | ServerMessage::Update { .. }
    )
}

/// A per-connection outbound buffer with backpressure: at most one coalesced
/// state-class frame plus an ordered queue of control frames, so a client that
/// stops reading cannot grow the server's memory without bound.
#[derive(Default)]
struct Outbox {
    inner: Mutex<VecDeque<Outbound>>,
    closed: AtomicBool,
    notify: Notify,
}

impl Outbox {
    fn push(&self, item: Outbound) {
        let mut queue = self.inner.lock().unwrap();
        if let Outbound::Text(message) = &item {
            if is_state_class(message) {
                queue.retain(|queued| !matches!(queued, Outbound::Text(m) if is_state_class(m)));
            }
        }
        queue.push_back(item);
        drop(queue);
        self.notify.notify_one();
    }

    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.notify.notify_one();
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    fn take_batch(&self) -> Vec<Outbound> {
        let mut queue = self.inner.lock().unwrap();
        queue.drain(..).collect()
    }
}

/// The writer half: drains the outbox and sends keepalive pings, treating a
/// peer that stops responding as gone. Notifies `finished` on the way out so
/// the reader ends the session too.
async fn write_loop(
    mut sink: SplitSink<WebSocket, Message>,
    outbox: Arc<Outbox>,
    alive: Arc<AtomicBool>,
    finished: Arc<Notify>,
) {
    let mut ping = tokio::time::interval_at(
        tokio::time::Instant::now() + KEEPALIVE_INTERVAL,
        KEEPALIVE_INTERVAL,
    );
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    async {
        loop {
            let batch = outbox.take_batch();
            if !batch.is_empty() {
                for item in batch {
                    let sent = match item {
                        Outbound::Text(message) => send_json(&mut sink, &message).await,
                        Outbound::Ping => {
                            sink.send(Message::Ping(Default::default())).await.is_ok()
                        }
                        Outbound::Pong(payload) => {
                            sink.send(Message::Pong(payload.into())).await.is_ok()
                        }
                        Outbound::Close => {
                            let _ = sink.send(protocol_close()).await;
                            return;
                        }
                    };
                    if !sent {
                        return;
                    }
                }
                continue;
            }
            if outbox.is_closed() {
                return;
            }
            tokio::select! {
                _ = outbox.notify.notified() => {}
                _ = ping.tick() => {
                    // No inbound frame since the last ping: the peer is gone.
                    if !alive.swap(false, Ordering::SeqCst) {
                        return;
                    }
                    let _ = sink.send(Message::Ping(Default::default())).await;
                }
            }
        }
    }
    .await;
    finished.notify_one();
}

async fn handle_socket(socket: WebSocket, app: Arc<App>) {
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
            frame = stream.next() => match frame {
                Some(Ok(Message::Text(text))) => {
                    alive.store(true, Ordering::SeqCst);
                    if !handle_text(&outbox, text.as_str(), &mut conn, &app).await {
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

/// Serialize and send one server message over the writer half. `false` means
/// the socket is gone.
async fn send_json(sink: &mut SplitSink<WebSocket, Message>, message: &ServerMessage) -> bool {
    match serde_json::to_string(message) {
        Ok(text) => sink.send(Message::Text(text.into())).await.is_ok(),
        // An unserializable message is a server bug, not a client error: the
        // connection ends rather than silently swallowing the answer.
        Err(_) => false,
    }
}

async fn next_outbound(conn: &mut Option<Conn>) -> Option<ServerMessage> {
    match conn {
        Some(conn) => conn.out_rx.recv().await,
        None => std::future::pending().await,
    }
}

/// Handle one client text frame. Returns `false` when the connection must
/// end (protocol failure).
async fn handle_text(
    outbox: &Outbox,
    text: &str,
    conn: &mut Option<Conn>,
    app: &Arc<App>,
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
    let authed = match &message {
        ClientMessage::Register {
            username,
            password,
            device_id,
            ..
        } => Some(register(app, username, password, device(device_id.as_deref())).await),
        ClientMessage::Login {
            username,
            password,
            device_id,
            ..
        } => Some(login(app, username, password, device(device_id.as_deref())).await),
        ClientMessage::Logout { token, .. } => Some(logout(app, token)),
        ClientMessage::SetProfile {
            display_name, token, ..
        } => Some(set_profile(app, display_name, token).await),
        _ => None,
    };
    if let Some(reply) = authed {
        // A login that succeeds mid-connection is adopted by the next
        // room-affecting action (design D8): the pinned identity is upgraded
        // now, so the account room limit applies and `seat_index_of`
        // upgrades the seat this connection already holds.
        if let ServerMessage::Session { account_id, .. } = &reply {
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
        outbox.push(Outbound::Text(reply));
        return true;
    }

    match message {
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
        ClientMessage::Register { .. }
        | ClientMessage::Login { .. }
        | ClientMessage::Logout { .. }
        | ClientMessage::SetProfile { .. } => {
            // Already answered above; unreachable.
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

fn session_message(account: &Account, token: &str, expires_at_ms: i64) -> ServerMessage {
    ServerMessage::Session {
        v: VERSION,
        account_id: account.account_id.clone(),
        username: account.username.clone(),
        display_name: account.reported_name().to_string(),
        token: token.to_string(),
        expires_at_ms,
    }
}

fn username_error_message(err: UsernameError) -> ServerMessage {
    use crate::domain::account::{MAX_USERNAME_LEN, MIN_USERNAME_LEN};
    let detail = match err {
        UsernameError::Length => {
            format!("username must be {MIN_USERNAME_LEN} to {MAX_USERNAME_LEN} characters")
        }
        UsernameError::Charset => {
            "username may only contain letters, digits, underscore, and hyphen".to_string()
        }
    };
    // The message names the offending field (spec "a validation error naming
    // the offending field"), so a client can point at the right input.
    ErrorCode::InvalidRequest.with_message(detail)
}

fn password_error_message() -> ServerMessage {
    use crate::domain::account::{MAX_PASSWORD_LEN, MIN_PASSWORD_LEN};
    ErrorCode::InvalidRequest
        .with_message(format!("password must be {MIN_PASSWORD_LEN} to {MAX_PASSWORD_LEN} characters"))
}

async fn register(
    app: &Arc<App>,
    raw_username: &str,
    password: &str,
    device_id: Option<String>,
) -> ServerMessage {
    // Validate before hashing, so a rejected input costs nothing (spec
    // "Credential and Session Data Protection").
    let username = match validate_username(raw_username) {
        Ok(username) => username,
        Err(err) => return username_error_message(err),
    };
    if let Err(PasswordError::Length) = validate_password(password) {
        return password_error_message();
    }

    let store = app.auth();
    if store.account_by_username(&username).is_some() {
        // Checked before any hashing, so a taken username cannot be used to
        // make the server spend argon2 time.
        tracing::info!(username = %username, "registration refused: the username is taken");
        return ServerMessage::error(ErrorCode::UsernameTaken);
    }

    let tooling = app.auth_tooling();
    let password_hash = match tooling.hasher.hash(password).await {
        Ok(hash) => hash,
        Err(err) => {
            tracing::error!(error = %err, "password hashing failed during registration");
            return ServerMessage::error(ErrorCode::InvalidRequest);
        }
    };

    let account = Account {
        account_id: uuid::Uuid::new_v4().to_string(),
        username: username.clone(),
        username_key: username_key(&username),
        password_hash,
        display_name: None,
        created_at_ms: now_ms(),
    };
    if let Err(err) = store.insert_account(account.clone()) {
        tracing::info!(username = %username, error = %err, "registration refused: the username is taken");
        return ServerMessage::error(ErrorCode::UsernameTaken);
    }
    let recorder = app.auth_recorder().clone();
    let written = account.clone();
    persist(recorder, account.account_id.clone(), move |recorder| {
        Box::pin(async move { recorder.record_account_created(&written).await })
    })
    .await;

    // A device known at registration time is bound right away (spec
    // "Device-to-Account Profile Linkage"); otherwise the first room message
    // carrying this session's token binds it.
    if let Some(device) = device_id.as_deref() {
        app.link_device_to(device, &account.account_id).await;
    }

    let (token, expires_at_ms) = issue_session(app, &account, device_id.as_deref()).await;
    tracing::info!(
        account_id = %account.account_id,
        username = %account.username,
        "registration succeeded"
    );
    session_message(&account, &token, expires_at_ms)
}

async fn login(
    app: &Arc<App>,
    raw_username: &str,
    password: &str,
    device_id: Option<String>,
) -> ServerMessage {
    // A password outside the accepted length never reaches argon2 (spec
    // "Credential and Session Data Protection").
    if validate_password(password).is_err() {
        // Reported as a credential failure, not a format failure: a login
        // asserts something about existing data, and "too short" would tell an
        // attacker a fact about the account they do not otherwise get.
        tracing::info!("login refused: the password is outside the accepted length");
        return ServerMessage::error(ErrorCode::InvalidCredentials);
    }

    let store = app.auth();
    let Some(account) = store.account_by_username(raw_username) else {
        // Same code and message as a wrong password (spec "Login and Session
        // Issuance"). No password check runs at all here, so neither the body
        // nor the timing of the reply discloses whether the account exists.
        tracing::info!("login refused: no such account");
        return ServerMessage::error(ErrorCode::InvalidCredentials);
    };

    let tooling = app.auth_tooling();
    if !tooling
        .hasher
        .verify(password, &account.password_hash)
        .await
    {
        tracing::info!(account_id = %account.account_id, "login refused: wrong password");
        return ServerMessage::error(ErrorCode::InvalidCredentials);
    }

    // A device that belonged to another account follows this one (spec "Guest
    // Play Fallback", "Device-to-Account Profile Linkage").
    if let Some(device) = device_id.as_deref() {
        app.link_device_to(device, &account.account_id).await;
    }

    let (token, expires_at_ms) = issue_session(app, &account, device_id.as_deref()).await;
    tracing::info!(
        account_id = %account.account_id,
        username = %account.username,
        "login succeeded"
    );
    session_message(&account, &token, expires_at_ms)
}

fn logout(app: &Arc<App>, token: &str) -> ServerMessage {
    let store = app.auth();
    let token_hash = hash_token(token);
    let now_ms = now_ms();

    let Some(session) = store.session_of_token_hash(&token_hash) else {
        tracing::info!("logout presented a token the server has never issued");
        return ServerMessage::error(ErrorCode::NotAuthenticated);
    };
    match store.session_by_token_hash(&token_hash, now_ms) {
        SessionLookup::Usable => {}
        SessionLookup::Revoked => {
            tracing::info!(account_id = %session.account_id, "logout of an already revoked session");
            return ServerMessage::error(ErrorCode::NotAuthenticated);
        }
        SessionLookup::Absent => {
            tracing::info!(account_id = %session.account_id, "logout of an expired session");
            return ServerMessage::error(ErrorCode::SessionExpired);
        }
    }

    // Only the presented session is revoked; the account's other sessions keep
    // working (spec "Session Lifetime, Reuse, and Revocation").
    store.revoke_session(&token_hash, now_ms);
    // The device-to-account link deliberately survives (spec "Logging out
    // keeps the link but ends the session").
    let recorder = app.auth_recorder().clone();
    let hash = token_hash.clone();
    let revoked_for = session.account_id.clone();
    tokio::spawn(async move {
        persist(recorder, revoked_for, move |recorder| {
            Box::pin(async move { recorder.record_session_revoked(&hash, now_ms).await })
        })
        .await;
    });

    let Some(account) = store.account_by_id(&session.account_id) else {
        return ServerMessage::error(ErrorCode::NotAuthenticated);
    };
    tracing::info!(account_id = %account.account_id, "logout succeeded");
    ServerMessage::SessionOk {
        v: VERSION,
        account_id: account.account_id.clone(),
        username: account.username.clone(),
        display_name: account.reported_name().to_string(),
        expires_at_ms: now_ms,
    }
}

async fn set_profile(app: &Arc<App>, raw_display_name: &str, token: &str) -> ServerMessage {
    let store = app.auth();
    let token_hash = hash_token(token);
    let now_ms = now_ms();
    // The same rule as `logout`: a token the server never issued is not a
    // session at all (`not_authenticated`, spec "A guest connection cannot
    // change a display name"), while a token whose lifetime has elapsed is
    // called out as expired rather than as "never signed in" (spec "An
    // expired session stops authenticating").
    let Some(session) = store.session_of_token_hash(&token_hash) else {
        tracing::info!("display-name change presented a token the server has never issued");
        return ServerMessage::error(ErrorCode::NotAuthenticated);
    };
    if session.is_revoked() {
        tracing::info!(account_id = %session.account_id, "display-name change with a revoked session");
        return ServerMessage::error(ErrorCode::NotAuthenticated);
    }
    if session.is_expired(now_ms) {
        tracing::info!(account_id = %session.account_id, "display-name change with an expired session");
        return ServerMessage::error(ErrorCode::SessionExpired);
    }

    // Validated before storing, so a rejected name leaves the stored one
    // untouched (spec "Profile Display Name").
    let display_name = match validate_display_name(raw_display_name) {
        Ok(name) => name,
        Err(err) => {
            tracing::info!(
                account_id = %session.account_id,
                reason = ?err,
                "display name refused: outside the accepted format"
            );
            return ServerMessage::error(ErrorCode::InvalidDisplayName);
        }
    };

    store.set_display_name(&session.account_id, &display_name);
    let recorder = app.auth_recorder().clone();
    let account_id = session.account_id.clone();
    let name = display_name.clone();
    persist(recorder, account_id.clone(), move |recorder| {
        Box::pin(async move { recorder.record_display_name_changed(&account_id, &name).await })
    })
    .await;
    tracing::info!(
        account_id = %session.account_id,
        "display name updated"
    );
    ServerMessage::ProfileUpdated {
        v: VERSION,
        account_id: session.account_id,
        display_name,
    }
}

/// Mint a session, insert it in memory, and mirror it durably (spec "Login and
/// Session Issuance"). Returns the plaintext token, which exists nowhere else.
async fn issue_session(
    app: &Arc<App>,
    account: &Account,
    device_id: Option<&str>,
) -> (String, i64) {
    let store = app.auth();
    let tooling = app.auth_tooling();
    let token = generate_token();
    let issued_at_ms = now_ms();
    let expires_at_ms = expires_at(issued_at_ms, tooling.session_ttl);
    let session = crate::domain::account::Session {
        session_id: uuid::Uuid::new_v4().to_string(),
        account_id: account.account_id.clone(),
        // Empty when the device is not known yet; the link is filled in when
        // the first room message names it.
        device_id: device_id.unwrap_or_default().to_string(),
        token_hash: hash_token(&token),
        created_at_ms: issued_at_ms,
        expires_at_ms,
        revoked_at_ms: None,
    };
    store.insert_session(session.clone());
    let recorder = app.auth_recorder().clone();
    let account_id = account.account_id.clone();
    persist(recorder, account_id, move |recorder| {
        Box::pin(async move { recorder.record_session_issued(&session).await })
    })
    .await;
    (token, expires_at_ms)
}

/// Mirror one write to the durable recorder, logging a failure and moving on
/// (spec "Server Persistence": a failed authentication write MUST NOT block
/// play, and MUST NOT alter the result the player sees).
async fn persist<F, Fut>(recorder: Arc<dyn AuthRecorder>, account_id: String, make: F)
where
    F: FnOnce(Arc<dyn AuthRecorder>) -> Fut,
    Fut: std::future::Future<Output = Result<(), RecorderError>>,
{
    if let Err(err) = make(recorder).await {
        tracing::error!(
            error = %err,
            %account_id,
            "failed to persist an authentication change; it is applied in memory only"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interface::protocol::Status;

    fn update(uci: &str) -> ServerMessage {
        ServerMessage::Update {
            v: VERSION,
            uci: Some(uci.to_string()),
            side_to_move: "b".into(),
            status: Status::Playing,
            white_rating: 1500.0,
            black_rating: 1500.0,
            opponent_online: true,
            white_time_ms: 1_000,
            black_time_ms: 1_000,
        }
    }

    fn a_state() -> ServerMessage {
        ServerMessage::State {
            v: VERSION,
            state: crate::interface::protocol::State {
                board_fen: "x".into(),
                move_list: vec![],
                side_to_move: "w".into(),
                status: Status::Playing,
                your_color: crate::interface::protocol::Color::White,
                white_rating: 1500.0,
                black_rating: 1500.0,
                opponent_online: true,
                time_control: "1+0".into(),
                white_time_ms: 60_000,
                black_time_ms: 60_000,
            },
        }
    }

    /// A slow client that never drains cannot grow memory: the pending
    /// state-class frames collapse to the newest one.
    #[test]
    fn state_frames_coalesce_to_the_newest_with_bounded_memory() {
        let outbox = Outbox::default();
        for i in 0..1_000 {
            outbox.push(Outbound::Text(update(&format!("m{i}"))));
        }
        let batch = outbox.take_batch();
        assert_eq!(batch.len(), 1, "state frames coalesce to a single slot");
        match &batch[0] {
            Outbound::Text(ServerMessage::Update { uci, .. }) => {
                assert_eq!(uci.as_deref(), Some("m999"), "the newest update wins");
            }
            other => panic!("expected the newest update, got {other:?}"),
        }
    }

    /// One-shot control frames are never dropped, even when surrounded by
    /// coalesced state frames.
    #[test]
    fn control_frames_are_never_dropped_and_keep_their_order() {
        let outbox = Outbox::default();
        outbox.push(Outbound::Text(ServerMessage::error(ErrorCode::NotYourTurn)));
        outbox.push(Outbound::Text(a_state()));
        outbox.push(Outbound::Text(update("a")));
        outbox.push(Outbound::Text(ServerMessage::error(ErrorCode::IllegalMove)));
        outbox.push(Outbound::Text(update("b")));

        let batch = outbox.take_batch();
        // The two control frames survive in order; only the newest state-class
        // frame (the "b" update) remains.
        assert_eq!(batch.len(), 3, "two control frames plus the newest state");
        assert!(matches!(
            &batch[0],
            Outbound::Text(ServerMessage::Error { .. })
        ));
        assert!(matches!(
            &batch[1],
            Outbound::Text(ServerMessage::Error { .. })
        ));
        match &batch[2] {
            Outbound::Text(ServerMessage::Update { uci, .. }) => {
                assert_eq!(uci.as_deref(), Some("b"), "the newest state-class frame is last");
            }
            other => panic!("expected the newest update last, got {other:?}"),
        }
    }
}