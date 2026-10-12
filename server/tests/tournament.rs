//! Arena tournament integration tests (add-tournaments phase 1, server-only):
//! driven through `App` + channels, the same style as `tests/rooms.rs` — no
//! sockets. Covers the two things that matter most: a real pairing produces a
//! playable room, and a tournament game's result lands as tournament points,
//! never as a Glicko-2 rating change.

use std::sync::Arc;
use std::time::Duration;

use chess_server::application::room_actor::RoomMsg;
use chess_server::domain::time_control::TimeControl;
use chess_server::infrastructure::config::Config;
use chess_server::infrastructure::password::now_ms;
use chess_server::infrastructure::server::{App, Conn};
use chess_server::interface::protocol::ServerMessage;
use tokio::sync::mpsc;

const SHORT_CONTROL: TimeControl = TimeControl {
    base_ms: 60_000,
    increment_ms: 0,
};

fn app_with_grace(grace_secs: u64) -> Arc<App> {
    let config = Config::for_test(
        "127.0.0.1".parse().unwrap(),
        0,
        Duration::from_secs(grace_secs),
    );
    Arc::new(App::new(config))
}

/// Register a player in a tournament's waiting pool and return the channel
/// its pairing arrives on, mirroring what `ws.rs`'s `JoinTournament` handler
/// does.
fn join_tournament(app: &Arc<App>, player_id: &str, tournament_id: &str) -> mpsc::UnboundedReceiver<Conn> {
    let (tx, rx) = mpsc::unbounded_channel();
    app.join_tournament(player_id, tournament_id, tx)
        .expect("join tournament");
    rx
}

async fn next_conn(rx: &mut mpsc::UnboundedReceiver<Conn>) -> Conn {
    tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("paired within 5s")
        .expect("pairing channel closed before pairing")
}

async fn next_message(rx: &mut mpsc::UnboundedReceiver<ServerMessage>) -> ServerMessage {
    rx.recv()
        .await
        .expect("outbound channel closed while a message was expected")
}

