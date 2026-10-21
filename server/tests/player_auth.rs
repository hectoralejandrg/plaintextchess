//! The email flows, end to end over real sockets (lichess-like-auth tasks
//! 4.x-9.x): registering with an email address, signing in with either the
//! username or the address, resuming a stored session, resetting a password
//! by email, confirming an address, changing an address, and the attempt
//! limits.
//!
//! The mail transport is a recorder: the test reads the messages the server
//! delivered, which is what a player would receive in their inbox. The
//! production path (a provider, or the log transport) is the same code with
//! a different transport.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chess_server::application::ports::{EmailError, EmailMessage, EmailSender};
use chess_server::infrastructure::config::Config;
use chess_server::infrastructure::server::App;
use chess_server::interface::protocol::{ClientMessage, ServerMessage, VERSION};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::{Message, Utf8Bytes};
use tokio_tungstenite::WebSocketStream;

type WsClient = WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// A transport that keeps every message it was given.
#[derive(Debug, Default)]
struct RecordingEmailSender {
    sent: Mutex<Vec<EmailMessage>>,
}

impl EmailSender for RecordingEmailSender {
    fn send<'a>(
        &'a self,
        message: &'a EmailMessage,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), EmailError>> + Send + 'a>>
    {
        Box::pin(async move {
            self.sent
                .lock()
                .expect("the recorder is not poisoned")
                .push(message.clone());
            Ok(())
        })
    }
}

/// The token the last delivered message carries.
fn token_in(message: &EmailMessage) -> String {
    let marker = "token=";
    let start = message
        .text_body
        .find(marker)
        .expect("the message carries a link with a token");
    message.text_body[start + marker.len()..]
        .split_whitespace()
        .next()
        .expect("the token is the first thing after the marker")
        .to_string()
}

/// A throwaway database file, removed on drop.
struct TempDb {
    dir: std::path::PathBuf,
    url: String,
}

impl TempDb {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("chess-player-auth-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let url = format!("sqlite:{}/auth.db", dir.display());
        Self { dir, url }
    }

    async fn count(&self, sql: &str) -> i64 {
        let pool = sqlx::SqlitePool::connect(&self.url).await.expect("open");
        let value: i64 = sqlx::query_scalar(sql).fetch_one(&pool).await.expect(sql);
        pool.close().await;
        value
    }

