//! Finished-game recorders (spec "Server Persistence"): the durable
//! SQLite recorder (add-sqlite-persistence D3/D4/D5) and the no-op
//! recorder that keeps in-memory runs and tests behavior-identical.

use std::str::FromStr;
use std::time::Duration;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};

use crate::application::ports::{GameRecorder, RecorderError};
use crate::domain::game_record::FinishedGame;

/// The embedded versioned migrations (design D5): compiled from
/// `server/migrations/`, no runtime file lookups.
pub const MIGRATIONS: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Records nothing and always succeeds. Used when persistence is
/// disabled (`Config::for_test`, `App::new`).
#[derive(Default)]
pub struct NoopGameRecorder;

impl GameRecorder for NoopGameRecorder {
    fn record_finished_game<'a>(
        &'a self,
        _game: &'a FinishedGame,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), RecorderError>> + Send + 'a>>
    {
        Box::pin(async move { Ok(()) })
    }
}

/// Open the project's SQLite pool (design D5): a single connection (there is
/// exactly one writer — this process), WAL mode, and a 5-second busy
/// timeout.
///
/// Shared by the game recorder and the auth recorder so both adapters reach
/// the same database with the same settings (task 4.1). One pool means one
/// writer, which is also why argon2 must never run while a connection is held
/// (design D5).
pub async fn connect_pool(database_url: &str) -> Result<SqlitePool, RecorderError> {
    // sqlx's URL parsing leaves `create_if_missing` off unless the URL
    // carries `?mode=rwc`, so a first-boot open of a missing database
    // file would fail with SQLITE_CANTOPEN. Force it: opening the
    // persistence database must create the file. (Harmless for
    // read-only or in-memory URLs, where the open mode is decided by
    // the other flags.)
    let options = SqliteConnectOptions::from_str(database_url)
        .map_err(|e| format!("invalid database url `{database_url}`: {e}"))?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_millis(5000));
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .map_err(|e| format!("cannot open database `{database_url}`: {e}"))
}

/// The durable recorder: a local SQLite file behind a single-connection
/// pool (spec "Server Persistence", design D4/D5).
pub struct SqliteGameRecorder {
    pool: SqlitePool,
}

impl SqliteGameRecorder {
    /// Open the pool with the project's SQLite settings (design D5).
    pub async fn connect(database_url: &str) -> Result<SqlitePool, RecorderError> {
        connect_pool(database_url).await
    }

    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// One idempotent commit per finished game (design D4):
    /// 1. the game row (`ON CONFLICT DO NOTHING` — the unique
    ///    `game_id` is the idempotency key; a replayed commit is a no-op);
    /// 2. both players' post-game states, guarded by
    ///    `updated_at_ms < excluded.updated_at_ms` so a stale commit can
    ///    never overwrite a newer persisted state;
    /// 3. one rating-history row per player (`INSERT OR IGNORE`, PK
    ///    `(device_id, game_id)` — replay-safe).
    async fn commit(&self, game: &FinishedGame, ended_at_ms: i64) -> Result<(), RecorderError> {
        let mut tx = self.pool.begin().await.map_err(|e| e.to_string())?;

        sqlx::query(
            r#"INSERT INTO games (
                   game_id, room_code, time_control, white_device, black_device,
                   status, winner, move_count,
                   white_rating_before, black_rating_before,
                   white_rating_after, black_rating_after, ended_at_ms
               )
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
               ON CONFLICT (game_id) DO NOTHING"#,
        )
        .bind(&game.game_id)
        .bind(&game.room_code)
        .bind(&game.time_control)
        .bind(&game.white_device)
        .bind(&game.black_device)
        .bind(&game.status)
        .bind(&game.winner)
        .bind(i64::from(game.move_count))
        .bind(game.white_rating_before)
        .bind(game.black_rating_before)
        .bind(game.white_state.rating)
        .bind(game.black_state.rating)
        .bind(ended_at_ms)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

