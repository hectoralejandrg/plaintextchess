//! Authentication integration tests over real sockets (tasks 8.1-8.5, 7.2,
//! 7.3, 9.3): register, log in, log out, set a display name, re-attach across
//! devices, and prove that a rejected authentication request never costs the
//! player their game.
//!
//! Every test runs the production constructor (`App::init`) against a
//! throwaway SQLite file, so the durable side is exercised too: the password
//! column, the device links, and the session rows are read back from disk
//! rather than from the in-memory store.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use chess_server::infrastructure::config::{
    Argon2Params, Config, MIN_ARGON2_MEMORY_KIB, MIN_ARGON2_PARALLELISM, MIN_ARGON2_TIME_COST,
};
use chess_server::infrastructure::server::App;
use chess_server::interface::protocol::{
    ClientMessage, Color, ServerMessage, State, Status, VERSION,
};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::{Message, Utf8Bytes};
use tokio_tungstenite::WebSocketStream;

type WsClient = WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// A throwaway database file, removed on drop.
struct TempDb {
    dir: std::path::PathBuf,
    url: String,
}

impl TempDb {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("chess-auth-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let url = format!("sqlite:{}/auth.db", dir.display());
        Self { dir, url }
    }

    /// One string column of the first row, or `None` when the value is NULL.
    async fn text(&self, sql: &str) -> Option<String> {
        let pool = self.pool().await;
        let value: Option<String> = sqlx::query_scalar(sql).fetch_one(&pool).await.expect(sql);
        pool.close().await;
        value
    }

    /// Every string in one column.
    async fn column(&self, sql: &str) -> Vec<String> {
        let pool = self.pool().await;
        let values: Vec<String> = sqlx::query_scalar(sql).fetch_all(&pool).await.expect(sql);
        pool.close().await;
        values
    }

    async fn count(&self, sql: &str) -> i64 {
        let pool = self.pool().await;
        let value: i64 = sqlx::query_scalar(sql).fetch_one(&pool).await.expect(sql);
        pool.close().await;
        value
    }

