//! Persistence integration tests (add-sqlite-persistence D4/D6,
//! add-player-login-sessions 9.2): play a terminal game over the full wire,
//! restart the server on the same database file, and assert the restarted
//! app's snapshot carries the persisted Glicko-2 ratings (not the 1500
//! default) and the database contains exactly one `games` row and two
//! `rating_history` rows; then assert the authentication tables (accounts,
//! profiles, sessions) come back with it, and that a failed authentication
//! write is logged instead of taking the player out of the game.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chess_server::infrastructure::config::{Argon2Params, Config};
use chess_server::infrastructure::server::App;
use chess_server::interface::protocol::{
    ClientMessage, Color, ServerMessage, State, Status,
};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::{Message, Utf8Bytes};
use tokio_tungstenite::WebSocketStream;

type WsClient = WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn terminal_state(client: &mut WsClient) -> State {
    loop {
        match next_server_message(client).await {
            ServerMessage::State { state, .. } => {
                if state.status.is_terminal() {
                    return state;
                }
            }
            ServerMessage::Error { code, .. } => panic!("expected a snapshot, got error `{code}`"),
            _ => {}
        }
    }
}

fn build_config(url: &str) -> Config {
    Config::with_database(url)
}

/// The same database wiring with argon2id at its cheapest supported cost:
/// still a real hash, just not seconds of memory-hard work per account.
fn auth_config(url: &str) -> Config {
    Config {
        argon2: Argon2Params {
            memory_kib: chess_server::infrastructure::config::MIN_ARGON2_MEMORY_KIB,
            time_cost: chess_server::infrastructure::config::MIN_ARGON2_TIME_COST,
            parallelism: chess_server::infrastructure::config::MIN_ARGON2_PARALLELISM,
        },
        ..Config::with_database(url)
    }
}

/// A throwaway database file, removed when the test ends.
struct TempDb {
    dir: std::path::PathBuf,
    url: String,
}

impl TempDb {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("chess-auth-db-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let url = format!("sqlite:{}/auth.db", dir.display());
        Self { dir, url }
    }

    async fn pool(&self) -> sqlx::SqlitePool {
        sqlx::SqlitePool::connect(&self.url)
            .await
            .expect("open the test database")
    }

    /// One string column of the first row, or `None` when it is NULL.
    async fn text(&self, sql: &str) -> Option<String> {
        let pool = self.pool().await;
        let value: Option<String> = sqlx::query_scalar(sql).fetch_one(&pool).await.expect(sql);
        pool.close().await;
        value
    }

    async fn count(&self, sql: &str) -> i64 {
        let pool = self.pool().await;
        let value: i64 = sqlx::query_scalar(sql).fetch_one(&pool).await.expect(sql);
        pool.close().await;
        value
    }

