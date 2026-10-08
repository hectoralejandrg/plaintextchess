//! Application state shared by the HTTP/WebSocket handlers (design D1,
//! infrastructure layer): the room registry (joinable rooms by code), the
//! one-room-per-device index, and the in-memory rating store.

#![allow(clippy::result_large_err)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use crate::application::ports::{
    Accounts, AuthRecorder, ChessEngine, GameRecorder, PlayerAuth, ProfileStore, Ratings,
    RoomRegistry, RoomServices, SessionLookup, Sessions, TimeSource,
};
use crate::application::room_actor::{run_room, RoomMsg};
use crate::domain::account::Identity;
use crate::domain::rating::RatingState;
use crate::domain::room::{CODE_ALPHABET, CODE_LEN, is_valid_code};
use crate::domain::time_control::TimeControl;
use crate::infrastructure::auth::{NoopAuthRecorder, PlayerAuthStore};
use crate::infrastructure::auth_recorder::SqliteAuthRecorder;
use crate::infrastructure::config::Config;
use crate::infrastructure::engine::CoreEngine;
use crate::infrastructure::password::{hash_token, now_ms, PasswordHasher};
use crate::infrastructure::ratings::RatingStore;
use crate::infrastructure::recorder::{MIGRATIONS, NoopGameRecorder, SqliteGameRecorder};
use crate::interface::protocol::{ErrorCode, ServerMessage};

/// The credential primitives the WebSocket auth handlers need, derived from
/// the app's configuration (spec "Credential and Session Data Protection").
#[derive(Clone)]
pub struct AuthTooling {
    pub hasher: PasswordHasher,
    pub session_ttl: Duration,
}

/// A live connection bound to a room: the mailbox to send the room's
/// decisions from, and the room's outbound messages to the client.
///
/// The connection carries its *pinned* identity (design D8). Pinning matters
/// because a client may log in mid-game: the identity is upgraded once, at the
/// moment of the login, and every later `Move`/`Resign`/`Leave` reuses that
/// upgraded value instead of re-resolving a token that may have expired since.
pub struct Conn {
    pub identity: Identity,
    pub code: String,
    pub mailbox: mpsc::UnboundedSender<RoomMsg>,
    pub out_rx: mpsc::UnboundedReceiver<ServerMessage>,
}

impl Conn {
    /// The device identifier, which is also the rating key.
    pub fn player_id(&self) -> &str {
        self.identity.device_id()
    }
}

/// Monotonic wall time in milliseconds (design D2/D9): the production
/// `TimeSource`. Only differences between marks are meaningful.
pub struct SystemTimeSource {
    origin: Instant,
}

impl SystemTimeSource {
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl TimeSource for SystemTimeSource {
    fn now_ms(&self) -> u64 {
        Instant::now().duration_since(self.origin).as_millis() as u64
    }
}

impl Default for SystemTimeSource {
    fn default() -> Self {
        Self::new()
    }
}

pub struct App {
    config: Config,
    /// Joinable rooms, by code. Terminal rooms stay listed until their
    /// players disconnect; forfeited rooms are removed immediately.
    rooms: Mutex<HashMap<String, mpsc::UnboundedSender<RoomMsg>>>,
    /// One room per device: player_id -> room code. The guest path, keyed by
    /// the client-asserted device identifier.
    players: Mutex<HashMap<String, String>>,
    /// One room per *account*: account_id -> room code (spec "Server Room
    /// Management"). Checked only for authenticated identities, so an account
    /// signed in on a phone and a tablet cannot hold two rooms while guests
    /// are unaffected.
    account_rooms: Mutex<HashMap<String, String>>,
    ratings: Arc<RatingStore>,
    engine: Arc<dyn ChessEngine>,
    time: Arc<dyn TimeSource>,
    recorder: Arc<dyn GameRecorder>,
    /// The authentication authority (design D4): accounts, device links, and
    /// sessions in memory, mirrored to `auth_recorder`.
    auth: Arc<PlayerAuthStore>,
    /// The durable side of `auth`. `None` for a run without a database.
    auth_recorder: Arc<dyn AuthRecorder>,
}

impl RoomRegistry for App {
    fn remove_room(&self, code: &str) {
        self.rooms.lock().unwrap().remove(code);
    }