    async fn pool(&self) -> sqlx::SqlitePool {
        sqlx::SqlitePool::connect(&self.url).await.expect("open the test database")
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A real argon2id hash at the cheapest supported cost: the hashing is still
/// real, but a suite that registers a dozen accounts does not spend seconds in
/// memory-hard work.
fn config_for(url: &str) -> Config {
    Config {
        database_url: Some(url.to_string()),
        argon2: Argon2Params {
            memory_kib: MIN_ARGON2_MEMORY_KIB,
            time_cost: MIN_ARGON2_TIME_COST,
            parallelism: MIN_ARGON2_PARALLELISM,
        },
        ..Config::for_test(
            "127.0.0.1".parse().unwrap(),
            0,
            Duration::from_secs(30),
        )
    }
}

async fn spawn_server(url: &str) -> (SocketAddr, Arc<App>) {
    let app = Arc::new(App::init(config_for(url)).await.expect("App::init"));
    let router = axum::Router::new()
        .route(
            "/healthz",
            axum::routing::get(chess_server::infrastructure::ws::healthz),
        )
        .route(
            "/ws",
            axum::routing::get(chess_server::infrastructure::ws::ws_upgrade),
        )
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
    let (stream, _response) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .expect("connect to /ws");
    stream
}

async fn send(client: &mut WsClient, message: &ClientMessage) {
    let json = serde_json::to_string(message).expect("serialize");
    client
        .send(Message::Text(Utf8Bytes::from(json)))
        .await
        .expect("send frame");
}

async fn next_server_message(client: &mut WsClient) -> ServerMessage {
    loop {
        match client.next().await {
            Some(Ok(Message::Text(text))) => {
                return serde_json::from_str(text.as_str()).expect("parse server message");
            }
            Some(Ok(Message::Close(_))) | None => {
                panic!("the server closed the socket unexpectedly")
            }
            Some(Err(err)) => panic!("socket read error: {err}"),
            Some(Ok(_)) => continue,
        }
    }
}

/// The `(code, message)` of an error answer: the pair the credential
/// indistinguishability requirement is about.
fn error_of(message: &ServerMessage) -> (String, String) {
    match message {
        ServerMessage::Error { code, message, .. } => (code.clone(), message.clone()),
        other => panic!("expected an error, got {other:?}"),
    }
}

async fn next_state(client: &mut WsClient) -> State {
    match next_server_message(client).await {
        ServerMessage::RoomReady { state, .. } | ServerMessage::State { state, .. } => state,
        ServerMessage::Error { code, .. } => panic!("expected a snapshot, got error `{code}`"),
        other => panic!("expected a snapshot, got {other:?}"),
    }
}

/// The socket stays open and sends nothing else: a refusal is exactly one
/// error frame.
async fn assert_silent(client: &mut WsClient) {
    match tokio::time::timeout(Duration::from_millis(150), client.next()).await {
        Err(_) => {}
        Ok(Some(Ok(frame))) => {
            panic!("expected silence after the refusal, got {frame:?}")
        }
        Ok(Some(Err(err))) => panic!("socket read error after the refusal: {err}"),
        Ok(None) => panic!("the socket was closed by a rejected authentication request"),
    }
}

async fn register(
    client: &mut WsClient,
    username: &str,
    password: &str,
    device_id: Option<&str>,
) -> ServerMessage {
    send(
        client,
        &ClientMessage::Register {
            v: 1,
            username: username.into(),
            password: password.into(),
            device_id: device_id.map(str::to_string),
        },
    )
    .await;
    next_server_message(client).await
}

async fn login(
    client: &mut WsClient,
    username: &str,
    password: &str,
    device_id: Option<&str>,
) -> ServerMessage {
    send(
        client,
        &ClientMessage::Login {
            v: 1,
            username: username.into(),
            password: password.into(),
            device_id: device_id.map(str::to_string),
        },
    )
    .await;
    next_server_message(client).await
}

async fn logout(client: &mut WsClient, token: &str) -> ServerMessage {
    send(
        client,
        &ClientMessage::Logout {
            v: 1,
            token: token.into(),
        },
    )
    .await;
    next_server_message(client).await
}

async fn set_profile(client: &mut WsClient, display_name: &str, token: &str) -> ServerMessage {
    send(
        client,
        &ClientMessage::SetProfile {
            v: 1,
            display_name: display_name.into(),
            token: token.into(),
        },
    )
    .await;
    next_server_message(client).await
}

/// The `(account_id, token)` of a `Session` answer, or a panic.
fn session(message: &ServerMessage) -> (String, String) {
    match message {
        ServerMessage::Session {
            account_id, token, ..
        } => (account_id.clone(), token.clone()),
        other => panic!("expected a session, got {other:?}"),
    }
}

/// Create a room and answer with the code plus the `room_ready` snapshot.
///
/// Both come back together because a lone creator receives *nothing* after its
/// `room_ready`: asking for another snapshot would wait forever.
async fn create_room_as(
    client: &mut WsClient,
    device_id: &str,
    token: Option<&str>,
) -> (String, State) {
    send(
        client,
        &ClientMessage::CreateRoom {
            v: 1,
            player_id: device_id.into(),
            time_control: None,
            token: token.map(str::to_string),
        },
    )
    .await;
    match next_server_message(client).await {
        ServerMessage::RoomReady {
            room_code, state, ..
        } => (room_code, state),
        other => panic!("expected room_ready, got {other:?}"),
    }
}

async fn join_room_as(client: &mut WsClient, device_id: &str, code: &str, token: Option<&str>) {
    send(
        client,
        &ClientMessage::JoinRoom {
            v: 1,
            player_id: device_id.into(),
            room_code: code.into(),
            token: token.map(str::to_string),
        },
    )
    .await;
}

async fn move_piece(client: &mut WsClient, uci: &str) {
    send(
        client,
        &ClientMessage::Move {
            v: 1,
            uci: uci.into(),
        },
    )
    .await;
}

/// Registers `Ana` on `device_id` and hands back `(account_id, token)`.
async fn sign_up(addr: SocketAddr, device_id: &str) -> (String, String) {
    let mut client = connect(addr).await;
    session(&register(&mut client, "Ana", "correct horse battery", Some(device_id)).await)
}

// ---------------------------------------------------------------------------
// 8.1 Registration
// ---------------------------------------------------------------------------

/// 8.1 A registration answers with a session whose token authenticates a room
/// message, and the durable password column holds an argon2id hash — never the
/// plaintext, and never the token.
#[tokio::test]
async fn registration_issues_a_usable_session_without_storing_the_password() {
    let db = TempDb::new();
    let (addr, _app) = spawn_server(&db.url).await;
    let mut client = connect(addr).await;

    // Auth messages are answerable before any room exists (8.5).
    let answer = register(&mut client, "Ana", "correct horse battery", Some("device-a")).await;
    let (account_id, token) = session(&answer);
    let ServerMessage::Session {
        username,
        display_name,
        expires_at_ms,
        ..
    } = &answer
    else {
        panic!("expected a session");
    };
    assert_eq!(username, "Ana");
    assert_eq!(display_name, "Ana", "the display name defaults to the username");
    assert!(*expires_at_ms > 0, "a session must say when it expires");
    assert!(!token.is_empty(), "a session must carry a token");

    // The token is usable: the very next room message carries it.
    let (code, ready) = create_room_as(&mut client, "device-a", Some(&token)).await;
    assert!(!code.is_empty());
    assert_eq!(ready.your_color, Color::White);

    // The stored password is a hash, not the plaintext.
    let stored = db
        .text("SELECT password_hash FROM accounts")
        .await
        .expect("the account was persisted");
    assert_ne!(
        stored, "correct horse battery",
        "the password must not be stored in the clear"
    );
    assert!(
        stored.starts_with("$argon2id$"),
        "the password must be an argon2id hash, got {stored}"
    );

    // The session row holds only the token's hash.
    let hashes = db.column("SELECT token_hash FROM sessions").await;
    assert_eq!(hashes.len(), 1, "one registration, one session");
    assert_ne!(hashes[0], token, "the token itself is never stored");
    assert_eq!(hashes[0].len(), 64, "SHA-256 in hex is 64 characters");

    // The device named at registration is bound to the account on disk.
    assert_eq!(
        db.text("SELECT account_id FROM profiles WHERE device_id = 'device-a'")
            .await
            .as_deref(),
        Some(account_id.as_str()),
        "the device link is durable"
    );
}

/// A taken username is refused, in any casing: the uniqueness key is the
/// case-folded username.
#[tokio::test]
async fn a_taken_username_is_refused_however_it_is_cased() {
    let db = TempDb::new();
    let (addr, _app) = spawn_server(&db.url).await;
    let mut setup = connect(addr).await;
    session(&register(&mut setup, "Ana", "correct horse battery", None).await);

    let mut second = connect(addr).await;
    let (code, message) = error_of(&register(&mut second, "Ana", "another long password", None).await);
    assert_eq!(code, "username_taken");

    let mut third = connect(addr).await;
    let (code_again, message_again) =
        error_of(&register(&mut third, "aNA", "another long password", None).await);
    assert_eq!(
        (code_again.as_str(), message_again.as_str()),
        (code.as_str(), message.as_str()),
        "the answer must not depend on the casing that was tried"
    );
    assert_eq!(
        db.count("SELECT COUNT(*) FROM accounts").await,
        1,
        "no second account was created"
    );
}

/// Validation names the offending field, so a client can point at the right
/// input (spec "Player Account Registration"), and rejects nothing silently.
#[tokio::test]
async fn a_registration_validation_error_names_the_field() {
    let db = TempDb::new();
    let (addr, _app) = spawn_server(&db.url).await;

    let cases = [
        ("too short", "a", "correct horse battery", "username"),
        ("with a space", "Ana Maria", "correct horse battery", "username"),
        ("with a symbol", "Ana!", "correct horse battery", "username"),
        ("far too long", &"a".repeat(64), "correct horse battery", "username"),
        ("empty password", "Ana", "x", "password"),
        ("far too long password", "Ana", &"p".repeat(200), "password"),
    ];
    for (label, username, password, expected) in cases {
        let mut client = connect(addr).await;
        let (code, message) = error_of(&register(&mut client, username, password, None).await);
        assert_eq!(code, "invalid_request", "{label}");
        assert!(
            message.contains(expected),
            "{label}: the message must name `{expected}`, got {message:?}"
        );
        assert_silent(&mut client).await;
    }
    assert_eq!(
        db.count("SELECT COUNT(*) FROM accounts").await,
        0,
        "a rejected registration creates nothing"
    );
}

// ---------------------------------------------------------------------------
// 8.2 Login
// ---------------------------------------------------------------------------

/// 8.2 An unknown username and a wrong password are indistinguishable: the same
/// code, the same message, and no session for either.
#[tokio::test]
async fn login_refuses_a_wrong_password_and_an_unknown_username_identically() {
    let db = TempDb::new();
    let (addr, _app) = spawn_server(&db.url).await;
    let mut setup = connect(addr).await;
    session(&register(&mut setup, "Ana", "correct horse battery", None).await);

    let mut wrong_password = connect(addr).await;
    let (wrong_code, wrong_message) =
        error_of(&login(&mut wrong_password, "Ana", "not the password", None).await);
    let mut unknown_user = connect(addr).await;
    let (unknown_code, unknown_message) =
        error_of(&login(&mut unknown_user, "Nobody", "correct horse battery", None).await);

    assert_eq!(wrong_code, "invalid_credentials");
    assert_eq!(
        (unknown_code, unknown_message),
        (wrong_code, wrong_message),
        "the two refusals must be byte-identical: a client must not be able to \
         tell an existing account from a missing one"
    );
    assert_eq!(
        db.count("SELECT COUNT(*) FROM sessions").await,
        1,
        "only the registration's session exists: neither refusal issued one"
    );
    assert_silent(&mut unknown_user).await;
}

/// 8.2 The success path, plus the relink: a device that belonged to another
/// account follows the account that just signed in, and one device holds one
/// account at a time.
#[tokio::test]
async fn login_issues_a_session_and_relinks_the_device() {
    let db = TempDb::new();
    let (addr, _app) = spawn_server(&db.url).await;

    let mut ana_setup = connect(addr).await;
    let (ana_id, _) = session(&register(&mut ana_setup, "Ana", "correct horse battery", None).await);
    let mut bob_setup = connect(addr).await;
    session(&register(&mut bob_setup, "Bob", "another long password", None).await);

    // The shared phone starts out linked to Bob.
    let mut bob_on_phone = connect(addr).await;
    let bob_id = session(
        &login(&mut bob_on_phone, "Bob", "another long password", Some("device-phone")).await,
    )
    .0;
    assert_eq!(
        db.text("SELECT account_id FROM profiles WHERE device_id = 'device-phone'")
            .await
            .as_deref(),
        Some(bob_id.as_str())
    );

    // Ana signs in on it: the link moves.
    let mut ana_on_phone = connect(addr).await;
    let (account_id, token) = session(
        &login(&mut ana_on_phone, "Ana", "correct horse battery", Some("device-phone")).await,
    );
    assert_eq!(account_id, ana_id);
    assert_eq!(
        db.text("SELECT account_id FROM profiles WHERE device_id = 'device-phone'")
            .await
            .as_deref(),
        Some(ana_id.as_str()),
        "the device followed the account that signed in"
    );
    assert_eq!(
        db.count("SELECT COUNT(*) FROM profiles WHERE device_id = 'device-phone'")
            .await,
        1,
        "one device holds one account: the link moved rather than shadowing"
    );

    // The session authenticates a room message.
    let (_, ready) = create_room_as(&mut ana_on_phone, "device-phone", Some(&token)).await;
    assert_eq!(ready.your_color, Color::White);
}

/// A login whose password is out of the accepted length is a credential
/// failure, not a format failure: it must disclose nothing about the account.
#[tokio::test]
async fn a_login_with_an_out_of_range_password_is_a_credential_failure() {
    let db = TempDb::new();
    let (addr, _app) = spawn_server(&db.url).await;
    let mut setup = connect(addr).await;
    session(&register(&mut setup, "Ana", "correct horse battery", None).await);

    for password in ["x", &"p".repeat(200)] {
        let mut client = connect(addr).await;
        let (code, message) = error_of(&login(&mut client, "Ana", password, None).await);
        assert_eq!(code, "invalid_credentials");
        assert_eq!(message, "That username and password do not match an account");
    }
    assert_eq!(db.count("SELECT COUNT(*) FROM sessions").await, 1);
}

// ---------------------------------------------------------------------------
// 8.3 Logout
// ---------------------------------------------------------------------------

/// 8.3 Logout revokes only the presented session, the device link survives, and
/// the revocation is durable.
#[tokio::test]
async fn logout_revokes_only_the_presented_session() {
    let db = TempDb::new();
    let (addr, _app) = spawn_server(&db.url).await;

    // Two sessions for one account, from two devices.
    let mut setup = connect(addr).await;
    let (account_id, first_token) =
        session(&register(&mut setup, "Ana", "correct horse battery", Some("device-phone")).await);
    let mut tablet = connect(addr).await;
    let (_, second_token) = session(
        &login(&mut tablet, "Ana", "correct horse battery", Some("device-tablet")).await,
    );

    // The second session holds a room, so the account is occupied: an
    // authenticated connection from a third device would be refused, while a
    // guest one is not. That difference is how the two tokens are compared.
    let _ = create_room_as(&mut tablet, "device-tablet", Some(&second_token)).await;

    // Logout the first token, on a connection that holds no room.
    let mut logout_client = connect(addr).await;
    match logout(&mut logout_client, &first_token).await {
        ServerMessage::SessionOk {
            v: VERSION,
            account_id: ended,
            username,
            display_name,
            expires_at_ms,
        } => {
            assert_eq!(ended, account_id);
            assert_eq!(username, "Ana");
            assert_eq!(display_name, "Ana");
            assert!(expires_at_ms > 0, "the answer says when the session ended");
        }
        other => panic!("expected session_ok, got {other:?}"),
    }

    // The revoked token no longer resolves the account: from a fresh device it
    // opens a room, where a live token would be refused by the account limit.
    let mut after_revoke = connect(addr).await;
    let _ = create_room_as(&mut after_revoke, "device-fresh", Some(&first_token)).await;

    // The other session is untouched.
    let mut still_authenticated = connect(addr).await;
    send(
        &mut still_authenticated,
        &ClientMessage::CreateRoom {
            v: 1,
            player_id: "device-laptop".into(),
            time_control: None,
            token: Some(second_token.clone()),
        },
    )
    .await;
    let (code, _) = error_of(&next_server_message(&mut still_authenticated).await);
    assert_eq!(
        code, "already_in_room",
        "the second session still authenticates: the account still holds a room"
    );

    // The revocation is on disk, and the device link is untouched.
    assert_eq!(
        db.count("SELECT COUNT(*) FROM sessions WHERE revoked_at_ms IS NOT NULL")
            .await,
        1,
        "exactly one session row is revoked"
    );
    assert_eq!(
        db.text("SELECT account_id FROM profiles WHERE device_id = 'device-phone'")
            .await
            .as_deref(),
        Some(account_id.as_str()),
        "logging out keeps the link but ends the session"
    );
}

/// Logging out a token the server never issued, and logging out twice, are both
/// refused — with distinct answers where the spec asks for them.
#[tokio::test]
async fn logout_of_an_unknown_or_already_revoked_token_is_refused() {
    let db = TempDb::new();
    let (addr, _app) = spawn_server(&db.url).await;
    let mut setup = connect(addr).await;
    let (_, token) = session(&register(&mut setup, "Ana", "correct horse battery", None).await);

    let mut stranger = connect(addr).await;
    let (code, _) = error_of(&logout(&mut stranger, "never-issued").await);
    assert_eq!(code, "not_authenticated");
    assert_silent(&mut stranger).await;

    let mut empty = connect(addr).await;
    let (code, _) = error_of(&logout(&mut empty, "").await);
    assert_eq!(code, "not_authenticated");

    let mut first = connect(addr).await;
    assert!(matches!(
        logout(&mut first, &token).await,
        ServerMessage::SessionOk { .. }
    ));
    assert_eq!(
        db.count("SELECT COUNT(*) FROM sessions WHERE revoked_at_ms IS NOT NULL")
            .await,
        1
    );

    let mut again = connect(addr).await;
    let (code, _) = error_of(&logout(&mut again, &token).await);
    assert_eq!(
        code, "not_authenticated",
        "a second logout is refused rather than answered as a fresh sign-out"
    );
    assert_silent(&mut again).await;
    assert_eq!(
        db.count("SELECT COUNT(*) FROM sessions WHERE revoked_at_ms IS NOT NULL")
            .await,
        1,
        "the refusal did not write anything"
    );
}

// ---------------------------------------------------------------------------
// 8.4 Display name
// ---------------------------------------------------------------------------

/// 8.4 A display name is authenticated-only, validated before it is stored, and
/// echoed back on success.
#[tokio::test]
async fn a_display_name_needs_a_session_and_a_rejected_one_keeps_the_stored_name() {
    let db = TempDb::new();
    let (addr, _app) = spawn_server(&db.url).await;

    // A guest is refused, and an empty token counts as no token.
    for token in ["", "never-issued"] {
        let mut guest = connect(addr).await;
        let (code, _) = error_of(&set_profile(&mut guest, "Ana T.", token).await);
        assert_eq!(
            code, "not_authenticated",
            "a display-name change is authenticated-only (token {token:?})"
        );
        assert_silent(&mut guest).await;
    }

    let mut setup = connect(addr).await;
    let (account_id, token) =
        session(&register(&mut setup, "Ana", "correct horse battery", None).await);

    // A good name is stored and echoed.
    let mut named = connect(addr).await;
    match set_profile(&mut named, "Ana T.", &token).await {
        ServerMessage::ProfileUpdated {
            v: VERSION,
            account_id: updated,
            display_name,
        } => {
            assert_eq!(updated, account_id);
            assert_eq!(display_name, "Ana T.");
        }
        other => panic!("expected profile_updated, got {other:?}"),
    }
    assert_eq!(
        db.text("SELECT display_name FROM accounts")
            .await
            .as_deref(),
        Some("Ana T."),
        "the name is durable"
    );

    // A rejected name keeps the stored one.
    for name in ["x".repeat(64).as_str(), "  ", "\u{0}bad"] {
        let mut bad = connect(addr).await;
        let (code, _) = error_of(&set_profile(&mut bad, name, &token).await);
        assert_eq!(code, "invalid_display_name", "name {name:?}");
        assert_silent(&mut bad).await;
        assert_eq!(
            db.text("SELECT display_name FROM accounts").await.as_deref(),
            Some("Ana T."),
            "the rejected name did not overwrite the stored one"
        );
    }

    // A revoked session cannot change the name either.
    let mut revoker = connect(addr).await;
    assert!(matches!(
        logout(&mut revoker, &token).await,
        ServerMessage::SessionOk { .. }
    ));
    let mut after = connect(addr).await;
    let (code, _) = error_of(&set_profile(&mut after, "Ana T.", &token).await);
    assert_eq!(code, "not_authenticated", "a revoked session is not a session");
    assert_eq!(
        db.text("SELECT display_name FROM accounts").await.as_deref(),
        Some("Ana T.")
    );
}

// ---------------------------------------------------------------------------
// 8.5 A refusal never costs the player their game
// ---------------------------------------------------------------------------

/// 8.5 Every rejection leaves the socket open, silent, and able to play: the
/// connection refused a login can still create and play a guest room.
#[tokio::test]
async fn a_refused_authentication_leaves_the_connection_able_to_play() {
    let db = TempDb::new();
    let (addr, _app) = spawn_server(&db.url).await;
    let mut setup = connect(addr).await;
    session(&register(&mut setup, "Ana", "correct horse battery", None).await);

    let mut client = connect(addr).await;
    let refusals = [
        ("Ana", "wrong password entirely", "wrong password"),
        ("Nobody", "correct horse battery", "unknown username"),
        ("a", "correct horse battery", "invalid username"),
        ("Ana", "x", "out-of-range password"),
    ];
    for (username, password, label) in refusals {
        let (_, message) = error_of(&login(&mut client, username, password, None).await);
        assert!(!message.is_empty(), "{label}: an error must carry a message");
        assert_silent(&mut client).await;
    }
    // The same socket still plays.
    let (code, state) = create_room_as(&mut client, "device-a", None).await;
    assert!(!code.is_empty());
    assert_eq!(state.your_color, Color::White);
    assert_eq!(state.status, Status::Playing);
    assert_eq!(
        db.count("SELECT COUNT(*) FROM accounts").await,
        1,
        "no refusal created an account"
    );
}

/// The auth arms answer and return before the room dispatch, so no auth frame
/// can create or join a room (design D11).
#[tokio::test]
async fn an_authentication_message_never_creates_or_joins_a_room() {
    let db = TempDb::new();
    let (addr, _app) = spawn_server(&db.url).await;
    let mut client = connect(addr).await;

    let (_, token) = session(&register(&mut client, "Ana", "correct horse battery", None).await);
    logout(&mut client, &token).await;
    set_profile(&mut client, "Ana T.", &token).await;

    // If any of those frames had reached the room dispatch, the connection would
    // already be in a room and this `create_room` would be `already_in_room`.
    let (code, state) = create_room_as(&mut client, "device-a", None).await;
    assert!(!code.is_empty(), "auth frames never entered the room dispatch");
    assert_eq!(state.your_color, Color::White);
    let _ = &db;
}

// ---------------------------------------------------------------------------
// 7.2 Guest play is unaffected
// ---------------------------------------------------------------------------

/// 7.2 The same game over the same frames, once with no token at all and once
/// with sessions: nothing about play changes but who the player is.
#[tokio::test]
async fn guest_and_authenticated_play_identically_over_the_wire() {
    let db = TempDb::new();
    let (addr, _app) = spawn_server(&db.url).await;

    // The guest game: no token on any frame.
    let mut guest_white = connect(addr).await;
    let mut guest_black = connect(addr).await;
    let (guest_code, _) = create_room_as(&mut guest_white, "device-a", None).await;
    join_room_as(&mut guest_black, "device-b", &guest_code, None).await;
    assert_eq!(next_state(&mut guest_black).await.your_color, Color::Black);
    // White is told the opponent arrived; draining it keeps the two games'
    // snapshots in lockstep below.
    assert!(next_state(&mut guest_white).await.opponent_online);

    // The authenticated game: the same frames plus a token.
    let mut white_setup = connect(addr).await;
    let (_, ana_token) = session(&register(&mut white_setup, "Ana", "correct horse battery", None).await);
    let mut black_setup = connect(addr).await;
    let (_, bob_token) = session(&register(&mut black_setup, "Bob", "another long password", None).await);
    let mut white = connect(addr).await;
    let mut black = connect(addr).await;
    let (code, _) = create_room_as(&mut white, "device-x", Some(&ana_token)).await;
    join_room_as(&mut black, "device-y", &code, Some(&bob_token)).await;
    let black_state = next_state(&mut black).await;
    assert_eq!(black_state.your_color, Color::Black);
    assert!(black_state.opponent_online);
    next_state(&mut white).await;

    // Both games follow one script and stay in lockstep, move for move and
    // snapshot for snapshot.
    for (side, uci) in [
        (Color::White, "e2e4"),
        (Color::Black, "e7e5"),
        (Color::White, "d1h5"),
        (Color::Black, "b8c6"),
        (Color::White, "f1c4"),
    ] {
        let (guest_state, authed_state) = match side {
            Color::White => {
                move_piece(&mut guest_white, uci).await;
                move_piece(&mut white, uci).await;
                (next_state(&mut guest_white).await, next_state(&mut white).await)
            }
            Color::Black => {
                move_piece(&mut guest_black, uci).await;
                move_piece(&mut black, uci).await;
                (next_state(&mut guest_black).await, next_state(&mut black).await)
            }
        };
        assert_eq!(guest_state.move_list, authed_state.move_list);
        assert_eq!(guest_state.board_fen, authed_state.board_fen);
        assert_eq!(guest_state.side_to_move, authed_state.side_to_move);
        assert_eq!(guest_state.status, authed_state.status);
    }
    let _ = &db;
}

// ---------------------------------------------------------------------------
// 7.3 A login issued mid-connection
// ---------------------------------------------------------------------------

/// 7.3 A login that arrives on a connection already playing is adopted by the
/// next room-affecting action: the account's room limit applies immediately,
/// the seat the connection holds is upgraded, and no one can replay its device.
#[tokio::test]
async fn a_login_mid_connection_is_adopted_by_the_next_action() {
    let db = TempDb::new();
    let (addr, _app) = spawn_server(&db.url).await;

    // White starts as a plain guest and Black joins.
    let mut white = connect(addr).await;
    let (code, _) = create_room_as(&mut white, "device-a", None).await;
    let mut black = connect(addr).await;
    join_room_as(&mut black, "device-b", &code, None).await;
    next_state(&mut black).await;
    next_state(&mut white).await;

    // Before the account exists, a second device of the same player opens its
    // own room: no account, so nothing limits it.
    let mut spare = connect(addr).await;
    let (spare_code, _) = create_room_as(&mut spare, "device-tablet", None).await;
    assert_ne!(spare_code, code);

    // White signs in mid-connection.
    let (_, token) = session(
        &register(&mut white, "Ana", "correct horse battery", Some("device-a")).await,
    );

    // The account room limit now applies: a second device of the account cannot
    // open a room while White holds this one.
    let mut thief = connect(addr).await;
    send(
        &mut thief,
        &ClientMessage::CreateRoom {
            v: 1,
            player_id: "device-laptop".into(),
            time_control: None,
            token: Some(token.clone()),
        },
    )
    .await;
    let (error_code, _) = error_of(&next_server_message(&mut thief).await);
    assert_eq!(
        error_code, "already_in_room",
        "one account holds at most one room, even from a second device, from the \
         moment the login is answered"
    );
    assert_silent(&mut thief).await;

    // White's next action still reaches the seat, under the upgraded identity.
    move_piece(&mut white, "e2e4").await;
    assert_eq!(
        next_state(&mut white).await.move_list,
        vec!["e2e4".to_string()],
        "the move landed in White's seat"
    );

    // The seat is account-owned now, so a connection with no session that
    // replays White's device id is refused.
    let mut replay = connect(addr).await;
    join_room_as(&mut replay, "device-a", &code, None).await;
    let (error_code, _) = error_of(&next_server_message(&mut replay).await);
    assert_eq!(
        error_code, "room_full",
        "a signed-out replay of a signed-in player's device id is refused"
    );
    assert_silent(&mut replay).await;

    // White's move reached Black, so the game is untouched by any of it.
    assert_eq!(
        next_state(&mut black).await.move_list,
        vec!["e2e4".to_string()],
        "the move reached Black as it would have without any of the above"
    );
    move_piece(&mut black, "e7e5").await;
    assert_eq!(
        next_state(&mut black).await.move_list,
        vec!["e2e4".to_string(), "e7e5".to_string()],
        "the game continued unchanged"
    );
    let _ = &db;
}

// ---------------------------------------------------------------------------
// 9.3 Cross-device re-attach
// ---------------------------------------------------------------------------

/// 9.3 The account follows the player: the same account re-attaches to a live
/// game from a second device with the server-measured remaining time, and a
/// third party presenting that device id is refused.
#[tokio::test]
async fn an_account_re_attaches_from_a_second_device_while_a_replay_is_refused() {
    let db = TempDb::new();
    let (addr, _app) = spawn_server(&db.url).await;

    // White registers first, so its seat is account-owned from the start.
    let mut phone = connect(addr).await;
    let (account_id, token) =
        session(&register(&mut phone, "Ana", "correct horse battery", Some("device-phone")).await);
    let (code, _) = create_room_as(&mut phone, "device-phone", Some(&token)).await;
    let mut black = connect(addr).await;
    join_room_as(&mut black, "device-black", &code, None).await;
    next_state(&mut black).await;
    next_state(&mut phone).await;

    // A move, so the re-attach has real state to resume.
    move_piece(&mut phone, "e2e4").await;
    let before = next_state(&mut phone).await;
    assert_eq!(before.move_list, vec!["e2e4".to_string()]);
    next_state(&mut black).await;

    // The phone's socket drops; the seat is held for the grace window.
    drop(phone);
    tokio::time::sleep(Duration::from_millis(80)).await;

    // A third party, no session, presenting the seated device id: refused.
    let mut replay = connect(addr).await;
    join_room_as(&mut replay, "device-phone", &code, None).await;
    let (error_code, _) = error_of(&next_server_message(&mut replay).await);
    assert_eq!(error_code, "room_full", "the seat was not handed over");
    assert_silent(&mut replay).await;

    // The real player, from a second device, with the session: the same seat.
    let mut tablet = connect(addr).await;
    join_room_as(&mut tablet, "device-tablet", &code, Some(&token)).await;
    let resumed = next_state(&mut tablet).await;
    assert_eq!(
        resumed.your_color, Color::White,
        "the account found its seat from another device"
    );
    assert_eq!(resumed.status, Status::Playing);
    assert_eq!(
        resumed.move_list, before.move_list,
        "the game resumed rather than restarting"
    );
    assert!(
        resumed.white_time_ms > 0 && resumed.white_time_ms <= before.white_time_ms,
        "the remaining time is the one the server measured: {} before the drop, \
         {} on re-attach",
        before.white_time_ms,
        resumed.white_time_ms
    );

    // And it really is the same player: they finish the game from the tablet.
    send(
        &mut tablet,
        &ClientMessage::Resign {
            v: 1,
        },
    )
    .await;
    loop {
        let state = next_state(&mut black).await;
        if state.status.is_terminal() {
            assert_eq!(state.status, Status::Resigned { winner: Color::Black });
            break;
        }
    }

    // The second device is linked to the account durably, so the profile
    // follows the player.
    assert_eq!(
        db.text("SELECT account_id FROM profiles WHERE device_id = 'device-tablet'")
            .await
            .as_deref(),
        Some(account_id.as_str()),
        "re-attaching from a second device links it"
    );
}

/// A logged-out player keeps playing: the session ends, the connection stays in
/// its room under the identity it entered with (spec "Guest Play Fallback").
#[tokio::test]
async fn a_player_who_logs_out_keeps_playing_on_the_same_connection() {
    let db = TempDb::new();
    let (addr, _app) = spawn_server(&db.url).await;
    let (_, token) = sign_up(addr, "device-phone").await;

    let mut client = connect(addr).await;
    let (code, _) = create_room_as(&mut client, "device-phone", Some(&token)).await;
    let mut opponent = connect(addr).await;
    join_room_as(&mut opponent, "device-b", &code, None).await;
    next_state(&mut opponent).await;
    next_state(&mut client).await;

    assert!(matches!(
        logout(&mut client, &token).await,
        ServerMessage::SessionOk { .. }
    ));

    // The connection is still in the room and still plays.
    move_piece(&mut client, "e2e4").await;
    let state = next_state(&mut client).await;
    assert_eq!(state.your_color, Color::White);
    assert_eq!(state.move_list, vec!["e2e4".to_string()]);
    let _ = &db;
}

/// The identity a connection pinned when it entered its room is the one every
/// later room action uses, and re-deriving it is not even possible once the
/// session is gone: the room messages carry no token, so `handle_text` has
/// nothing to re-resolve from (spec "Re-attachment ... matched by the player's
/// resolved identity", design D8).
#[tokio::test]
async fn every_room_action_reuses_the_identity_the_connection_pinned() {
    let db = TempDb::new();
    let (addr, _app) = spawn_server(&db.url).await;

    // The connection enters its room as a guest, then signs in mid-connection:
    // its pinned identity is upgraded in place (7.3).
    let mut white = connect(addr).await;
    let (code, _) = create_room_as(&mut white, "device-a", None).await;
    let mut black = connect(addr).await;
    join_room_as(&mut black, "device-b", &code, None).await;
    next_state(&mut black).await;
    next_state(&mut white).await;

    let (_, token) = session(
        &register(&mut white, "Ana", "correct horse battery", Some("device-a")).await,
    );
    assert!(matches!(
        logout(&mut white, &token).await,
        ServerMessage::SessionOk { .. }
    ));

    // Move: no token is on the frame, and the seat answers anyway, because the
    // connection's own identity is what reaches the actor.
    move_piece(&mut white, "e2e4").await;
    assert_eq!(
        next_state(&mut white).await.move_list,
        vec!["e2e4".to_string()],
        "Move reused the pinned identity rather than falling back to nothing"
    );

    // Resign: same thing, and it ends the game with the opponent as winner.
    send(&mut white, &ClientMessage::Resign { v: 1 }).await;
    loop {
        let state = next_state(&mut black).await;
        if state.status.is_terminal() {
            assert_eq!(state.status, Status::Resigned { winner: Color::Black });
            break;
        }
    }

    // White sees the terminal snapshot too; draining it keeps the next read from
    // picking up a frame the resignation left behind.
    loop {
        let state = next_state(&mut white).await;
        if state.status.is_terminal() {
            assert_eq!(state.status, Status::Resigned { winner: Color::Black });
            break;
        }
    }

    // Leave: after the game is over the connection leaves the room and is free
    // to open another one, which it can only do if it left rather than stayed.
    send(&mut white, &ClientMessage::Leave { v: 1 }).await;
    let (_, new_state) = create_room_as(&mut white, "device-a", None).await;
    assert_eq!(
        new_state.your_color, Color::White,
        "Leave released the connection, which could then create a room"
    );
    let _ = &db;
}
