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

use std::sync::Arc;

use axum::extract::ws::{CloseCode, CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use axum::Json;

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

async fn handle_socket(mut socket: WebSocket, app: Arc<App>) {
    let mut conn: Option<Conn> = None;
    // Set once the room has seated this connection, which is the only thing
    // that can produce a `room_ready` or a state snapshot on the way out.
    let mut seated = false;

    loop {
        tokio::select! {
            out = next_outbound(&mut conn) => match out {
                Some(message) => {
                    if matches!(
                        message,
                        ServerMessage::RoomReady { .. } | ServerMessage::State { .. }
                    ) {
                        seated = true;
                    }
                    if !send_json(&mut socket, &message).await {
                        break;
                    }
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
                identity: conn.identity,
            });
    }
}

/// The room's next outbound message, or `None` when the room channel is
/// closed; pending forever while the connection is not in a room.
/// Serialize and send one message. `false` means the socket is gone.
async fn send_json(socket: &mut WebSocket, message: &ServerMessage) -> bool {
    match serde_json::to_string(message) {
        Ok(text) => socket.send(Message::Text(text.into())).await.is_ok(),
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
        let _ = send_json(socket, &reply).await;
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
                let _ = send_json(
                    socket,
                    &ServerMessage::error(ErrorCode::AlreadyInRoom),
                )
                .await;
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
                        let _ = send_json(socket, &err).await;
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
                let _ = send_json(
                    socket,
                    &ServerMessage::error(ErrorCode::AlreadyInRoom),
                )
                .await;
            } else {
                let identity = app.resolve_identity(&player_id, token.as_deref());
                app.link_identity(&identity);
                match app.join_room(identity, &room_code) {
                    Ok(new_conn) => *conn = Some(new_conn),
                    Err(err) => {
                        let _ = send_json(socket, &err).await;
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
                    identity: conn.identity.clone(),
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
                let _ = conn.mailbox.send(RoomMsg::Leave {
                    identity: conn.identity,
                });
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