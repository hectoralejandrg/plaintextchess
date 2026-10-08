//! The durable side of authentication (spec "Credential and Session Data
//! Protection", design D2/D4/D6/D7): `SqliteAuthRecorder` mirrors accounts,
//! device links, and sessions into the same SQLite file the ratings use.
//!
//! Every method is one small idempotent write, and every one of them is
//! allowed to fail: the caller logs the failure and the player keeps playing
//! (spec "Server Persistence" — "a failed authentication write MUST NOT block
//! play"). Nothing here is read at runtime — `App::init` loads the rows once
//! into the in-memory store, which is the authority for every later lookup.

use sqlx::sqlite::SqlitePool;

use crate::application::ports::{AuthRecorder, RecorderError};
use crate::domain::account::{Account, Profile, Session};
use crate::infrastructure::recorder::MIGRATIONS;

/// One `sessions` row, in the order `load_all` selects it.
type SessionRow = (String, String, String, String, i64, i64, Option<i64>);

pub struct SqliteAuthRecorder {
    pool: SqlitePool,
}

impl SqliteAuthRecorder {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Read every account, device link, and *unexpired* session back at
    /// startup (design D7): expired sessions are dropped rather than loaded,
    /// so a long-dead server does not carry its session table forward
    /// forever. Revoked sessions are kept — the in-memory store needs them to
    /// tell "revoked" apart from "never existed".
    pub async fn load_all(
        pool: &SqlitePool,
    ) -> Result<(Vec<Account>, Vec<Profile>, Vec<Session>), RecorderError> {
        let account_rows: Vec<(String, String, String, String, Option<String>, i64)> = sqlx::query_as(
            "SELECT account_id, username, username_key, password_hash, display_name, created_at_ms \
             FROM accounts ORDER BY account_id",
        )
        .fetch_all(pool)
        .await
        .map_err(|e| e.to_string())?;

        let profile_rows: Vec<(String, String, i64, i64)> = sqlx::query_as(
            "SELECT device_id, account_id, created_at_ms, updated_at_ms FROM profiles \
             ORDER BY device_id",
        )
        .fetch_all(pool)
        .await
        .map_err(|e| e.to_string())?;

        let now_ms = crate::infrastructure::password::now_ms();
        let session_rows: Vec<SessionRow> = sqlx::query_as(
            "SELECT session_id, account_id, device_id, token_hash, created_at_ms, expires_at_ms, \
             revoked_at_ms FROM sessions WHERE expires_at_ms > ? ORDER BY session_id",
        )
        .bind(now_ms)
        .fetch_all(pool)
        .await
        .map_err(|e| e.to_string())?;

        let accounts = account_rows
            .into_iter()
            .map(
                |(account_id, username, username_key, password_hash, display_name, created_at_ms)| {
                    Account {
                        account_id,
                        username,
                        username_key,
                        password_hash,
                        display_name,
                        created_at_ms,
                    }
                },
            )
            .collect();
        let profiles = profile_rows
            .into_iter()
            .map(
                |(device_id, account_id, created_at_ms, updated_at_ms)| Profile {
                    device_id,
                    account_id,
                    created_at_ms,
                    updated_at_ms,
                },
            )
            .collect();
        let sessions = session_rows
            .into_iter()
            .map(
                |(
                    session_id,
                    account_id,
                    device_id,
                    token_hash,
                    created_at_ms,
                    expires_at_ms,
                    revoked_at_ms,
                )| Session {
                    session_id,
                    account_id,
                    device_id,
                    token_hash,
                    created_at_ms,
                    expires_at_ms,
                    revoked_at_ms,
                },
            )
            .collect();
        Ok((accounts, profiles, sessions))
    }

    /// Delete every session that expired at or before `now_ms` (design D7).
    /// Run once at startup: there is no sweeper, so this is the only place
    /// expired rows go away.
    pub async fn purge_expired(
        pool: &SqlitePool,
        now_ms: i64,
    ) -> Result<usize, RecorderError> {
        let result = sqlx::query("DELETE FROM sessions WHERE expires_at_ms <= ?")
            .bind(now_ms)
            .execute(pool)
            .await
            .map_err(|e| e.to_string())?;
        Ok(result.rows_affected() as usize)
    }
}