    /// Polls until `sql` yields `expected`, so a write the server handed to a
    /// spawned task is waited for rather than raced.
    async fn wait_for_count(&self, sql: &str, expected: i64, budget: Duration) -> bool {
        let deadline = std::time::Instant::now() + budget;
        loop {
            if self.count(sql).await == expected {
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    async fn wait_for_text(&self, sql: &str, expected: &str, budget: Duration) -> bool {
        let deadline = std::time::Instant::now() + budget;
        loop {
            if self.text(sql).await.as_deref() == Some(expected) {
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
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

fn error_code(message: &ServerMessage) -> String {
    match message {
        ServerMessage::Error { code, .. } => code.clone(),
        other => panic!("expected an error, got {other:?}"),
    }
}

async fn send_and_answer(client: &mut WsClient, message: &ClientMessage) -> ServerMessage {
    client.send(action(message)).await.expect("send frame");
    next_server_message(client).await
}

async fn create_room_as(
    client: &mut WsClient,
    device_id: &str,
    token: Option<&str>,
) -> (String, State) {
    let message = send_and_answer(
        client,
        &ClientMessage::CreateRoom {
            v: 1,
            player_id: device_id.into(),
            time_control: None,
            token: token.map(str::to_string),
        },
    )
    .await;
    match message {
        ServerMessage::RoomReady {
            room_code, state, ..
        } => (room_code, state),
        other => panic!("expected room_ready, got {other:?}"),
    }
}

/// Captures the process-wide tracing output so a test can assert that a
/// failure the player never sees was still logged.
#[derive(Clone, Default)]
struct LogCapture(Arc<Mutex<Vec<u8>>>);

impl LogCapture {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().expect("log buffer")).into_owned()
    }
}

impl std::io::Write for LogCapture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("log buffer").extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogCapture {
    type Writer = LogCapture;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Waits up to `budget` for `needle` to appear in the captured log.
///
/// The device-link write is handed to a spawned task, so its log line can
/// land a moment after the answer the client already received.
async fn log_contains(capture: &LogCapture, needle: &str, budget: Duration) -> bool {
    let deadline = std::time::Instant::now() + budget;
    loop {
        if capture.text().contains(needle) {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn spawn_server_with_db(url: &str) -> (SocketAddr, Arc<App>) {
    let config = build_config(url);
    let app = Arc::new(App::init(config.clone()).await.expect("App::init with temp DB"));
    let router = axum::Router::new()
        .route("/healthz", axum::routing::get(chess_server::infrastructure::ws::healthz))
        .route("/ws", axum::routing::get(chess_server::infrastructure::ws::ws_upgrade))
        .with_state(Arc::clone(&app));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind test server");
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

async fn next_server_message(client: &mut WsClient) -> ServerMessage {
    loop {
        match client.next().await {
            Some(Ok(Message::Text(text))) => {
                return serde_json::from_str(text.as_str()).expect("parse server message");
            }
            Some(Ok(Message::Close(_))) => panic!("server closed the socket unexpectedly"),
            Some(Err(err)) => panic!("socket read error: {err}"),
            Some(Ok(_)) => continue,
            None => panic!("server closed the socket unexpectedly"),
        }
    }
}

async fn next_state(client: &mut WsClient) -> State {
    match next_server_message(client).await {
        ServerMessage::RoomReady { state, .. }
        | ServerMessage::State { state, .. } => state,
        ServerMessage::Error { code, .. } => {
            panic!("expected a snapshot, got error `{code}`");
        }
        other => panic!("expected a snapshot, got {other:?}"),
    }
}

#[tokio::test]
async fn persistence_survives_restart() {
    let dir = std::env::temp_dir().join(format!(
        "chess-persistence-test-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let url = format!("sqlite:{}/persist.db", dir.display());

    // Phase 1: play a game to a terminal result over the wire.
    let (addr, app) = spawn_server_with_db(&url).await;
    let mut white_client = connect(addr).await;
    let mut black_client = connect(addr).await;

    // White creates the room.
    white_client
        .send(action(&ClientMessage::CreateRoom {
            v: 1,
            player_id: "device-a".into(),
            time_control: None,
            token: None,
        }))
        .await
        .expect("send create_room");
    let room_ready = next_server_message(&mut white_client).await;
    let ServerMessage::RoomReady { room_code, your_color, state, .. } = room_ready else {
        panic!("expected RoomReady for white");
    };
    assert_eq!(your_color, Color::White);
    assert_eq!(state.status, Status::Playing);

    // Black joins.
    black_client
        .send(action(&ClientMessage::JoinRoom {
            v: 1,
            player_id: "device-b".into(),
            room_code,
            token: None,
        }))
        .await
        .expect("send join_room");
    let _ = next_state(&mut black_client).await;

    // Play: black joins, then resigns immediately (a terminal result with
    // a clear winner, avoiding any dependence on the production core's
    // checkmate detection).
    black_client.send(action(&ClientMessage::Resign { v: 1 })).await.expect("resign");

    // Consume the terminal snapshot on white's side (skip any queued
    // non-terminal snapshots from earlier moves, then return the first
    // terminal state).
    let white_final = terminal_state(&mut white_client).await;
    assert_eq!(
        white_final.status,
        Status::Resigned { winner: Color::White },
        "the final status must be resignation with white as winner"
    );
    assert!(white_final.white_rating > 1500.0, "white rating increased after the win");
    assert!(white_final.black_rating < 1500.0, "black rating decreased after the loss");

    // Ensure the snapshot reached the black client too.
    let black_final = terminal_state(&mut black_client).await;
    assert_eq!(
        black_final.status,
        Status::Resigned { winner: Color::White },
        "the opponent also sees the terminal result"
    );

    // Phase 2: drop the server and restart with the same DB.
    drop(app);
    drop(white_client);
    drop(black_client);

    // Restart App::init on the same database file.
    let restarted_app = App::init(build_config(&url)).await.expect("restart init");
    let restarted_app = Arc::new(restarted_app);
    assert!(
        restarted_app.ratings().rating_state("device-a").is_some(),
        "device-a's persisted Glicko-2 state must survive the restart"
    );
    assert!(
        restarted_app.ratings().rating_state("device-b").is_some(),
        "device-b's persisted Glicko-2 state must survive the restart"
    );
    let white_state = restarted_app.ratings().rating_state("device-a").unwrap();
    assert!(
        white_state.rating > 1500.0,
        "persisted white rating after restart must be above the default (post-win), got {white_state:?}"
    );
    let black_state = restarted_app.ratings().rating_state("device-b").unwrap();
    assert!(
        black_state.rating < 1500.0,
        "persisted black rating after restart must be below the default (post-loss), got {black_state:?}"
    );

    // Phase 3: direct database assertions.
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .expect("direct DB open on same file");

    // One finished game.
    let game_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM games")
        .fetch_one(&pool)
        .await
        .expect("count games");
    assert_eq!(game_count, 1, "exactly one finished game in the database");

    // The ratings table has both devices at their post-game state.
    let (post_white_rating,): (f64,) = sqlx::query_as(
        "SELECT rating FROM players WHERE device_id = 'device-a'",
    )
    .fetch_one(&pool)
    .await
    .expect("post-game white rating");
    assert!(post_white_rating > 1500.0);

    let (post_black_rating,): (f64,) = sqlx::query_as(
        "SELECT rating FROM players WHERE device_id = 'device-b'",
    )
    .fetch_one(&pool)
    .await
    .expect("post-game black rating");
    assert!(post_black_rating < 1500.0);

    // Two rating-history rows (one per device).
    let history_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM rating_history")
        .fetch_one(&pool)
        .await
        .expect("count history");
    assert_eq!(history_count, 2, "exactly two rating history rows");

    // The finished game row carries the resignation.
    let (status_str, winner_str, movecount): (String, Option<String>, i64) = sqlx::query_as(
        "SELECT status, winner, move_count FROM games",
    )
    .fetch_one(&pool)
    .await
    .expect("game row");
    assert_eq!(status_str, "resigned");
    assert_eq!(winner_str, Some("white".into()));
    assert_eq!(movecount, 0, "no moves played before the resignation");

    // Clean up.
    drop(restarted_app);
    drop(pool);
    std::fs::remove_dir_all(&dir).ok();
}

/// A registration writes the account, its first device link, and a session,
/// and a second device that logs in is linked to the same account and gets a
/// session of its own (spec "Device-to-Account Profile Linkage", "Accounts and
/// sessions survive a server restart").
#[tokio::test]
async fn an_account_its_device_links_and_its_sessions_survive_a_restart() {
    let db = TempDb::new();
    let (addr, _app) = spawn_app(&db.url).await;

    // Phase 1: register on the phone.
    let mut phone = connect(addr).await;
    let (account_id, phone_token) = session(
        &send_and_answer(
            &mut phone,
            &ClientMessage::Register {
                v: 1,
                username: "Ana".into(),
                password: "correct horse battery".into(),
                device_id: Some("device-phone".into()),
            },
        )
        .await,
    );

    // A second device logs in to the same account: it is linked and gets its
    // own session, and the first session keeps working.
    let mut tablet = connect(addr).await;
    let (tablet_account_id, tablet_token) = session(
        &send_and_answer(
            &mut tablet,
            &ClientMessage::Login {
                v: 1,
                username: "ana".into(),
                password: "correct horse battery".into(),
                device_id: Some("device-tablet".into()),
            },
        )
        .await,
    );
    assert_eq!(
        tablet_account_id, account_id,
        "a differently cased username is the same account"
    );
    assert_ne!(
        tablet_token, phone_token,
        "each session carries its own token"
    );

    // Durable side, read back from the file: one account, two device links,
    // two sessions, and no plaintext password anywhere.
    assert_eq!(db.count("SELECT COUNT(*) FROM accounts").await, 1);
    assert_eq!(db.count("SELECT COUNT(*) FROM sessions").await, 2);
    assert!(
        db.wait_for_count(
            &format!("SELECT COUNT(*) FROM profiles WHERE account_id = '{account_id}'"),
            2,
            Duration::from_secs(5),
        )
        .await,
        "both devices are linked to the account"
    );
    assert!(
        db.wait_for_text(
            "SELECT account_id FROM profiles WHERE device_id = 'device-tablet'",
            &account_id,
            Duration::from_secs(5),
        )
        .await,
        "the second device's link is durable and points at the account"
    );
    let stored_hash = db
        .text("SELECT password_hash FROM accounts")
        .await
        .expect("the account row");
    assert_ne!(
        stored_hash, "correct horse battery",
        "the password column is not the plaintext"
    );
    assert!(
        stored_hash.starts_with("$argon2"),
        "the password column is an argon2id hash, got {stored_hash:?}"
    );

    // A device that moves to another account is relinked, not duplicated.
    let mut shared = connect(addr).await;
    let (other_account_id, _) = session(
        &send_and_answer(
            &mut shared,
            &ClientMessage::Register {
                v: 1,
                username: "Bob".into(),
                password: "another long password".into(),
                device_id: Some("device-phone".into()),
            },
        )
        .await,
    );
    assert_ne!(other_account_id, account_id);
    assert!(
        db.wait_for_text(
            "SELECT account_id FROM profiles WHERE device_id = 'device-phone'",
            &other_account_id,
            Duration::from_secs(5),
        )
        .await,
        "a device holds one account at a time"
    );
    assert_eq!(
        db.count("SELECT COUNT(*) FROM profiles").await,
        2,
        "the relinked device kept its row: one row per device, not per link"
    );

    // Phase 2: restart on the same file and prove the durable authentication
    // state came back.
    drop(phone);
    drop(tablet);
    drop(shared);
    let restarted = Arc::new(App::init(auth_config(&db.url)).await.expect("restart init"));
    assert_eq!(restarted.auth().account_count(), 2);
    assert_eq!(restarted.auth().device_count(), 2);
    assert_eq!(
        restarted.auth().session_count(),
        3,
        "the three sessions issued before the restart were loaded, not reissued"
    );
    drop(restarted);

    let (restarted_addr, restarted_app) = spawn_app(&db.url).await;

    // A token issued before the restart still authenticates: the account it
    // resolves to is the one that owns the tablet's device link.
    let mut phone_again = connect(restarted_addr).await;
    let (code, _) = create_room_as(&mut phone_again, "device-phone", Some(&phone_token)).await;
    assert!(!code.is_empty());

    // And it is the *account* that now holds the room, not the device: a
    // second device of the same account cannot open one.
    let mut laptop = connect(restarted_addr).await;
    let refusal = send_and_answer(
        &mut laptop,
        &ClientMessage::CreateRoom {
            v: 1,
            player_id: "device-laptop".into(),
            time_control: None,
            token: Some(tablet_token),
        },
    )
    .await;
    assert_eq!(
        error_code(&refusal),
        "already_in_room",
        "the account's room limit survived the restart"
    );
    drop(restarted_app);
}

/// A failed authentication write is logged and costs the player nothing: the
/// answer still arrives and the connection can still create a room and be
/// joined (spec "A failed authentication write does not block play").
#[tokio::test]
async fn a_failed_authentication_write_is_logged_and_leaves_the_player_able_to_play() {
    let db = TempDb::new();
    let (addr, _app) = spawn_app(&db.url).await;

    let capture = LogCapture::default();
    let _ = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(capture.clone())
        .try_init();

    // Break every authentication table after startup, so the recorder's
    // inserts fail for real instead of being faked by a mock.
    let pool = db.pool().await;
    for table in ["accounts", "profiles", "sessions"] {
        sqlx::query(&format!("DROP TABLE {table}"))
            .execute(&pool)
            .await
            .unwrap_or_else(|err| panic!("drop {table}: {err}"));
    }
    drop(pool);

    // The registration is answered normally even though nothing could be
    // written.
    let mut client = connect(addr).await;
    let (account_id, _token) = session(
        &send_and_answer(
            &mut client,
            &ClientMessage::Register {
                v: 1,
                username: "Ana".into(),
                password: "correct horse battery".into(),
                device_id: Some("device-a".into()),
            },
        )
        .await,
    );
    assert!(
        log_contains(
            &capture,
            "failed to persist an authentication change",
            Duration::from_secs(5)
        )
        .await,
        "the failed account commit was logged; captured log:\n{}",
        capture.text()
    );

    // And the player is still a player: the same connection can open a room
    // and be joined, exactly as a guest could.
    let (code, state) = create_room_as(&mut client, "device-a", None).await;
    assert_eq!(state.your_color, Color::White);
    let mut opponent = connect(addr).await;
    let joined = send_and_answer(
        &mut opponent,
        &ClientMessage::JoinRoom {
            v: 1,
            player_id: "device-b".into(),
            room_code: code,
            token: None,
        },
    )
    .await;
    match joined {
        ServerMessage::RoomReady { state, .. } => {
            assert_eq!(state.your_color, Color::Black);
        }
        other => panic!("expected room_ready for the opponent, got {other:?}"),
    }

    // The log records the failure against the account it was for, and never
    // echoes the credential.
    let log = capture.text();
    assert!(
        log.contains(&account_id),
        "the failure names the account it belongs to"
    );
    assert!(
        !log.contains("correct horse battery"),
        "the log never carries the password"
    );
}

async fn spawn_app(url: &str) -> (SocketAddr, Arc<App>) {
    let app = Arc::new(
        App::init(auth_config(url))
            .await
            .expect("App::init with a temp auth database"),
    );
    let router = axum::Router::new()
        .route("/healthz", axum::routing::get(chess_server::infrastructure::ws::healthz))
        .route("/ws", axum::routing::get(chess_server::infrastructure::ws::ws_upgrade))
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