    async fn text(&self, sql: &str) -> Option<String> {
        let pool = sqlx::SqlitePool::connect(&self.url).await.expect("open");
        let value: Option<String> = sqlx::query_scalar(sql).fetch_one(&pool).await.expect(sql);
        pool.close().await;
        value
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn config_for(url: &str) -> Config {
    Config {
        database_url: Some(url.to_string()),
        public_base_url: "https://play.example.com".to_string(),
        ..Config::for_test("127.0.0.1".parse().unwrap(), 0, Duration::from_secs(30))
    }
}

async fn spawn_server(
    url: &str,
    sender: Arc<RecordingEmailSender>,
) -> (SocketAddr, Arc<App>) {
    let app = Arc::new(
        App::init_with_email_sender(config_for(url), sender)
            .await
            .expect("App::init"),
    );
    let router = axum::Router::new()
        .route(
            "/healthz",
            axum::routing::get(chess_server::infrastructure::ws::healthz_with_features),
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
        let frame = client.next().await.expect("a frame").expect("a message");
        let text = match frame {
            Message::Text(text) => text,
            Message::Close(_) => panic!("the server closed the connection"),
            other => {
                let _ = other;
                continue;
            }
        };
        let value: serde_json::Value = serde_json::from_str(&text).expect("valid json");
        if value["type"] == "error" {
            return ServerMessage::Error {
                v: VERSION,
                code: value["code"].as_str().unwrap_or_default().to_string(),
                message: value["message"].as_str().unwrap_or_default().to_string(),
                field: value
                    .get("field")
                    .and_then(|field| field.as_str())
                    .map(str::to_string),
            };
        }
        return serde_json::from_value(value).expect("a known server message");
    }
}

fn session_of(message: &ServerMessage) -> (String, String) {
    match message {
        ServerMessage::Session {
            account_id,
            username,
            token,
            ..
        } => (account_id.clone(), token.clone()),
        other => panic!("expected a session, got {other:?}"),
    }
}

fn error_of(message: &ServerMessage) -> (String, String, Option<String>) {
    match message {
        ServerMessage::Error {
            code, message, field, ..
        } => (code.clone(), message.clone(), field.clone()),
        other => panic!("expected an error, got {other:?}"),
    }
}

async fn register(
    client: &mut WsClient,
    username: &str,
    email: &str,
    password: &str,
) -> ServerMessage {
    send(
        client,
        &ClientMessage::Register {
            v: VERSION,
            username: username.into(),
            email: Some(email.into()),
            password: password.into(),
            device_id: Some("device-a".into()),
        },
    )
    .await;
    next_server_message(client).await
}

async fn login(
    client: &mut WsClient,
    identifier: &str,
    password: &str,
) -> ServerMessage {
    send(
        client,
        &ClientMessage::Login {
            v: VERSION,
            username: None,
            identifier: Some(identifier.into()),
            password: password.into(),
            device_id: Some("device-a".into()),
        },
    )
    .await;
    next_server_message(client).await
}

#[tokio::test]
async fn a_registration_stores_the_email_and_issues_a_session() {
    let db = TempDb::new();
    let sender = Arc::new(RecordingEmailSender::default());
    let (addr, _app) = spawn_server(&db.url, Arc::clone(&sender)).await;

    let mut client = connect(addr).await;
    let answer = register(
        &mut client,
        "Ana",
        "ana@example.com",
        "correct horse battery",
    )
    .await;
    let (account_id, token) = session_of(&answer);
    assert!(!account_id.is_empty());
    assert!(!token.is_empty());

    // The address is stored, and the account starts unverified.
    assert_eq!(
        db.text("SELECT email FROM accounts WHERE username_key = 'ana'")
            .await
            .as_deref(),
        Some("ana@example.com")
    );
    assert_eq!(
        db.count("SELECT COUNT(*) FROM accounts WHERE email_verified = 0")
            .await,
        1
    );

    // The verification message was delivered, and it carries a token.
    let delivered = sender.sent.lock().expect("the recorder is not poisoned").clone();
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].to, "ana@example.com");
    assert!(delivered[0].text_body.contains("token="));
}

#[tokio::test]
async fn a_duplicate_email_is_refused() {
    let db = TempDb::new();
    let sender = Arc::new(RecordingEmailSender::default());
    let (addr, _app) = spawn_server(&db.url, Arc::clone(&sender)).await;

    let mut first = connect(addr).await;
    let _ = register(&mut first, "ana", "ana@example.com", "correct horse battery").await;

    let mut second = connect(addr).await;
    let answer = register(
        &mut second,
        "bob",
        "ANA@example.com",
        "correct horse battery",
    )
    .await;
    let (code, _message, _field) = error_of(&answer);
    assert_eq!(code, "email_taken");
    assert_eq!(db.count("SELECT COUNT(*) FROM accounts").await, 1);
}

#[tokio::test]
async fn a_player_can_sign_in_with_the_email_address() {
    let db = TempDb::new();
    let sender = Arc::new(RecordingEmailSender::default());
    let (addr, _app) = spawn_server(&db.url, Arc::clone(&sender)).await;

    let mut setup = connect(addr).await;
    let _ = register(&mut setup, "ana", "ana@example.com", "correct horse battery").await;

    let mut client = connect(addr).await;
    let answer = login(&mut client, "ana@example.com", "correct horse battery").await;
    let (_account_id, token) = session_of(&answer);
    assert!(!token.is_empty());
}