impl AuthRecorder for SqliteAuthRecorder {
    /// `INSERT ... ON CONFLICT DO NOTHING` rather than a plain insert: a
    /// retried write must not fail, and the unique `username_key` index is
    /// what makes a duplicate detectable (design D3).
    fn record_account_created<'a>(
        &'a self,
        account: &'a Account,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), RecorderError>> + Send + 'a>>
    {
        Box::pin(async move {
            // A plain INSERT, not `ON CONFLICT DO NOTHING`: the in-memory
            // store already refused a duplicate username before this call, so
            // a unique-index violation here would mean the two layers
            // disagree. Surfacing that as an error the caller logs beats
            // silently dropping the row and losing the account.
            sqlx::query(
                r#"INSERT INTO accounts (
                       account_id, username, username_key, password_hash,
                       display_name, created_at_ms
                   )
                   VALUES (?, ?, ?, ?, ?, ?)"#,
            )
            .bind(&account.account_id)
            .bind(&account.username)
            .bind(&account.username_key)
            .bind(&account.password_hash)
            .bind(&account.display_name)
            .bind(account.created_at_ms)
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
            Ok(())
        })
    }

    fn record_display_name_changed<'a>(
        &'a self,
        account_id: &'a str,
        display_name: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), RecorderError>> + Send + 'a>>
    {
        Box::pin(async move {
            sqlx::query("UPDATE accounts SET display_name = ? WHERE account_id = ?")
                .bind(display_name)
                .bind(account_id)
                .execute(&self.pool)
                .await
                .map_err(|e| e.to_string())?;
            Ok(())
        })
    }

    /// Upsert, because one device holds exactly one account link: a device
    /// that re-registers moves, and its creation mark survives the move.
    fn record_device_linked<'a>(
        &'a self,
        profile: &'a Profile,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), RecorderError>> + Send + 'a>>
    {
        Box::pin(async move {
            sqlx::query(
                r#"INSERT INTO profiles (device_id, account_id, created_at_ms, updated_at_ms)
                   VALUES (?, ?, ?, ?)
                   ON CONFLICT (device_id) DO UPDATE SET
                       account_id = excluded.account_id,
                       updated_at_ms = excluded.updated_at_ms"#,
            )
            .bind(&profile.device_id)
            .bind(&profile.account_id)
            .bind(profile.created_at_ms)
            .bind(profile.updated_at_ms)
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
            Ok(())
        })
    }

    /// Only `session.token_hash` is bound. The plaintext token exists in the
    /// login response and nowhere else, so it cannot be written here even by
    /// accident (design D6).
    fn record_session_issued<'a>(
        &'a self,
        session: &'a Session,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), RecorderError>> + Send + 'a>>
    {
        Box::pin(async move {
            sqlx::query(
                r#"INSERT INTO sessions (
                       session_id, account_id, device_id, token_hash,
                       created_at_ms, expires_at_ms, revoked_at_ms
                   )
                   VALUES (?, ?, ?, ?, ?, ?, NULL)"#,
            )
            .bind(&session.session_id)
            .bind(&session.account_id)
            .bind(&session.device_id)
            .bind(&session.token_hash)
            .bind(session.created_at_ms)
            .bind(session.expires_at_ms)
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
            Ok(())
        })
    }

    /// Revoke by token hash and only if the session is still live, so a
    /// replayed logout cannot move an existing revocation mark.
    fn record_session_revoked<'a>(
        &'a self,
        token_hash: &'a str,
        revoked_at_ms: i64,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), RecorderError>> + Send + 'a>>
    {
        Box::pin(async move {
            sqlx::query(
                "UPDATE sessions SET revoked_at_ms = ? \
                 WHERE token_hash = ? AND revoked_at_ms IS NULL",
            )
            .bind(revoked_at_ms)
            .bind(token_hash)
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
            Ok(())
        })
    }
}