    /// Forget a player, but only the entries still bound to this room (so a
    /// late rejection never unbinds the player from a newer room). An
    /// authenticated identity holds two entries and both go (design D8/D9).
    fn untrack_if(&self, identity: &Identity, code: &str) {
        {
            let mut players = self.players.lock().unwrap();
            if players.get(identity.device_id()).is_some_and(|c| c == code) {
                players.remove(identity.device_id());
            }
        }
        if let Some(account_id) = identity.account_id() {
            let mut account_rooms = self.account_rooms.lock().unwrap();
            if account_rooms.get(account_id).is_some_and(|c| c == code) {
                account_rooms.remove(account_id);
            }
        }
    }
}

impl App {
    /// Synchronous, persistence-free constructor: the recorder is a
    /// `NoopGameRecorder` and ratings are in-memory only. Used by tests
    /// (`Config::for_test`) and by any run where persistence is disabled.
    pub fn new(config: Config) -> Self {
        let engine: Arc<dyn ChessEngine> = Arc::new(CoreEngine);
        let ratings = Arc::new(RatingStore::new(Arc::clone(&engine)));
        let time: Arc<dyn TimeSource> = Arc::new(SystemTimeSource::new());
        let recorder: Arc<dyn GameRecorder> = Arc::new(NoopGameRecorder);
        Self {
            config,
            rooms: Mutex::new(HashMap::new()),
            players: Mutex::new(HashMap::new()),
            account_rooms: Mutex::new(HashMap::new()),
            ratings,
            engine,
            time,
            recorder,
            auth: Arc::new(PlayerAuthStore::new()),
            auth_recorder: Arc::new(NoopAuthRecorder),
        }
    }

    /// Production constructor (spec "Server Persistence", design D6):
    /// open the configured database, run the versioned migrations, load
    /// the persisted per-device Glicko-2 state into the in-memory rating
    /// store, and attach the SQLite recorder. Fails fast — with an error,
    /// never a half-initialized app — when the database cannot be created,
    /// opened, or migrated.
    pub async fn init(config: Config) -> Result<Self, String> {
        let database_url = config.database_url.clone().ok_or_else(|| {
            "persistence is disabled: App::init requires a Config with database_url \
             (use App::new for the in-memory test path)"
                .to_string()
        })?;

        // The database file's parent directory must exist (design D6):
        // create it so the default `data/` location works out of the box.
        if let Some(parent) = Self::sqlite_parent_dir(&database_url) {
            std::fs::create_dir_all(&parent).map_err(|e| {
                format!(
                    "cannot create the database directory {parent:?} for `{database_url}`: {e}"
                )
            })?;
        }

        let pool = SqliteGameRecorder::connect(&database_url).await?;
        MIGRATIONS
            .run(&pool)
            .await
            .map_err(|e| format!("database migrations failed for `{database_url}`: {e}"))?;

        let engine: Arc<dyn ChessEngine> = Arc::new(CoreEngine);
        let ratings = Arc::new(RatingStore::new(Arc::clone(&engine)));
        // Design D1: the in-memory store is the runtime authority, seeded
        // from the database at startup so ratings survive restarts.
        let rows =
            sqlx::query_as::<_, (String, f64, f64, f64)>(
                "SELECT device_id, rating, rating_deviation, volatility FROM players",
            )
            .fetch_all(&pool)
            .await
            .map_err(|e| {
                format!("loading persisted ratings failed for `{database_url}`: {e}")
            })?;
        for (device_id, rating, rating_deviation, volatility) in rows {
            ratings.seed(
                &device_id,
                RatingState {
                    rating,
                    rating_deviation,
                    volatility,
                },
            );
        }

        let time: Arc<dyn TimeSource> = Arc::new(SystemTimeSource::new());
        let recorder: Arc<dyn GameRecorder> = Arc::new(SqliteGameRecorder::new(pool.clone()));

        // Authentication loads after the ratings and before anything can
        // accept a connection (task 9.1). The order matters in one place only:
        // expired session rows are purged from the database *first*, and what
        // is left is what the in-memory store is seeded with, so the two can
        // never disagree about which sessions exist.
        let started_at_ms = now_ms();
        let purged = SqliteAuthRecorder::purge_expired(&pool, started_at_ms)
            .await
            .map_err(|e| format!("purging expired sessions failed for `{database_url}`: {e}"))?;
        let (accounts, profiles, sessions) = SqliteAuthRecorder::load_all(&pool)
            .await
            .map_err(|e| format!("loading player accounts failed for `{database_url}`: {e}"))?;

        let auth = Arc::new(PlayerAuthStore::new());
        for account in accounts {
            // A duplicate here is unreachable: `account_id` is the primary key
            // and the store starts empty. Logged rather than ignored silently
            // so a corrupt file is visible.
            if let Err(err) = auth.insert_account(account) {
                tracing::error!(
                    error = %err,
                    "a persisted account could not be loaded and was skipped"
                );
            }
        }
        // `create_profile` fills both the profile row and the device→account
        // index, so one pass rebuilds both.
        for profile in profiles {
            auth.create_profile(profile);
        }
        for session in sessions {
            auth.insert_session(session);
        }
        if purged > 0 {
            tracing::info!(purged, "dropped expired sessions at startup");
        }
        tracing::info!(
            accounts = auth.account_count(),
            devices = auth.device_count(),
            sessions = auth.session_count(),
            "loaded player authentication state"
        );

        Ok(Self {
            config,
            rooms: Mutex::new(HashMap::new()),
            players: Mutex::new(HashMap::new()),
            account_rooms: Mutex::new(HashMap::new()),
            ratings,
            engine,
            time,
            recorder,
            auth,
            auth_recorder: Arc::new(SqliteAuthRecorder::new(pool)),
        })
    }