#[tokio::test]
async fn a_stored_token_resumes_the_session_without_a_new_token() {
    let db = TempDb::new();
    let sender = Arc::new(RecordingEmailSender::default());
    let (addr, _app) = spawn_server(&db.url, Arc::clone(&sender)).await;

    let mut setup = connect(addr).await;
    let (_account_id, token) = session_of(
        &register(&mut setup, "ana", "ana@example.com", "correct horse battery").await,
    );

    // A fresh connection presents the stored token.
    let mut resumed = connect(addr).await;
    send(
        &mut resumed,
        &ClientMessage::ResumeSession {
            v: VERSION,
            token: token.clone(),
        },
    )
    .await;
    let answer = next_server_message(&mut resumed).await;
    match answer {
        ServerMessage::SessionResumed {
            username,
            email_verified,
            ..
        } => {
            assert_eq!(username, "ana");
            assert!(!email_verified, "the address is not confirmed yet");
        }
        other => panic!("expected a resumed session, got {other:?}"),
    }
}

#[tokio::test]
async fn a_stale_token_is_refused_and_the_connection_stays_open() {
    let db = TempDb::new();
    let sender = Arc::new(RecordingEmailSender::default());
    let (addr, _app) = spawn_server(&db.url, Arc::clone(&sender)).await;

    let mut client = connect(addr).await;
    send(
        &mut client,
        &ClientMessage::ResumeSession {
            v: VERSION,
            token: "a-token-the-server-never-issued".into(),
        },
    )
    .await;
    let answer = next_server_message(&mut client).await;
    let (code, _message, _field) = error_of(&answer);
    assert_eq!(code, "not_authenticated");

    // The connection is still usable: the player can play as a guest.
    send(
        &mut client,
        &ClientMessage::CreateRoom {
            v: VERSION,
            player_id: "device-a".into(),
            time_control: None,
            token: None,
        },
    )
    .await;
    let answer = next_server_message(&mut client).await;
    assert!(
        matches!(answer, ServerMessage::RoomReady { .. }),
        "the connection stays open: {answer:?}"
    );
}

#[tokio::test]
async fn a_reset_request_delivers_a_token_and_the_new_password_works() {
    let db = TempDb::new();
    let sender = Arc::new(RecordingEmailSender::default());
    let (addr, _app) = spawn_server(&db.url, Arc::clone(&sender)).await;

    let mut setup = connect(addr).await;
    let _ = register(&mut setup, "ana", "ana@example.com", "correct horse battery").await;

    // Ask for a reset.
    let mut client = connect(addr).await;
    send(
        &mut client,
        &ClientMessage::ForgotPassword {
            v: VERSION,
            email: "ana@example.com".into(),
        },
    )
    .await;
    let answer = next_server_message(&mut client).await;
    assert!(
        matches!(answer, ServerMessage::PasswordResetRequested { .. }),
        "the reset is acknowledged: {answer:?}"
    );

    // The message carries the token.
    let delivered = sender.sent.lock().expect("the recorder is not poisoned").clone();
    let reset = delivered
        .iter()
        .find(|message| message.subject.starts_with("Reset"))
        .expect("a reset message was delivered");
    let token = token_in(reset);

    // Set a new password with it.
    let mut reset_client = connect(addr).await;
    send(
        &mut reset_client,
        &ClientMessage::ResetPassword {
            v: VERSION,
            token: token.clone(),
            password: "a brand new password".into(),
        },
    )
    .await;
    let answer = next_server_message(&mut reset_client).await;
    assert!(
        matches!(answer, ServerMessage::PasswordReset { .. }),
        "the password was reset: {answer:?}"
    );

    // The new password signs in; the old one does not.
    let mut client = connect(addr).await;
    let answer = login(&mut client, "ana", "a brand new password").await;
    let (_account_id, _token) = session_of(&answer);
    let mut other = connect(addr).await;
    let answer = login(&mut other, "ana", "correct horse battery").await;
    let (code, _message, _field) = error_of(&answer);
    assert_eq!(code, "invalid_credentials");
}