/// Apply the migrations and return a pool ready for both recorders. Shared
/// with `App::init`, which runs migrations itself before attaching anything.
pub async fn migrated_pool(database_url: &str) -> Result<SqlitePool, RecorderError> {
    let pool = crate::infrastructure::recorder::connect_pool(database_url).await?;
    MIGRATIONS.run(&pool).await.map_err(|e| e.to_string())?;
    Ok(pool)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ports::{
        Accounts, ProfileStore, SessionLookup, Sessions,
    };
    use crate::infrastructure::auth::{NoopAuthRecorder, PlayerAuthStore};
    use crate::infrastructure::password::{hash_token, now_ms};

    struct TempDb {
        dir: std::path::PathBuf,
        pool: SqlitePool,
        url: String,
    }

    impl TempDb {
        async fn new() -> Self {
            let dir = std::env::temp_dir()
                .join(format!("chess-auth-recorder-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&dir).expect("create temp dir");
            let url = format!("sqlite:{}/auth.db", dir.display());
            let pool = migrated_pool(&url).await.expect("open temp db");
            Self { dir, pool, url }
        }

        /// Reopen the same file with a fresh pool, as a restart would.
        async fn reopen(&self) -> SqlitePool {
            crate::infrastructure::recorder::connect_pool(&self.url)
                .await
                .expect("reopen")
        }

        fn recorder(&self) -> SqliteAuthRecorder {
            SqliteAuthRecorder::new(self.pool.clone())
        }
    }

    impl Drop for TempDb {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn account(username: &str) -> Account {
        Account {
            account_id: "account-1".into(),
            username: username.into(),
            username_key: crate::domain::account::username_key(username),
            password_hash: "$argon2id$v=19$m=19456,t=2,p=1$c2FsdA$aGFzaA".into(),
            display_name: None,
            created_at_ms: 1_000,
        }
    }

    fn profile(device_id: &str, account_id: &str) -> Profile {
        Profile {
            device_id: device_id.into(),
            account_id: account_id.into(),
            created_at_ms: 1_000,
            updated_at_ms: 1_000,
        }
    }

    fn session(token_hash: &str, account_id: &str) -> Session {
        Session {
            session_id: format!("session-{token_hash}"),
            account_id: account_id.into(),
            device_id: "device-a".into(),
            token_hash: token_hash.into(),
            created_at_ms: now_ms(),
            // A wall-clock stamp an hour out, so the row is genuinely live:
            // `expires_at_ms` is compared against real time at load.
            expires_at_ms: now_ms() + 3_600_000,
            revoked_at_ms: None,
        }
    }

    // ------------------------------------------------------------------
    // 4.2 Account insert and display name
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn an_account_row_carries_the_hash_and_never_the_plaintext() {
        let db = TempDb::new().await;
        let recorder = db.recorder();
        let password = "correct horse battery";
        let mut account = account("Ana");
        account.password_hash = "$argon2id$v=19$m=19456,t=2,p=1$c2FsdA$cmVhbA".into();
        recorder.record_account_created(&account).await.expect("write");

        let (stored_hash, username, username_key, display_name): (String, String, String, Option<String>) =
            sqlx::query_as(
                "SELECT password_hash, username, username_key, display_name FROM accounts \
                 WHERE account_id = 'account-1'",
            )
            .fetch_one(&db.pool)
            .await
            .expect("read back");

        assert_eq!(stored_hash, account.password_hash);
        assert_ne!(stored_hash, password, "the plaintext must never be persisted");
        assert_eq!(username, "Ana");
        assert_eq!(username_key, "ana", "the fold is materialized, not left to NOCASE");
        assert_eq!(display_name, None);
    }

    #[tokio::test]
    async fn a_repeated_username_key_fails_at_the_database() {
        let db = TempDb::new().await;
        let recorder = db.recorder();
        recorder.record_account_created(&account("Ana")).await.expect("write");

        // A different account id, the same case-folded username: the unique
        // index on username_key is what refuses it, which is a stronger
        // guarantee than an application-level check alone.
        let mut second = account("ana");
        second.account_id = "account-2".into();
        let error = recorder
            .record_account_created(&second)
            .await
            .expect_err("a duplicate username_key must not be storable");
        assert!(
            error.to_lowercase().contains("unique"),
            "the failure names the unique constraint: {error}"
        );

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM accounts")
            .fetch_one(&db.pool)
            .await
            .expect("count");
        assert_eq!(count, 1, "the refused insert left no row behind");
    }

    #[tokio::test]
    async fn re_recording_the_same_account_reports_a_duplicate() {
        let db = TempDb::new().await;
        let recorder = db.recorder();
        let account = account("Ana");
        recorder.record_account_created(&account).await.expect("first");

        // The in-memory store refuses a duplicate username before the recorder
        // is reached, so a second write here means the two layers disagreed.
        // Reporting it beats dropping the row silently.
        let error = recorder
            .record_account_created(&account)
            .await
            .expect_err("a replayed account write must not look like success");
        assert!(
            error.to_lowercase().contains("unique"),
            "the failure names the unique constraint: {error}"
        );
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM accounts")
            .fetch_one(&db.pool)
            .await
            .expect("count");
        assert_eq!(count, 1, "the refused write left the original row alone");
    }

    #[tokio::test]
    async fn a_display_name_update_persists_and_is_idempotent() {
        let db = TempDb::new().await;
        let recorder = db.recorder();
        recorder.record_account_created(&account("Ana")).await.expect("write");

        recorder
            .record_display_name_changed("account-1", "Ana T.")
            .await
            .expect("update");
        let stored: Option<String> =
            sqlx::query_scalar("SELECT display_name FROM accounts WHERE account_id = 'account-1'")
                .fetch_one(&db.pool)
                .await
                .expect("read back");
        assert_eq!(stored.as_deref(), Some("Ana T."));

        // Updating an account that does not exist is a no-op, not a failure:
        // a stale logout or a purged account must not break the connection.
        recorder
            .record_display_name_changed("account-404", "Ghost")
            .await
            .expect("no-op");
    }

    // ------------------------------------------------------------------
    // 4.3 Device links and sessions
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn a_device_link_is_upserted_so_a_relink_moves_the_device() {
        let db = TempDb::new().await;
        let recorder = db.recorder();
        recorder.record_account_created(&account("Ana")).await.expect("write");
        let mut bob = account("Bob");
        bob.account_id = "account-2".into();
        recorder.record_account_created(&bob).await.expect("write");

        recorder
            .record_device_linked(&profile("device-a", "account-1"))
            .await
            .expect("link");
        recorder
            .record_device_linked(&profile("device-a", "account-2"))
            .await
            .expect("relink");

        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT device_id, account_id FROM profiles")
                .fetch_all(&db.pool)
                .await
                .expect("read back");
        assert_eq!(
            rows,
            vec![("device-a".to_string(), "account-2".to_string())],
            "one device holds exactly one link, and it moved"
        );
    }

    #[tokio::test]
    async fn a_stored_session_row_holds_only_the_token_hash() {
        let db = TempDb::new().await;
        let recorder = db.recorder();
        recorder.record_account_created(&account("Ana")).await.expect("write");
        let token = "dGhpcyBpcyB0aGUgcGxhaW50ZXh0IHRva2VuIG9mIHRoaXMg";
        let token_hash = hash_token(token);
        let mut issued = session(&token_hash, "account-1");
        issued.device_id = "device-a".into();

        recorder.record_session_issued(&issued).await.expect("write");

        let stored: Vec<(String, String, String)> =
            sqlx::query_as("SELECT token_hash, account_id, device_id FROM sessions")
                .fetch_all(&db.pool)
                .await
                .expect("read back");
        assert_eq!(stored, vec![(token_hash.clone(), "account-1".into(), "device-a".into())]);
        assert_ne!(stored[0].0, token, "the row holds the hash, not the token");

        // Nothing anywhere in the sessions table is the plaintext.
        let dump: String = sqlx::query_scalar(
            "SELECT group_concat(token_hash || COALESCE(device_id, '')) FROM sessions",
        )
        .fetch_one(&db.pool)
        .await
        .expect("dump");
        assert!(
            !dump.contains(token),
            "the plaintext token must not appear in any session column"
        );
    }

    #[tokio::test]
    async fn querying_by_hash_returns_the_right_session() {
        let db = TempDb::new().await;
        let recorder = db.recorder();
        recorder.record_account_created(&account("Ana")).await.expect("write");
        let mut bob = account("Bob");
        bob.account_id = "account-2".into();
        recorder.record_account_created(&bob).await.expect("write");
        recorder
            .record_session_issued(&session("hash-a", "account-1"))
            .await
            .expect("write");
        recorder
            .record_session_issued(&session("hash-b", "account-2"))
            .await
            .expect("write");

        let (account_id, device_id, revoked): (String, String, Option<i64>) = sqlx::query_as(
            "SELECT account_id, device_id, revoked_at_ms FROM sessions WHERE token_hash = 'hash-b'",
        )
        .fetch_one(&db.pool)
        .await
        .expect("point read through the unique index");
        assert_eq!(account_id, "account-2");
        assert_eq!(device_id, "device-a");
        assert_eq!(revoked, None, "a live session has no revocation mark");
    }

    #[tokio::test]
    async fn revoking_marks_only_the_presented_session() {
        let db = TempDb::new().await;
        let recorder = db.recorder();
        recorder.record_account_created(&account("Ana")).await.expect("write");
        recorder.record_session_issued(&session("hash-a", "account-1")).await.expect("write");
        recorder.record_session_issued(&session("hash-b", "account-1")).await.expect("write");

        recorder.record_session_revoked("hash-a", 5_000).await.expect("revoke");

        let marks: Vec<(String, Option<i64>)> =
            sqlx::query_as("SELECT token_hash, revoked_at_ms FROM sessions ORDER BY token_hash")
                .fetch_all(&db.pool)
                .await
                .expect("read back");
        assert_eq!(
            marks,
            vec![
                ("hash-a".to_string(), Some(5_000)),
                ("hash-b".to_string(), None),
            ],
            "the account's other session is untouched: phone and tablet stay signed in"
        );

        // A replayed logout must not move the original mark.
        recorder.record_session_revoked("hash-a", 9_000).await.expect("replay");
        let mark: Option<i64> =
            sqlx::query_scalar("SELECT revoked_at_ms FROM sessions WHERE token_hash = 'hash-a'")
                .fetch_one(&db.pool)
                .await
                .expect("read back");
        assert_eq!(mark, Some(5_000));
    }

    // ------------------------------------------------------------------
    // 4.4 Startup loading and the expired-row purge
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn loading_rebuilds_the_state_that_was_written() {
        let db = TempDb::new().await;
        let recorder = db.recorder();
        let mut ana = account("Ana");
        ana.display_name = Some("Ana T.".into());
        recorder.record_account_created(&ana).await.expect("write");
        recorder
            .record_device_linked(&profile("device-a", "account-1"))
            .await
            .expect("link");
        recorder
            .record_session_issued(&session("hash-1", "account-1"))
            .await
            .expect("session");
        recorder.record_session_issued(&session("hash-2", "account-1")).await.expect("session");
        recorder.record_session_revoked("hash-2", 7_000).await.expect("revoke");

        // Restart: a brand-new pool on the same file.
        let reopened = db.reopen().await;
        let (accounts, profiles, sessions) =
            SqliteAuthRecorder::load_all(&reopened).await.expect("load");

        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].username, "Ana");
        assert_eq!(accounts[0].display_name.as_deref(), Some("Ana T."));
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].device_id, "device-a");
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[1].revoked_at_ms, Some(7_000), "a revoked session is still loaded");

        // And the loaded state actually authenticates once it is in the store.
        let store = PlayerAuthStore::new();
        for loaded in accounts {
            store.insert_account(loaded).expect("load into memory");
        }
        for loaded in profiles {
            store.create_profile(loaded);
        }
        for loaded in sessions {
            store.insert_session(loaded);
        }
        assert_eq!(
            store.session_by_token_hash("hash-1", now_ms()),
            SessionLookup::Usable,
            "a session issued before the restart still authenticates"
        );
        assert_eq!(
            store.session_by_token_hash("hash-2", now_ms()),
            SessionLookup::Revoked,
            "and a revoked one is still revoked after the restart"
        );
        assert_eq!(store.account_id_of_device("device-a").as_deref(), Some("account-1"));
    }

    #[tokio::test]
    async fn expired_session_rows_are_purged_at_startup_and_never_loaded() {
        let db = TempDb::new().await;
        let recorder = db.recorder();
        let now = now_ms();
        recorder.record_account_created(&account("Ana")).await.expect("write");

        let mut expired = session("hash-expired", "account-1");
        expired.expires_at_ms = now - 1_000;
        recorder.record_session_issued(&expired).await.expect("write");
        let mut live = session("hash-live", "account-1");
        live.expires_at_ms = now + 3_600_000;
        recorder.record_session_issued(&live).await.expect("write");

        let reopened = db.reopen().await;
        let (_, _, sessions) = SqliteAuthRecorder::load_all(&reopened).await.expect("load");
        assert_eq!(
            sessions.len(),
            1,
            "an expired row is never loaded, so a restarted server cannot"
        );
        assert_eq!(sessions[0].token_hash, "hash-live");

        let purged = SqliteAuthRecorder::purge_expired(&reopened, now).await.expect("purge");
        assert_eq!(purged, 1);
        let remaining: Vec<String> =
            sqlx::query_scalar("SELECT token_hash FROM sessions")
                .fetch_all(&reopened)
                .await
                .expect("read back");
        assert_eq!(remaining, vec!["hash-live".to_string()]);

        // Purging again is a no-op.
        assert_eq!(SqliteAuthRecorder::purge_expired(&reopened, now).await.expect("purge"), 0);
    }

    #[tokio::test]
    async fn the_purge_leaves_accounts_and_device_links_alone() {
        let db = TempDb::new().await;
        let recorder = db.recorder();
        recorder.record_account_created(&account("Ana")).await.expect("write");
        recorder
            .record_device_linked(&profile("device-a", "account-1"))
            .await
            .expect("link");
        let mut expired = session("hash-expired", "account-1");
        expired.expires_at_ms = 1;
        recorder.record_session_issued(&expired).await.expect("write");

        assert_eq!(SqliteAuthRecorder::purge_expired(&db.pool, now_ms()).await.expect("purge"), 1);

        let accounts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM accounts")
            .fetch_one(&db.pool)
            .await
            .expect("count");
        let profiles: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM profiles")
            .fetch_one(&db.pool)
            .await
            .expect("count");
        assert_eq!(accounts, 1, "session expiry must never log an account out");
        assert_eq!(profiles, 1);
    }

    #[tokio::test]
    async fn a_failing_write_reports_an_error_and_leaves_the_store_correct() {
        // Point the recorder at a database with no auth tables: every write
        // must fail loudly to the caller (which logs) without taking the
        // in-memory store down with it.
        let dir = std::env::temp_dir().join(format!("chess-auth-nodb-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let url = format!("sqlite:{}/nodb.db", dir.display());
        let pool = crate::infrastructure::recorder::connect_pool(&url)
            .await
            .expect("open");
        // Only the original schema: no `accounts`, `profiles`, or `sessions`.
        sqlx::raw_sql("CREATE TABLE placeholder (id INTEGER PRIMARY KEY)")
            .execute(&pool)
            .await
            .expect("create placeholder");

        let recorder = SqliteAuthRecorder::new(pool);
        assert!(recorder.record_account_created(&account("Ana")).await.is_err());
        assert!(recorder.record_display_name_changed("a", "n").await.is_err());
        assert!(recorder.record_device_linked(&profile("d", "a")).await.is_err());
        assert!(recorder.record_session_issued(&session("h", "a")).await.is_err());
        assert!(recorder.record_session_revoked("h", 1).await.is_err());

        // The in-memory authority is unaffected, so play continues.
        let store = PlayerAuthStore::new();
        store.insert_account(account("Ana")).expect("insert");
        assert_eq!(store.account_by_username("ana").map(|a| a.account_id), Some("account-1".into()));

        // And the no-op recorder, which is what a database-less run pairs
        // with, succeeds on the same calls.
        let noop = NoopAuthRecorder;
        noop.record_account_created(&account("Ana")).await.expect("noop");
        noop.record_session_revoked("h", 1).await.expect("noop");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn the_recorder_and_the_in_memory_store_agree_after_every_write() {
        let db = TempDb::new().await;
        let recorder = db.recorder();
        let store = PlayerAuthStore::new();

        let mut ana = account("Ana");
        ana.display_name = Some("Ana T.".into());
        recorder.record_account_created(&ana).await.expect("write");
        store.insert_account(ana.clone()).expect("insert");
        store.set_display_name("account-1", "Ana T.");

        let linked = profile("device-a", "account-1");
        recorder.record_device_linked(&linked).await.expect("link");
        store.link_device("device-a", "account-1");

        let issued = session("hash-1", "account-1");
        recorder.record_session_issued(&issued).await.expect("write");
        store.insert_session(issued);

        recorder.record_session_revoked("hash-1", 5_000).await.expect("revoke");
        assert!(store.revoke_session("hash-1", 5_000));

        let (accounts, profiles, sessions) =
            SqliteAuthRecorder::load_all(&db.pool).await.expect("load");
        assert_eq!(accounts[0].display_name, ana.display_name);
        assert_eq!(profiles[0].account_id, linked.account_id);
        assert_eq!(sessions[0].revoked_at_ms, Some(5_000));
        assert_eq!(
            store.session_by_token_hash("hash-1", now_ms()),
            SessionLookup::Revoked
        );
    }
}