    /// The directory that a `sqlite:` URL's database file lives in, if any:
    /// `None` for `:memory:` URLs and for a bare file name in the working
    /// directory.
    fn sqlite_parent_dir(database_url: &str) -> Option<std::path::PathBuf> {
        let path = database_url.strip_prefix("sqlite:")?;
        if path == ":memory:" {
            return None;
        }
        std::path::Path::new(path)
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(std::path::Path::to_path_buf)
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn ratings(&self) -> &RatingStore {
        &self.ratings
    }

    pub fn room_codes(&self) -> Vec<String> {
        self.rooms.lock().unwrap().keys().cloned().collect()
    }

    pub fn contains_room(&self, code: &str) -> bool {
        self.rooms.lock().unwrap().contains_key(code)
    }

    /// Who a connection is, given the token it presented (task 6.1).
    ///
    /// Absent, unknown, expired, and revoked tokens all resolve to the same
    /// guest (spec "Guest Play Fallback"): authentication adds an identity,
    /// it never withholds the ability to play. The four cases are still
    /// distinguished in the log so an operator can tell a client bug from a
    /// natural logout, and never by the token's value.
    pub fn resolve_identity(&self, device_id: &str, token: Option<&str>) -> Identity {
        let guest = || Identity::Guest {
            device_id: device_id.to_string(),
        };
        let Some(token) = token.filter(|t| !t.is_empty()) else {
            return guest();
        };
        let token_hash = hash_token(token);
        let now_ms = now_ms();
        match self.auth.session_by_token_hash(&token_hash, now_ms) {
            SessionLookup::Usable => {
                let Some(session) = self.auth.usable_session(&token_hash, now_ms) else {
                    return guest();
                };
                // The session records the device that signed in. Trust the
                // *connection's* device id rather than the session's: a player
                // may re-attach from a second device, and the account is what
                // must follow (design D1/D9).
                Identity::Account {
                    account_id: session.account_id,
                    device_id: device_id.to_string(),
                }
            }
            SessionLookup::Revoked => {
                tracing::info!(device_id = %device_id, "presented session is revoked; playing as a guest");
                guest()
            }
            SessionLookup::Absent => {
                tracing::info!(device_id = %device_id, "presented session is unknown or expired; playing as a guest");
                guest()
            }
        }
    }

    /// The authentication authority, for the WebSocket auth handlers (spec
    /// "Authentication Wire Messages").
    pub fn auth(&self) -> &Arc<PlayerAuthStore> {
        &self.auth
    }

    /// Bind a device to an account, in memory and durably (spec "Device-to-Account
    /// Profile Linkage"). A device holds one account at a time, so a device
    /// that signs in as somebody else moves: a session's `device_id` follows
    /// the account it was last used with.
    ///
    /// The in-memory write happens first and synchronously, so the device is
    /// bound for every later lookup even if the durable write fails.
    pub async fn link_device_to(&self, device_id: &str, account_id: &str) {
        self.auth.link_device(device_id, account_id);
        let Some(profile) = self.auth.profile_of(device_id) else {
            return;
        };
        let recorder = Arc::clone(&self.auth_recorder);
        tokio::spawn(async move {
            if let Err(err) = recorder.record_device_linked(&profile).await {
                tracing::error!(
                    error = %err,
                    account_id = %profile.account_id,
                    device_id = %profile.device_id,
                    "failed to persist a device link; the device is linked in memory only"
                );
            }
        });
    }

    /// Bind the device an identity names to its account, if any. Called on the
    /// room path so that the token-first order (`login`, then
    /// `create_room`) ends up linked even though the `login` itself had no
    /// device to bind.
    pub fn link_identity(self: &Arc<Self>, identity: &Identity) {
        let Some(account_id) = identity.account_id() else {
            return;
        };
        self.auth
            .link_device(identity.device_id(), account_id);
        let Some(profile) = self.auth.profile_of(identity.device_id()) else {
            return;
        };
        let recorder = Arc::clone(&self.auth_recorder);
        tokio::spawn(async move {
            if let Err(err) = recorder.record_device_linked(&profile).await {
                tracing::error!(
                    error = %err,
                    account_id = %profile.account_id,
                    device_id = %profile.device_id,
                    "failed to persist a device link; the device is linked in memory only"
                );
            }
        });
    }

    /// The durable side of `auth`, so a handler can mirror a write and log a
    /// failure without the player waiting on it (spec "Server Persistence").
    pub fn auth_recorder(&self) -> &Arc<dyn AuthRecorder> {
        &self.auth_recorder
    }

    /// Hashing and issuing for the WebSocket auth handlers.
    pub fn auth_tooling(&self) -> AuthTooling {
        AuthTooling {
            hasher: PasswordHasher::new(self.config.argon2),
            session_ttl: Duration::from_secs(self.config.session_ttl_secs),
        }
    }

    pub fn is_valid_code(code: &str) -> bool {
        is_valid_code(code)
    }

    /// The ports the room actor needs (design D1): this app owns the
    /// registry, and it shares the ratings, the engine, and the time source
    /// with the room it spawns.
    fn services(&self, this: &Arc<Self>) -> RoomServices {
        RoomServices {
            registry: Arc::clone(this) as Arc<dyn RoomRegistry>,
            ratings: Arc::clone(&self.ratings) as Arc<dyn Ratings>,
            engine: Arc::clone(&self.engine),
            time: Arc::clone(&self.time),
            reconnect_grace: self.config.reconnect_grace,
            recorder: Arc::clone(&self.recorder),
            auth: Arc::clone(&self.auth) as Arc<dyn PlayerAuth>,
            random_colors: self.config.random_colors,
            incremental_updates: self.config.incremental_updates,
        }
    }

    /// Create a room with the default time control (spec "Server Room
    /// Management"): unique code, the creator seated White, the room actor
    /// spawned.
    ///
    /// Takes `impl Into<Identity>` so a bare device identifier means "guest",
    /// which is how every pre-existing caller — and every client that never
    /// authenticates — keeps working unchanged.
    pub fn create_room(self: &Arc<Self>, player: impl Into<Identity>) -> Result<Conn, ServerMessage> {
        self.create_room_with_time_control(player, TimeControl::DEFAULT)
    }

    /// Create a room with an explicit time control (spec "Creating a room
    /// chooses its time control"), used by the wire path and by tests that
    /// need short clocks.
    pub fn create_room_with_time_control(
        self: &Arc<Self>,
        player: impl Into<Identity>,
        time_control: TimeControl,
    ) -> Result<Conn, ServerMessage> {
        let identity = player.into();
        self.check_room_limit(&identity, None)?;
        let code = self.unique_code();
        let (mailbox_tx, mailbox_rx) = mpsc::unbounded_channel();
        let (out_tx, out_rx) = mpsc::unbounded_channel();

        let app = Arc::clone(self);
        let actor_code = code.clone();
        let services = self.services(&app);
        tokio::spawn(async move {
            run_room(mailbox_rx, services, actor_code, time_control).await;
        });

        self.rooms
            .lock()
            .unwrap()
            .insert(code.clone(), mailbox_tx.clone());
        self.track_identity(&identity, &code);
        let conn_identity = identity.clone();
        let _ = mailbox_tx.send(RoomMsg::Connect {
            identity,
            code: None,
            out: out_tx,
        });

        Ok(Conn {
            identity: conn_identity,
            code,
            mailbox: mailbox_tx,
            out_rx,
        })
    }

    /// Join (or re-attach to) a room by code. Format errors, players already
    /// in a room, and unknown rooms are rejected before the room actor is
    /// involved; the actor rejects full or finished rooms.
    pub fn join_room(
        self: &Arc<Self>,
        player: impl Into<Identity>,
        raw_code: &str,
    ) -> Result<Conn, ServerMessage> {
        let identity = player.into();
        let code = raw_code.to_uppercase();
        if !Self::is_valid_code(&code) {
            return Err(ServerMessage::error(ErrorCode::InvalidRoomCode));
        }
        self.check_room_limit(&identity, Some(&code))?;
        let mailbox = {
            let rooms = self.rooms.lock().unwrap();
            rooms
                .get(&code)
                .cloned()
                .ok_or_else(|| ServerMessage::error(ErrorCode::RoomNotFound))?
        };
        let (out_tx, out_rx) = mpsc::unbounded_channel();
        self.track_identity(&identity, &code);
        if mailbox
            .send(RoomMsg::Connect {
                identity: identity.clone(),
                code: Some(code.clone()),
                out: out_tx,
            })
            .is_err()
        {
            // The room actor is gone: it has already dropped the room.
            self.untrack_if(&identity, &code);
            return Err(ServerMessage::error(ErrorCode::RoomNotFound));
        }
        Ok(Conn {
            identity,
            code,
            mailbox,
            out_rx,
        })
    }

    /// The one-room-per-player rule (spec "Server Room Management"), checked
    /// on both keys when the player is authenticated (design D10).
    ///
    /// `same_code` is the room being joined: an identity already bound to
    /// *that* room is reconnecting, which is allowed, while one bound to any
    /// other room is not. Guests only ever have the device entry.
    fn check_room_limit(
        &self,
        identity: &Identity,
        same_code: Option<&str>,
    ) -> Result<(), ServerMessage> {
        let device = self.players.lock().unwrap();
        let holds_other = |current: Option<&String>| {
            current.is_some_and(|code| Some(code.as_str()) != same_code)
        };
        if holds_other(device.get(identity.device_id())) {
            return Err(ServerMessage::error(ErrorCode::AlreadyInRoom));
        }
        drop(device);

        if let Some(account_id) = identity.account_id() {
            let account_rooms = self.account_rooms.lock().unwrap();
            if holds_other(account_rooms.get(account_id)) {
                // One account holds at most one room even when it is signed
                // in on two devices (spec "Server Room Management").
                return Err(ServerMessage::error(ErrorCode::AlreadyInRoom));
            }
        }
        Ok(())
    }

    /// Re-record a connection's identity for the room it is already in.
    ///
    /// Needed when a login arrives *mid-connection* (design D8): the room was
    /// tracked under the guest identity, so without this the account's room
    /// entry would not exist and the one-account-one-room rule would not apply
    /// until the player re-joined. The device entry is rewritten with the same
    /// value it already had, so this is idempotent.
    pub fn adopt_identity(&self, identity: &Identity, code: &str) {
        self.track_identity(identity, code);
    }

    fn track_identity(&self, identity: &Identity, code: &str) {
        {
            let mut players = self.players.lock().unwrap();
            players.insert(identity.device_id().to_string(), code.to_string());
        }
        if let Some(account_id) = identity.account_id() {
            let mut account_rooms = self.account_rooms.lock().unwrap();
            account_rooms.insert(account_id.to_string(), code.to_string());
        }
    }

    /// Draw a room code that no current room uses.
    fn unique_code(&self) -> String {
        loop {
            let mut code = String::new();
            for _ in 0..CODE_LEN {
                let index = rand::random::<usize>() % CODE_ALPHABET.len();
                code.push(CODE_ALPHABET[index] as char);
            }
            let rooms = self.rooms.lock().unwrap();
            if !rooms.contains_key(&code) {
                return code;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// A temp directory that will hold a database file; removed on drop
    /// (best effort).
    struct TempDbDir {
        dir: std::path::PathBuf,
    }

    impl TempDbDir {
        fn new() -> Self {
            let dir = std::env::temp_dir()
                .join(format!("chess-app-init-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&dir).expect("create temp dir");
            Self { dir }
        }

        fn database_url(&self) -> String {
            format!("sqlite:{}/app.db", self.dir.display())
        }
    }

    impl Drop for TempDbDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn config_with_url(url: String) -> Config {
        Config::with_database(url)
    }

    #[tokio::test]
    async fn init_loads_persisted_players_into_the_rating_store() {
        let dir = TempDbDir::new();
        // Pre-seed the database as a previous server run would have left it.
        {
            let pool =
                SqliteGameRecorder::connect(&dir.database_url()).await.expect("open pre-seed db");
            MIGRATIONS
                .run(&pool)
                .await
                .expect("pre-seed migrations");
            sqlx::query(
                "INSERT INTO players (device_id, rating, rating_deviation, volatility, updated_at_ms) \
                 VALUES ('device-seed-a', 1600.0, 300.0, 0.07, 1), \
                        ('device-seed-b', 1400.0, 250.0, 0.05, 1)",
            )
            .execute(&pool)
            .await
            .expect("seed players");
        }

        let app = match App::init(config_with_url(dir.database_url())).await {
            Ok(app) => app,
            Err(err) => panic!("init must succeed on a valid database: {err}"),
        };
        assert_eq!(app.ratings().rating_of("device-seed-a"), 1600.0);
        assert_eq!(
            app.ratings().rating_state("device-seed-a"),
            Some(RatingState {
                rating: 1600.0,
                rating_deviation: 300.0,
                volatility: 0.07,
            }),
            "the persisted full Glicko-2 state is loaded"
        );
        assert_eq!(app.ratings().rating_of("device-seed-b"), 1400.0);
        // Devices the database does not know still start at the default.
        // (Check the state first: rating_of lazily creates the default
        // session, which would make the state visible afterwards.)
        assert_eq!(app.ratings().rating_state("device-unknown"), None);
        assert_eq!(app.ratings().rating_of("device-unknown"), 1500.0);
    }

    #[tokio::test]
    async fn init_fails_fast_when_the_database_cannot_be_opened() {
        let dir = TempDbDir::new();
        // A path component that is a regular file: no database can live there.
        let blocker = dir.dir.join("blocker");
        std::fs::write(&blocker, b"x").expect("write blocker");
        let url = format!("sqlite:{}/db.db", blocker.display());

        let err = match App::init(config_with_url(url)).await {
            Ok(_) => panic!("init must return Err, not Ok"),
            Err(err) => err,
        };
        assert!(
            err.to_lowercase().contains("database"),
            "the error must mention the database: {err}"
        );
    }

    #[tokio::test]
    async fn init_without_a_configured_database_is_an_error() {
        let config = Config::for_test(
            "127.0.0.1".parse().unwrap(),
            0,
            Duration::from_secs(30),
        );
        assert!(
            App::init(config).await.is_err(),
            "App::init with persistence disabled must fail, not fall back to Noop"
        );
    }

    // ------------------------------------------------------------------
    // 9.1 Startup loads authentication state
    // ------------------------------------------------------------------

    fn in_memory_app() -> Arc<App> {
        Arc::new(App::new(Config::for_test(
            "127.0.0.1".parse().unwrap(),
            0,
            Duration::from_secs(30),
        )))
    }

    /// Register an account directly in the store and issue a session for it,
    /// the way the WebSocket handlers do, and hand back the plaintext token.
    fn seed_account(
        app: &Arc<App>,
        username: &str,
        account_id: &str,
        ttl: std::time::Duration,
    ) -> String {
        let account = crate::domain::account::Account {
            account_id: account_id.into(),
            username: username.into(),
            username_key: crate::domain::account::username_key(username),
            password_hash: "$argon2id$seed".into(),
            display_name: None,
            created_at_ms: now_ms(),
        };
        app.auth().insert_account(account).expect("seed account");
        let token = crate::infrastructure::password::generate_token();
        app.auth().insert_session(crate::domain::account::Session {
            session_id: uuid::Uuid::new_v4().to_string(),
            account_id: account_id.into(),
            device_id: "device-seed".into(),
            token_hash: crate::infrastructure::password::hash_token(&token),
            created_at_ms: now_ms(),
            expires_at_ms: crate::infrastructure::password::expires_at(now_ms(), ttl),
            revoked_at_ms: None,
        });
        token
    }

    /// A session issued before a restart still authenticates after it
    /// (task 9.1). Nothing about the token changes across the restart: only
    /// its hash is stored, and that is what the new process looks up.
    #[tokio::test]
    async fn a_session_issued_before_a_restart_still_authenticates_after_it() {
        let dir = TempDbDir::new();
        let url = dir.database_url();

        let (account_id, token, username) = {
            let pool = SqliteGameRecorder::connect(&url).await.expect("open");
            MIGRATIONS.run(&pool).await.expect("migrate");
            let recorder = SqliteAuthRecorder::new(pool.clone());
            let account = crate::domain::account::Account {
                account_id: "account-1".into(),
                username: "Ana".into(),
                username_key: crate::domain::account::username_key("Ana"),
                password_hash: "$argon2id$seed".into(),
                display_name: Some("Ana T.".into()),
                created_at_ms: now_ms(),
            };
            recorder.record_account_created(&account).await.expect("account");
            recorder
                .record_device_linked(&crate::domain::account::Profile {
                    device_id: "device-a".into(),
                    account_id: "account-1".into(),
                    created_at_ms: now_ms(),
                    updated_at_ms: now_ms(),
                })
                .await
                .expect("link");
            let token = crate::infrastructure::password::generate_token();
            recorder
                .record_session_issued(&crate::domain::account::Session {
                    session_id: uuid::Uuid::new_v4().to_string(),
                    account_id: "account-1".into(),
                    device_id: "device-a".into(),
                    token_hash: crate::infrastructure::password::hash_token(&token),
                    created_at_ms: now_ms(),
                    expires_at_ms: crate::infrastructure::password::expires_at(
                        now_ms(),
                        Duration::from_secs(3600),
                    ),
                    revoked_at_ms: None,
                })
                .await
                .expect("session");
            ("account-1".to_string(), token, "Ana".to_string())
        };

        // Restart.
        let app = App::init(config_with_url(url)).await.expect("init");
        assert_eq!(app.auth().account_count(), 1, "the account was loaded");
        assert_eq!(
            app.auth().account_by_id(&account_id).map(|a| a.username),
            Some(username),
            "with its display name intact"
        );
        assert_eq!(
            app.auth()
                .account_by_id(&account_id)
                .map(|a| a.reported_name().to_string()),
            Some("Ana T.".to_string())
        );
        assert_eq!(
            app.auth().account_id_of_device("device-a").as_deref(),
            Some("account-1"),
            "the device link was rebuilt, not just the profile row"
        );

        let identity = app.resolve_identity("device-a", Some(&token));
        assert_eq!(
            identity.account_id(),
            Some("account-1"),
            "the pre-restart token still authenticates"
        );
        // And from a *different* device, because the account follows (task
        // 9.3).
        let from_tablet = app.resolve_identity("device-tablet", Some(&token));
        assert_eq!(
            from_tablet.account_id(),
            Some("account-1"),
            "the account follows the player to another device"
        );
        assert_eq!(from_tablet.device_id(), "device-tablet");
    }

    #[tokio::test]
    async fn init_purges_expired_sessions_and_keeps_the_rest() {
        let dir = TempDbDir::new();
        let url = dir.database_url();
        {
            let pool = SqliteGameRecorder::connect(&url).await.expect("open");
            MIGRATIONS.run(&pool).await.expect("migrate");
            sqlx::query(
                "INSERT INTO accounts (account_id, username, username_key, password_hash, \
                 created_at_ms) VALUES ('account-1', 'Ana', 'ana', '$argon2id$seed', 1)",
            )
            .execute(&pool)
            .await
            .expect("account");
            sqlx::query(
                "INSERT INTO sessions (session_id, account_id, device_id, token_hash, \
                 created_at_ms, expires_at_ms) VALUES \
                 ('s-expired', 'account-1', 'device-a', 'hash-expired', 1, 1), \
                 ('s-live', 'account-1', 'device-a', 'hash-live', 1, ?)",
            )
            .bind(crate::infrastructure::password::expires_at(
                now_ms(),
                Duration::from_secs(3600),
            ))
            .execute(&pool)
            .await
            .expect("sessions");
        }

        let app = App::init(config_with_url(url.clone())).await.expect("init");
        assert_eq!(app.auth().session_count(), 1, "only the live session loads");
        assert_eq!(
            app.auth().session_by_token_hash("hash-live", now_ms()),
            SessionLookup::Usable
        );
        assert_eq!(
            app.auth().session_by_token_hash("hash-expired", now_ms()),
            SessionLookup::Absent
        );
        // The expired row is gone from the database too, not just skipped in
        // memory: the purge is the only sweeper there is (design D7).
        let pool = SqliteGameRecorder::connect(&url).await.expect("reopen");
        let rows: Vec<String> = sqlx::query_scalar("SELECT session_id FROM sessions")
            .fetch_all(&pool)
            .await
            .expect("read back");
        assert_eq!(rows, vec!["s-live".to_string()]);
        assert_eq!(app.auth().account_count(), 1, "the account survives the purge");
    }

    #[tokio::test]
    async fn a_revoked_session_is_still_known_to_be_revoked_after_a_restart() {
        let dir = TempDbDir::new();
        let url = dir.database_url();
        {
            let pool = SqliteGameRecorder::connect(&url).await.expect("open");
            MIGRATIONS.run(&pool).await.expect("migrate");
            sqlx::query(
                "INSERT INTO accounts (account_id, username, username_key, password_hash, \
                 created_at_ms) VALUES ('account-1', 'Ana', 'ana', '$argon2id$seed', 1)",
            )
            .execute(&pool)
            .await
            .expect("account");
            sqlx::query(
                "INSERT INTO sessions (session_id, account_id, device_id, token_hash, \
                 created_at_ms, expires_at_ms, revoked_at_ms) \
                 VALUES ('s-1', 'account-1', 'device-a', 'hash-1', 1, ?, 12345)",
            )
            .bind(crate::infrastructure::password::expires_at(
                now_ms(),
                Duration::from_secs(3600),
            ))
            .execute(&pool)
            .await
            .expect("session");
        }

        let app = App::init(config_with_url(url)).await.expect("init");
        assert_eq!(
            app.auth().session_by_token_hash("hash-1", now_ms()),
            SessionLookup::Revoked,
            "revocation survives a restart: the store needs it to tell \
             `revoked` apart from `never existed`"
        );
    }

    // ------------------------------------------------------------------
    // 6.3 The per-account room limit
    // ------------------------------------------------------------------

    fn account_identity(device_id: &str, account_id: &str) -> Identity {
        Identity::Account {
            account_id: account_id.into(),
            device_id: device_id.into(),
        }
    }

    #[tokio::test]
    async fn one_account_cannot_hold_two_rooms_across_two_devices() {
        let app = in_memory_app();
        let phone = account_identity("device-phone", "account-1");
        let tablet = account_identity("device-tablet", "account-1");

        let first = app.create_room(phone.clone()).expect("first room");
        assert!(app.create_room(phone.clone()).is_err(), "same device, twice");

        let err = match app.create_room(tablet.clone()) {
            Ok(_) => panic!(
                "a second device of the same account must not open a second room"
            ),
            Err(err) => err,
        };
        let value: serde_json::Value = serde_json::to_value(&err).unwrap();
        assert_eq!(value["code"], "already_in_room");

        // The phone's room is untouched and the tablet can still join it,
        // which is the point: one account, one room.
        assert!(app.join_room(tablet, &first.code).is_ok());
    }

    #[tokio::test]
    async fn two_unrelated_guests_are_unaffected_by_the_account_limit() {
        let app = in_memory_app();
        let a = app.create_room("device-a").expect("a");
        let b = app.create_room("device-b").expect("b");
        // Two accounts, two rooms, no interference.
        let c = app
            .create_room(account_identity("device-c", "account-1"))
            .expect("c");
        let d = app
            .create_room(account_identity("device-d", "account-2"))
            .expect("d");
        assert_ne!(a.code, b.code);
        assert_ne!(c.code, d.code);
        // And a guest can still join a room an account holds.
        assert!(app.join_room("device-e", &c.code).is_ok());
    }

    #[tokio::test]
    async fn an_account_is_refused_a_second_room_but_may_rejoin_its_own() {
        let app = in_memory_app();
        let identity = account_identity("device-phone", "account-1");
        let first = app.create_room(identity.clone()).expect("first");
        // Re-joining the same room is a reconnect, not a second room.
        assert!(
            app.join_room(identity.clone(), &first.code).is_ok(),
            "re-attaching to the room it already holds must be allowed"
        );
        // A different room is refused.
        let other = app
            .create_room(account_identity("device-tablet", "account-2"))
            .expect("other");
        assert!(app.join_room(identity, &other.code).is_err());
    }

    // ------------------------------------------------------------------
    // 6.4 Identity-aware unbinding
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn untracking_releases_both_the_device_and_the_account_entry() {
        let app = in_memory_app();
        let identity = account_identity("device-phone", "account-1");
        app.create_room(identity.clone()).expect("room");
        assert!(app.players.lock().unwrap().contains_key("device-phone"));
        assert!(app.account_rooms.lock().unwrap().contains_key("account-1"));

        app.untrack_if(&identity, &app_code(&app, &identity));
        assert!(
            !app.players.lock().unwrap().contains_key("device-phone"),
            "the device entry is released"
        );
        assert!(
            !app.account_rooms.lock().unwrap().contains_key("account-1"),
            "so is the account entry, even though the caller named only the device"
        );
    }

    /// The room an identity is currently tracked in, for the `untrack_if`
    /// calls that must name the right code.
    fn app_code(app: &Arc<App>, identity: &Identity) -> String {
        app.players
            .lock()
            .unwrap()
            .get(identity.device_id())
            .cloned()
            .expect("tracked")
    }

    #[tokio::test]
    async fn untracking_ignores_a_room_the_player_has_already_left() {
        let app = in_memory_app();
        let identity = account_identity("device-phone", "account-1");
        let first = app.create_room(identity.clone()).expect("first room");
        // Release the first room and join a second one.
        app.untrack_if(&identity, &first.code);
        let second = app.create_room(identity.clone()).expect("second room");

        // A late rejection naming the *old* room must not unbind the player
        // from the room they are actually in.
        app.untrack_if(&identity, &first.code);
        assert_eq!(
            app.players.lock().unwrap().get("device-phone"),
            Some(&second.code),
            "the stale untrack did nothing"
        );
        assert_eq!(
            app.account_rooms.lock().unwrap().get("account-1"),
            Some(&second.code)
        );
    }

    #[tokio::test]
    async fn untracking_a_guest_releases_only_the_device_entry() {
        let app = in_memory_app();
        let identity = Identity::from("device-guest");
        app.create_room(identity.clone()).expect("room");
        let code = app_code(&app, &identity);
        app.untrack_if(&identity, &code);
        assert!(!app.players.lock().unwrap().contains_key("device-guest"));
        assert!(
            app.account_rooms.lock().unwrap().is_empty(),
            "a guest never held an account entry"
        );
    }

    // ------------------------------------------------------------------
    // 6.1 resolve_identity
    // ------------------------------------------------------------------

    #[test]
    fn a_missing_unknown_expired_or_revoked_token_all_resolve_to_a_guest() {
        let app = in_memory_app();
        let token = seed_account(&app, "Ana", "account-1", Duration::from_secs(3600));
        let dead = seed_account(&app, "Bob", "account-2", Duration::from_secs(0));
        let revoked = seed_account(&app, "Cleo", "account-3", Duration::from_secs(3600));
        let revoked_hash = hash_token(&revoked);
        assert!(app
            .auth()
            .revoke_session(&revoked_hash, now_ms()));

        for (label, presented) in [
            ("no token", None),
            ("an empty token", Some("")),
            ("an unknown token", Some("never-issued")),
            ("an expired session", Some(dead.as_str())),
            ("a revoked session", Some(revoked.as_str())),
        ] {
            let identity = app.resolve_identity("device-x", presented);
            assert_eq!(identity.account_id(), None, "{label} must play as a guest");
            assert_eq!(identity.device_id(), "device-x", "{label} keeps its device");
        }

        // And the live token does resolve, so the fallback above is a real
        // distinction and not the only outcome.
        assert_eq!(
            app.resolve_identity("device-x", Some(&token)).account_id(),
            Some("account-1")
        );
    }

    #[test]
    fn a_resolved_identity_keeps_the_connections_device_not_the_sessions() {
        let app = in_memory_app();
        let token = seed_account(&app, "Ana", "account-1", Duration::from_secs(3600));
        let identity = app.resolve_identity("device-tablet", Some(&token));
        assert_eq!(identity.account_id(), Some("account-1"));
        assert_eq!(
            identity.device_id(),
            "device-tablet",
            "the session was issued on another device; the account is what follows"
        );
    }
}