#[tokio::test]
async fn a_reset_token_works_only_once() {
    let db = TempDb::new();
    let sender = Arc::new(RecordingEmailSender::default());
    let (addr, _app) = spawn_server(&db.url, Arc::clone(&sender)).await;

    let mut setup = connect(addr).await;
    let _ = register(&mut setup, "ana", "ana@example.com", "correct horse battery").await;

    let mut client = connect(addr).await;
    send(
        &mut client,
        &ClientMessage::ForgotPassword {
            v: VERSION,
            email: "ana@example.com".into(),
        },
    )
    .await;
    let _ = next_server_message(&mut client).await;
    let delivered = sender.sent.lock().expect("the recorder is not poisoned").clone();
    let token = token_in(
        delivered
            .iter()
            .find(|message| message.subject.starts_with("Reset"))
            .expect("a reset message was delivered"),
    );

    let mut first = connect(addr).await;
    send(
        &mut first,
        &ClientMessage::ResetPassword {
            v: VERSION,
            token: token.clone(),
            password: "a brand new password".into(),
        },
    )
    .await;
    assert!(matches!(
        next_server_message(&mut first).await,
        ServerMessage::PasswordReset { .. }
    ));

    let mut second = connect(addr).await;
    send(
        &mut second,
        &ClientMessage::ResetPassword {
            v: VERSION,
            token,
            password: "another new password".into(),
        },
    )
    .await;
    let answer = next_server_message(&mut second).await;
    let (code, _message, _field) = error_of(&answer);
    assert_eq!(code, "invalid_reset_token");
}

#[tokio::test]
async fn the_reset_answer_does_not_reveal_whether_the_address_exists() {
    let db = TempDb::new();
    let sender = Arc::new(RecordingEmailSender::default());
    let (addr, _app) = spawn_server(&db.url, Arc::clone(&sender)).await;

    let mut client = connect(addr).await;
    send(
        &mut client,
        &ClientMessage::ForgotPassword {
            v: VERSION,
            email: "nobody@example.com".into(),
        },
    )
    .await;
    let answer = next_server_message(&mut client).await;
    assert!(
        matches!(answer, ServerMessage::PasswordResetRequested { .. }),
        "the answer is the same one a registered address gets: {answer:?}"
    );
    assert!(
        sender.sent.lock().expect("the recorder is not poisoned").is_empty(),
        "nothing is delivered for an address no account holds"
    );
}

#[tokio::test]
async fn a_new_account_is_unverified_until_the_token_is_used() {
    let db = TempDb::new();
    let sender = Arc::new(RecordingEmailSender::default());
    let (addr, _app) = spawn_server(&db.url, Arc::clone(&sender)).await;

    let mut setup = connect(addr).await;
    let answer = register(
        &mut setup,
        "ana",
        "ana@example.com",
        "correct horse battery",
    )
    .await;
    match &answer {
        ServerMessage::Session { email_verified, .. } => assert!(!email_verified),
        other => panic!("expected a session, got {other:?}"),
    }

    let delivered = sender.sent.lock().expect("the recorder is not poisoned").clone();
    let token = token_in(
        delivered
            .iter()
            .find(|message| message.subject.starts_with("Confirm"))
            .expect("a verification message was delivered"),
    );

    let mut client = connect(addr).await;
    send(
        &mut client,
        &ClientMessage::VerifyEmail {
            v: VERSION,
            token: token.clone(),
        },
    )
    .await;
    let answer = next_server_message(&mut client).await;
    match &answer {
        ServerMessage::EmailVerified { email_verified, .. } => assert!(*email_verified),
        other => panic!("expected a confirmed address, got {other:?}"),
    }
    assert_eq!(
        db.count("SELECT COUNT(*) FROM accounts WHERE email_verified = 1")
            .await,
        1
    );

    // The token is spent.
    let mut again = connect(addr).await;
    send(
        &mut again,
        &ClientMessage::VerifyEmail {
            v: VERSION,
            token,
        },
    )
    .await;
    let answer = next_server_message(&mut again).await;
    let (code, _message, _field) = error_of(&answer);
    assert_eq!(code, "invalid_verification_token");
}