// ---------------------------------------------------------------------------
// Pairing (tasks 2/3 of add-tournaments phase 1)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn two_waiting_players_get_paired_into_a_real_game() {
    let app = app_with_grace(30);
    let now = now_ms();
    let tournament_id = app.create_tournament(SHORT_CONTROL, now, 60 * 60_000);

    let mut a_wait = join_tournament(&app, "device-a", &tournament_id);
    // The first joiner has nobody to pair with yet.
    assert!(
        tokio::time::timeout(Duration::from_millis(200), a_wait.recv())
            .await
            .is_err(),
        "a lone waiting player must not be paired with itself"
    );

    let mut b_wait = join_tournament(&app, "device-b", &tournament_id);
    // The second joiner's arrival pairs both (event-driven, no tick needed).
    let mut conn_a = next_conn(&mut a_wait).await;
    let mut conn_b = next_conn(&mut b_wait).await;

    assert_eq!(conn_a.code, conn_b.code, "both seats share one room");
    assert!(!conn_a.code.is_empty());

    // Each side's own `Connect` reply is a real, playable room (`room_ready`
    // was already consumed to build `tournament_pairing` on the wire path in
    // `ws.rs`; driven through `App` directly here, it is still queued).
    match next_message(&mut conn_a.out_rx).await {
        ServerMessage::RoomReady { room_code, .. } => assert_eq!(room_code, conn_a.code),
        other => panic!("expected room_ready, got {other:?}"),
    }
    match next_message(&mut conn_b.out_rx).await {
        ServerMessage::RoomReady { room_code, .. } => assert_eq!(room_code, conn_b.code),
        other => panic!("expected room_ready, got {other:?}"),
    }
    // The creator (white) gets one more snapshot once the joiner (black)
    // seats, announcing the opponent is now online (same as `rooms.rs`'s
    // `start_game` helper drains it as `join_snapshot_a`).
    let _ = next_message(&mut conn_a.out_rx).await;

    // The room is a genuine playable game: a move from White is applied and
    // reflected back (`Config::for_test`'s default is full-state snapshots,
    // not incremental updates — same convention `tests/rooms.rs` uses).
    let _ = conn_a.mailbox.send(RoomMsg::Move {
        identity: conn_a.identity.clone(),
        uci: "e2e4".to_string(),
    });
    match next_message(&mut conn_a.out_rx).await {
        ServerMessage::State { state, .. } => {
            assert_eq!(state.move_list, vec!["e2e4".to_string()]);
        }
        other => panic!("expected a state snapshot, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Tournament points vs. Glicko-2 rating (the decision this phase exists to
// enforce: separate, never both)
// ---------------------------------------------------------------------------

/// A throwaway database file, removed when the test ends (same pattern as
/// `tests/persistence.rs`'s `TempDb`).
struct TempDb {
    dir: std::path::PathBuf,
    url: String,
}

impl TempDb {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("chess-tournament-db-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let url = format!("sqlite:{}/tournament.db", dir.display());
        Self { dir, url }
    }

    async fn pool(&self) -> sqlx::SqlitePool {
        sqlx::SqlitePool::connect(&self.url)
            .await
            .expect("open the test database")
    }

    async fn standing(&self, tournament_id: &str, device_id: &str) -> Option<(i64, i64)> {
        let pool = self.pool().await;
        let row: Option<(i64, i64)> = sqlx::query_as(
            "SELECT points, games_played FROM tournament_standings WHERE tournament_id = ? AND device_id = ?",
        )
        .bind(tournament_id)
        .bind(device_id)
        .fetch_optional(&pool)
        .await
        .expect("query tournament_standings");
        pool.close().await;
        row
    }

    /// Polls until a standing row exists, so an async recorder write handed
    /// off by the room actor is waited for rather than raced.
    async fn wait_for_standing(&self, tournament_id: &str, device_id: &str, budget: Duration) -> Option<(i64, i64)> {
        let deadline = std::time::Instant::now() + budget;
        loop {
            if let Some(row) = self.standing(tournament_id, device_id).await {
                return Some(row);
            }
            if std::time::Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[tokio::test]
async fn a_finished_tournament_game_updates_standings_and_never_touches_rating() {
    let db = TempDb::new();
    let app = Arc::new(
        App::init(Config::with_database(&db.url))
            .await
            .expect("init app with a real database"),
    );

    let now = now_ms();
    let tournament_id = app.create_tournament(SHORT_CONTROL, now, 60 * 60_000);

    let mut a_wait = join_tournament(&app, "device-a", &tournament_id);
    let mut b_wait = join_tournament(&app, "device-b", &tournament_id);
    let mut conn_a = next_conn(&mut a_wait).await;
    let mut conn_b = next_conn(&mut b_wait).await;
    let _ = next_message(&mut conn_a.out_rx).await; // room_ready
    let _ = next_message(&mut conn_b.out_rx).await; // room_ready
    let _ = next_message(&mut conn_a.out_rx).await; // opponent-online snapshot (creator only)

    // White resigns: Black wins (points 2-0 doubled), no moves played.
    let _ = conn_a.mailbox.send(RoomMsg::Resign {
        identity: conn_a.identity.clone(),
    });
    let _ = next_message(&mut conn_a.out_rx).await; // terminal update
    let _ = next_message(&mut conn_b.out_rx).await; // terminal update

    // Standings are written asynchronously (same fire-and-forget style as
    // every other recorder call in this codebase); poll for it.
    let winner_standing = db
        .wait_for_standing(&tournament_id, "device-b", Duration::from_secs(2))
        .await
        .expect("the winner's standing is recorded");
    assert_eq!(winner_standing, (2, 1), "a decisive win is worth 2 points, 1 game played");

    let loser_standing = db
        .wait_for_standing(&tournament_id, "device-a", Duration::from_secs(2))
        .await
        .expect("the loser's standing is recorded");
    assert_eq!(loser_standing, (0, 1), "a decisive loss is worth 0 points, 1 game played");

    // The critical regression check: neither player's Glicko-2 rating ever
    // *changed*. `rating_of` lazily materializes a default-rating session
    // for display purposes (every snapshot carries `white_rating`/
    // `black_rating`, win or lose, tournament or not) — that alone is not
    // evidence of a rating update. `apply_terminal_ratings` (the only thing
    // that would move it away from the default) must never have run.
    use chess_server::domain::rating::DEFAULT_RATING;
    assert_eq!(
        app.ratings().rating_of("device-a"),
        DEFAULT_RATING,
        "a tournament loss must not move the loser's rating"
    );
    assert_eq!(
        app.ratings().rating_of("device-b"),
        DEFAULT_RATING,
        "a tournament win must not move the winner's rating"
    );

    // And the `games`/`rating_history` tables (the casual-game path) stay
    // empty too — the only row this game wrote is in `tournament_games`.
    let pool = db.pool().await;
    let games_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM games")
        .fetch_one(&pool)
        .await
        .expect("games count");
    assert_eq!(games_count, 0, "a tournament game must not write a casual `games` row");
    let tournament_games_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tournament_games")
        .fetch_one(&pool)
        .await
        .expect("tournament_games count");
    assert_eq!(tournament_games_count, 1);
    pool.close().await;
}

// ---------------------------------------------------------------------------
// The arena window ends on schedule
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_tournament_ends_at_its_scheduled_time() {
    let app = app_with_grace(30);
    // Already in the past: the actor's first tick (every 2s) must observe
    // `is_over` immediately and end the tournament without needing a player.
    let now = now_ms();
    let tournament_id = app.create_tournament(SHORT_CONTROL, now - 120_000, 60_000);

    tokio::time::sleep(Duration::from_secs(3)).await;

    let summary = app
        .list_tournaments()
        .into_iter()
        .find(|t| t.tournament_id == tournament_id)
        .expect("the tournament is still listed after it ends");
    assert_eq!(summary.status, "finished");
}