        for (device_id, state) in [
            (&game.white_device, &game.white_state),
            (&game.black_device, &game.black_state),
        ] {
            sqlx::query(
                r#"INSERT INTO players (
                       device_id, rating, rating_deviation, volatility, updated_at_ms
                   )
                   VALUES (?, ?, ?, ?, ?)
                   ON CONFLICT (device_id) DO UPDATE SET
                       rating = excluded.rating,
                       rating_deviation = excluded.rating_deviation,
                       volatility = excluded.volatility,
                       updated_at_ms = excluded.updated_at_ms
                   WHERE players.updated_at_ms < excluded.updated_at_ms"#,
            )
            .bind(device_id)
            .bind(state.rating)
            .bind(state.rating_deviation)
            .bind(state.volatility)
            .bind(ended_at_ms)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
        }

        for (device_id, rating_before, state) in [
            (&game.white_device, game.white_rating_before, &game.white_state),
            (&game.black_device, game.black_rating_before, &game.black_state),
        ] {
            sqlx::query(
                r#"INSERT OR IGNORE INTO rating_history
                       (device_id, game_id, rating_before, rating_after)
                   VALUES (?, ?, ?, ?)"#,
            )
            .bind(device_id)
            .bind(&game.game_id)
            .bind(rating_before)
            .bind(state.rating)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
        }

        tx.commit().await.map_err(|e| e.to_string())?;
        Ok(())
    }
}