#[tokio::test]
async fn the_current_address_stays_in_force_until_the_new_one_is_confirmed() {
    let db = TempDb::new();
    let sender = Arc::new(RecordingEmailSender::default());
    let (addr, _app) = spawn_server(&db.url, Arc::clone(&sender)).await;

    let mut setup = connect(addr).await;
    let (_account_id, token) = session_of(
        &register(&mut setup, "ana", "ana@example.com", "correct horse battery").await,
    );

    let mut client = connect(addr).await;
    send(
        &mut client,
        &ClientMessage::ChangeEmail {
            v: VERSION,
            token: token.clone(),
            email: "new@example.com".into(),
        },
    )
    .await;
    let answer = next_server_message(&mut client).await;
    assert!(
        matches!(answer, ServerMessage::EmailVerificationRequested { .. }),
        "the change is acknowledged: {answer:?}"
    );

    // The old address is still the one in force.
    assert_eq!(
        db.text("SELECT email FROM accounts WHERE username_key = 'ana'")
            .await
            .as_deref(),
        Some("ana@example.com")
    );

    // An address another account holds is refused.
    let mut other = connect(addr).await;
    let _ = register(
        &mut other,
        "bob",
        "bob@example.com",
        "correct horse battery",
    )
    .await;
    let mut third = connect(addr).await;
    send(
        &mut third,
        &ClientMessage::ChangeEmail {
            v: VERSION,
            token,
            email: "bob@example.com".into(),
        },
    )
    .await;
    let answer = next_server_message(&mut third).await;
    let (code, _message, _field) = error_of(&answer);
    assert_eq!(code, "email_taken");
}

#[tokio::test]
async fn confirming_a_new_address_replaces_the_old_one() {
    let db = TempDb::new();
    let sender = Arc::new(RecordingEmailSender::default());
    let (addr, _app) = spawn_server(&db.url, Arc::clone(&sender)).await;

    let mut setup = connect(addr).await;
    let (_account_id, token) = session_of(
        &register(
            &mut setup,
            "ana",
            "ana@example.com",
            "correct horse battery",
        )
        .await,
    );

    // Ask for a different address.
    let mut client = connect(addr).await;
    send(
        &mut client,
        &ClientMessage::ChangeEmail {
            v: VERSION,
            token: token.clone(),
            email: "new@example.com".into(),
        },
    )
    .await;
    let answer = next_server_message(&mut client).await;
    assert!(
        matches!(answer, ServerMessage::EmailVerificationRequested { .. }),
        "the change is acknowledged: {answer:?}"
    );

    // The old address is still the one in force.
    assert_eq!(
        db.text("SELECT email FROM accounts WHERE username_key = 'ana'")
            .await
            .as_deref(),
        Some("ana@example.com")
    );

    // The confirmation message carries a token, and it is the one the
    // account's verification is done with.
    let delivered = sender.sent.lock().expect("the recorder is not poisoned").clone();
    let confirmation = delivered
        .iter()
        .filter(|message| message.to == "new@example.com")
        .next_back()
        .expect("a confirmation message went to the new address");
    let confirmation_token = token_in(confirmation);

    let mut confirm = connect(addr).await;
    send(
        &mut confirm,
        &ClientMessage::VerifyEmail {
            v: VERSION,
            token: confirmation_token,
        },
    )
    .await;
    let answer = next_server_message(&mut confirm).await;
    assert!(
        matches!(answer, ServerMessage::EmailVerified { .. }),
        "the address was confirmed: {answer:?}"
    );

    // The new address is the one in force, and it is verified.
    assert_eq!(
        db.text("SELECT email FROM accounts WHERE username_key = 'ana'")
            .await
            .as_deref(),
        Some("new@example.com")
    );
    assert_eq!(
        db.count("SELECT COUNT(*) FROM accounts WHERE email_verified = 1")
            .await,
        1
    );
    assert_eq!(
        db.count("SELECT COUNT(*) FROM accounts WHERE pending_email IS NOT NULL")
            .await,
        0,
        "no pending address is left behind"
    );

    // The new address signs in.
    let mut sign_in = connect(addr).await;
    let answer = login(&mut sign_in, "new@example.com", "correct horse battery").await;
    let (_account_id, _token) = session_of(&answer);
}

/// A token confirms the address it was mailed to (spec "Email Change").
#[tokio::test]
async fn a_token_confirms_only_the_address_it_was_sent_to() {
    let db = TempDb::new();
    let sender = Arc::new(RecordingEmailSender::default());
    let (addr, _app) = spawn_server(&db.url, Arc::clone(&sender)).await;

    let mut setup = connect(addr).await;
    let (_account_id, token) = session_of(
        &register(
            &mut setup,
            "ana",
            "ana@example.com",
            "correct horse battery",
        )
        .await,
    );

    // A change to another address: the confirmation goes to the new one.
    let mut client = connect(addr).await;
    send(
        &mut client,
        &ClientMessage::ChangeEmail {
            v: VERSION,
            token: token.clone(),
            email: "new@example.com".into(),
        },
    )
    .await;
    let _ = next_server_message(&mut client).await;

    // A resend goes to the address that is being confirmed, and it retires
    // the token that was sent there.
    send(
        &mut client,
        &ClientMessage::ResendVerification {
            v: VERSION,
            token: token.clone(),
        },
    )
    .await;
    let _ = next_server_message(&mut client).await;

    let delivered = sender.sent.lock().expect("the recorder is not poisoned").clone();
    // The messages that carried a token: the one for the old address (the
    // registration) and the ones for the new one.
    let old_token = delivered
        .iter()
        .find(|message| message.to == "ana@example.com")
        .map(token_in)
        .expect("a verification message went to the old address");
    let new_token = delivered
        .iter()
        .rev()
        .find(|message| message.to == "new@example.com")
        .map(token_in)
        .expect("a confirmation message went to the new address");

    // The token that was sent to the old address cannot confirm the new one.
    let mut wrong = connect(addr).await;
    send(
        &mut wrong,
        &ClientMessage::VerifyEmail {
            v: VERSION,
            token: old_token,
        },
    )
    .await;
    let answer = next_server_message(&mut wrong).await;
    let (code, _message, _field) = error_of(&answer);
    assert_eq!(code, "invalid_verification_token");
    assert_eq!(
        db.text("SELECT email FROM accounts WHERE username_key = 'ana'")
            .await
            .as_deref(),
        Some("ana@example.com"),
        "the address was not replaced"
    );

    // The token that was sent to the new address does.
    let mut right = connect(addr).await;
    send(
        &mut right,
        &ClientMessage::VerifyEmail {
            v: VERSION,
            token: new_token,
        },
    )
    .await;
    let answer = next_server_message(&mut right).await;
    assert!(
        matches!(answer, ServerMessage::EmailVerified { .. }),
        "the new address was confirmed: {answer:?}"
    );
    assert_eq!(
        db.text("SELECT email FROM accounts WHERE username_key = 'ana'")
            .await
            .as_deref(),
        Some("new@example.com")
    );
}

#[tokio::test]
async fn the_attempt_limit_is_enforced_and_says_when_to_try_again() {
    let db = TempDb::new();
    let sender = Arc::new(RecordingEmailSender::default());

    // One sign-in attempt per minute.
    let mut config = config_for(&db.url);
    config.auth_rate_limit_login_per_min = 1;
    let app = Arc::new(
        App::init_with_email_sender(config, Arc::clone(&sender) as Arc<dyn EmailSender>)
            .await
            .expect("App::init"),
    );
    let router = axum::Router::new()
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

    let mut setup = connect(addr).await;
    let _ = register(&mut setup, "ana", "ana@example.com", "correct horse battery").await;

    // The first sign-in attempt is inside the limit.
    let mut first = connect(addr).await;
    let answer = login(&mut first, "ana", "correct horse battery").await;
    let (_account_id, _token) = session_of(&answer);

    // The second is refused, and the refusal says when to try again.
    let mut second = connect(addr).await;
    let answer = login(&mut second, "ana", "correct horse battery").await;
    let (code, message, _field) = error_of(&answer);
    assert_eq!(code, "too_many_attempts");
    assert!(
        message.contains("try again"),
        "the refusal says when to try again: {message:?}"
    );
}