impl GameRecorder for SqliteGameRecorder {
    fn record_finished_game<'a>(
        &'a self,
        game: &'a FinishedGame,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), RecorderError>> + Send + 'a>>
    {
        Box::pin(async move {
            // `ended_at_ms` is stamped at commit time (wall clock, unix
            // ms, design D4): the domain record stays timestamp-free.
            let ended_at_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock")
                .as_millis() as i64;
            self.commit(game, ended_at_ms).await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::rating::RatingState;

    /// A temp-file SQLite database with the migrations applied; the
    /// directory is removed on drop (best effort).
    struct TempDb {
        dir: std::path::PathBuf,
    }

    impl TempDb {
        async fn new() -> (Self, SqlitePool) {
            let dir = std::env::temp_dir().join(format!(
                "chess-recorder-test-{}",
                uuid::Uuid::new_v4()
            ));
            std::fs::create_dir_all(&dir).expect("create temp dir");
            let url = format!("sqlite:{}/recorder.db", dir.display());
            let pool = SqliteGameRecorder::connect(&url).await.expect("open temp db");
            MIGRATIONS.run(&pool).await.expect("run migrations");
            (Self { dir }, pool)
        }
    }

    impl Drop for TempDb {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn sample_game() -> FinishedGame {
        FinishedGame {
            game_id: uuid::Uuid::new_v4().to_string(),
            room_code: "ABCDEF".into(),
            time_control: "15+10".into(),
            white_device: "device-a".into(),
            black_device: "device-b".into(),
            status: "checkmated".into(),
            winner: Some("white".into()),
            move_count: 7,
            white_rating_before: 1500.0,
            black_rating_before: 1500.0,
            white_state: RatingState {
                rating: 1520.0,
                rating_deviation: 199.5,
                volatility: 0.05,
            },
            black_state: RatingState {
                rating: 1480.0,
                rating_deviation: 199.5,
                volatility: 0.05,
            },
        }
    }

    async fn table_digest(pool: &SqlitePool) -> String {
        let games: Vec<(String, String, Option<String>, i64)> =
            sqlx::query_as(
                "SELECT game_id, status, winner, move_count FROM games ORDER BY game_id",
            )
            .fetch_all(pool)
            .await
            .expect("games");
        let players: Vec<(String, f64, f64, f64)> = sqlx::query_as(
            "SELECT device_id, rating, rating_deviation, volatility FROM players ORDER BY device_id",
        )
        .fetch_all(pool)
        .await
        .expect("players");
        let history: Vec<(String, String, f64, f64)> = sqlx::query_as(
            "SELECT device_id, game_id, rating_before, rating_after FROM rating_history ORDER BY device_id, game_id",
        )
        .fetch_all(pool)
        .await
        .expect("history");
        format!("{games:?} {players:?} {history:?}")
    }

    #[tokio::test]
    async fn migrations_create_the_player_auth_tables() {
        let (_db, pool) = TempDb::new().await;
        for table in ["accounts", "profiles", "sessions"] {
            let count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?",
            )
            .bind(table)
            .fetch_one(&pool)
            .await
            .expect("sqlite_master");
            assert_eq!(count, 1, "migration must create `{table}`");
        }
    }

    #[tokio::test]
    async fn the_noop_recorder_records_nothing_and_succeeds() {
        let recorder = NoopGameRecorder;
        let game = sample_game();
        assert!(recorder.record_finished_game(&game).await.is_ok());
        assert!(recorder.record_finished_game(&game).await.is_ok());
    }

    #[tokio::test]
    async fn a_commit_writes_the_game_row_player_states_and_history() {
        let (_db, pool) = TempDb::new().await;
        let recorder = SqliteGameRecorder::new(pool.clone());
        let game = sample_game();

        // The public port path (stamps ended_at_ms itself).
        recorder.record_finished_game(&game).await.expect("commit");

        let (room_code, status, winner, move_count, white_after, black_after): (
            String,
            String,
            Option<String>,
            i64,
            f64,
            f64,
        ) = sqlx::query_as(
            "SELECT room_code, status, winner, move_count, white_rating_after, black_rating_after FROM games",
        )
        .fetch_one(&pool)
        .await
        .expect("game row");
        assert_eq!(room_code, "ABCDEF");
        assert_eq!(status, "checkmated");
        assert_eq!(winner, Some("white".into()));
        assert_eq!(move_count, 7);
        assert_eq!(white_after, 1520.0);
        assert_eq!(black_after, 1480.0);

        let (rating, deviation, volatility): (f64, f64, f64) = sqlx::query_as(
            "SELECT rating, rating_deviation, volatility FROM players WHERE device_id = 'device-a'",
        )
        .fetch_one(&pool)
        .await
        .expect("white player row");
        assert_eq!((rating, deviation, volatility), (1520.0, 199.5, 0.05));

        let (before, after): (f64, f64) = sqlx::query_as(
            "SELECT rating_before, rating_after FROM rating_history WHERE device_id = 'device-b'",
        )
        .fetch_one(&pool)
        .await
        .expect("black history row");
        assert_eq!(before, 1500.0);
        assert_eq!(after, 1480.0);

        let history_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM rating_history")
            .fetch_one(&pool)
            .await
            .expect("history count");
        assert_eq!(history_count, 2);
    }

    #[tokio::test]
    async fn committing_the_same_game_twice_changes_nothing() {
        let (_db, pool) = TempDb::new().await;
        let recorder = SqliteGameRecorder::new(pool.clone());
        let game = sample_game();

        recorder.commit(&game, 1000).await.expect("first commit");
        let digest_one = table_digest(&pool).await;

        // A retried/replayed commit of the same finished game (for
        // example after a crash): the unique `game_id` makes it a no-op.
        recorder.commit(&game, 2000).await.expect("second commit");
        let digest_two = table_digest(&pool).await;

        assert_eq!(
            digest_one, digest_two,
            "a replayed commit must leave every table unchanged"
        );

        let game_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM games")
            .fetch_one(&pool)
            .await
            .expect("game count");
        assert_eq!(game_count, 1);
    }

    #[tokio::test]
    async fn a_stale_commit_does_not_overwrite_a_newer_persisted_state() {
        let (_db, pool) = TempDb::new().await;
        let recorder = SqliteGameRecorder::new(pool.clone());
        let game = sample_game();

        // A newer persisted state already exists (far-future timestamp).
        sqlx::query(
            "INSERT INTO players (device_id, rating, rating_deviation, volatility, updated_at_ms) \
             VALUES ('device-a', 2000.0, 50.0, 0.01, 9999999999999)",
        )
        .execute(&pool)
        .await
        .expect("seed newer state");

        // This commit carries an older timestamp: the stale-guard
        // (`WHERE players.updated_at_ms < excluded.updated_at_ms`) must
        // skip the player update while the rest of the commit lands.
        recorder.commit(&game, 1000).await.expect("stale commit");

        let (rating, deviation): (f64, f64) = sqlx::query_as(
            "SELECT rating, rating_deviation FROM players WHERE device_id = 'device-a'",
        )
        .fetch_one(&pool)
        .await
        .expect("white player row");
        assert_eq!((rating, deviation), (2000.0, 50.0), "the newer state survives");

        // The game row and history rows still committed.
        let game_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM games")
            .fetch_one(&pool)
            .await
            .expect("game count");
        assert_eq!(game_count, 1);
    }
}