/// The readiness body, fetched with a plain HTTP request.
async fn fetch_health(addr: SocketAddr) -> serde_json::Value {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut stream = tokio::net::TcpStream::connect(addr)
        .await
        .expect("connect to the health endpoint");
    stream
        .write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .expect("send the request");
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .await
        .expect("read the response");
    let body = response
        .split("\r\n\r\n")
        .nth(1)
        .expect("the response has a body");
    serde_json::from_str(body).expect("a json body")
}

/// A server whose configuration the test chooses.
async fn spawn_server_with_config(
    config: Config,
    sender: Arc<RecordingEmailSender>,
) -> (SocketAddr, Arc<App>) {
    let app = Arc::new(
        App::init_with_email_sender(config, sender)
            .await
            .expect("App::init"),
    );
    let router = axum::Router::new()
        .route(
            "/healthz",
            axum::routing::get(chess_server::infrastructure::ws::healthz_with_features),
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

#[tokio::test]
async fn an_unknown_identifier_still_costs_a_password_verification() {
    let db = TempDb::new();
    let sender = Arc::new(RecordingEmailSender::default());

    // The default hashing cost, so the verification is expensive enough to
    // measure.
    let mut config = config_for(&db.url);
    config.argon2 = chess_server::infrastructure::config::Argon2Params::default();
    let (addr, _app) = spawn_server_with_config(config, Arc::clone(&sender)).await;

    let mut setup = connect(addr).await;
    let _ = register(
        &mut setup,
        "ana",
        "ana@example.com",
        "correct horse battery",
    )
    .await;

    // A wrong password for a real account: one argon2 pass.
    let mut wrong = connect(addr).await;
    let started = std::time::Instant::now();
    let _ = login(&mut wrong, "ana", "not the password").await;
    let wrong_password_cost = started.elapsed();

    // An identifier no account holds: the same work, so the reply does not
    // disclose whether the account exists.
    let mut unknown = connect(addr).await;
    let started = std::time::Instant::now();
    let answer = login(&mut unknown, "nobody@example.com", "not the password").await;
    let unknown_cost = started.elapsed();

    let (code, _message, _field) = error_of(&answer);
    assert_eq!(code, "invalid_credentials");
    // Both attempts did the work: the unknown identifier is not answered
    // straight from the lookup.
    assert!(
        unknown_cost > std::time::Duration::from_millis(10),
        "an unknown identifier still costs a verification: {unknown_cost:?}"
    );
    assert!(
        wrong_password_cost > std::time::Duration::from_millis(10),
        "a wrong password costs a verification: {wrong_password_cost:?}"
    );
    // The comparison is loose on purpose: the two attempts run in parallel
    // with the rest of the suite, and the point is that neither is free.
    assert!(
        unknown_cost >= wrong_password_cost / 4,
        "an unknown identifier must not be cheaper: {unknown_cost:?} against {wrong_password_cost:?}"
    );
}

#[tokio::test]
async fn a_refused_attempt_costs_no_password_hashing() {
    let db = TempDb::new();
    let sender = Arc::new(RecordingEmailSender::default());

    let mut config = config_for(&db.url);
    config.argon2 = chess_server::infrastructure::config::Argon2Params::default();
    config.auth_rate_limit_login_per_min = 1;
    let (addr, _app) = spawn_server_with_config(config, Arc::clone(&sender)).await;

    let mut setup = connect(addr).await;
    let _ = register(
        &mut setup,
        "ana",
        "ana@example.com",
        "correct horse battery",
    )
    .await;

    // The first attempt is inside the limit, and it hashes the password.
    let mut first = connect(addr).await;
    let started = std::time::Instant::now();
    let _ = login(&mut first, "ana", "correct horse battery").await;
    let allowed_cost = started.elapsed();
    assert!(
        allowed_cost > std::time::Duration::from_millis(10),
        "the allowed attempt hashes the password: {allowed_cost:?}"
    );

    // The second is refused before the password is looked at.
    let mut second = connect(addr).await;
    let started = std::time::Instant::now();
    let answer = login(&mut second, "ana", "correct horse battery").await;
    let refused_cost = started.elapsed();
    let (code, _message, _field) = error_of(&answer);
    assert_eq!(code, "too_many_attempts");
    assert!(
        refused_cost < allowed_cost / 2,
        "a refused attempt must not hash: {refused_cost:?} against {allowed_cost:?}"
    );
}

/// The guest path must survive every authentication change (spec "Guest Play
/// Fallback"): a connection with no session, an expired one, or a revoked one
/// still plays.
#[tokio::test]
async fn a_connection_without_a_usable_session_still_plays() {
    let db = TempDb::new();
    let sender = Arc::new(RecordingEmailSender::default());
    let (addr, _app) = spawn_server(&db.url, Arc::clone(&sender)).await;

    let mut setup = connect(addr).await;
    let (_account_id, token) = session_of(
        &register(
            &mut setup,
            "ana",
            "ana@example.com",
            "correct horse battery",
        )
        .await,
    );

    // A revoked session: the player signs out and keeps playing. Each
    // connection is its own device: a device holds one room at a time.
    let mut revoked = connect(addr).await;
    send(
        &mut revoked,
        &ClientMessage::Logout {
            v: VERSION,
            token: token.clone(),
        },
    )
    .await;
    let answer = next_server_message(&mut revoked).await;
    assert!(
        matches!(answer, ServerMessage::SessionOk { .. }),
        "the session was revoked: {answer:?}"
    );
    send(
        &mut revoked,
        &ClientMessage::CreateRoom {
            v: VERSION,
            player_id: "device-revoked".into(),
            time_control: None,
            token: Some(token.clone()),
        },
    )
    .await;
    let answer = next_server_message(&mut revoked).await;
    assert!(
        matches!(answer, ServerMessage::RoomReady { .. }),
        "a revoked session does not block play: {answer:?}"
    );

    // An unknown session: the same.
    let mut unknown = connect(addr).await;
    send(
        &mut unknown,
        &ClientMessage::CreateRoom {
            v: VERSION,
            player_id: "device-unknown".into(),
            time_control: None,
            token: Some("a-token-the-server-never-issued".into()),
        },
    )
    .await;
    let answer = next_server_message(&mut unknown).await;
    assert!(
        matches!(answer, ServerMessage::RoomReady { .. }),
        "an unknown session does not block play: {answer:?}"
    );

    // No session at all: the same.
    let mut guest = connect(addr).await;
    send(
        &mut guest,
        &ClientMessage::CreateRoom {
            v: VERSION,
            player_id: "device-guest".into(),
            time_control: None,
            token: None,
        },
    )
    .await;
    let answer = next_server_message(&mut guest).await;
    assert!(
        matches!(answer, ServerMessage::RoomReady { .. }),
        "a guest plays: {answer:?}"
    );
}

#[tokio::test]
async fn the_health_endpoint_reports_the_authentication_features() {
    let db = TempDb::new();
    let sender = Arc::new(RecordingEmailSender::default());
    let (addr, _app) = spawn_server(&db.url, Arc::clone(&sender)).await;

    let body: serde_json::Value = fetch_health(addr).await;

    let features = body["auth_features"]
        .as_array()
        .expect("the body names the authentication features");
    let names: Vec<&str> = features
        .iter()
        .filter_map(|value| value.as_str())
        .collect();
    for expected in [
        "password_reset",
        "email_verification",
        "email_change",
        "session_resumption",
    ] {
        assert!(
            names.contains(&expected),
            "the body names `{expected}`: {names:?}"
        );
    }
    assert!(
        !names.contains(&"breached_password_refusal"),
        "a feature that is turned off does not appear: {names:?}"
    );
}
