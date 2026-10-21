//! Application state shared by the HTTP/WebSocket handlers (design D1,
//! infrastructure layer): the room registry (joinable rooms by code), the
//! one-room-per-device index, and the in-memory rating store.

#![allow(clippy::result_large_err)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use crate::application::ports::{
    Accounts, AuthError, AuthRecorder, AuthTokens, ChessEngine, EmailSender, GameRecorder,
    PlayerAuth, ProfileStore, RatingMirror, Ratings, RoomRegistry, RoomServices, SessionLookup,
    Sessions, TimeSource, TournamentRecorder,
};
use crate::infrastructure::broker::Broker;
use crate::application::room_actor::{run_room, RoomMsg};
use crate::application::tournament_actor::{
    run_tournament, TournamentHandle, TournamentHooks, TournamentMsg,
};
use crate::domain::account::Identity;
use crate::domain::rating::RatingState;
use crate::domain::room::{CODE_ALPHABET, CODE_LEN, is_valid_code};
use crate::domain::time_control::TimeControl;
use crate::domain::tournament::{Tournament, TournamentStatus};
use crate::infrastructure::auth::{NoopAuthRecorder, PlayerAuthStore};
use crate::infrastructure::auth_recorder::SqliteAuthRecorder;
use crate::infrastructure::email::{
    reset_email, reset_link, verification_email, verification_link, LogEmailSender,
    RESET_TOKEN_TTL_SECS, VERIFICATION_TOKEN_TTL_SECS,
};
use crate::infrastructure::net::Cidr;
use crate::infrastructure::password::generate_token;
use crate::infrastructure::rate_limit::{RateDecision, RateLimiter};
use crate::infrastructure::config::Config;
use crate::infrastructure::engine::CoreEngine;
use crate::infrastructure::password::{hash_token, now_ms, PasswordHasher};
use crate::infrastructure::ratings::RatingStore;
use crate::infrastructure::recorder::{
    MIGRATIONS, NoopGameRecorder, NoopRatingMirror, NoopTournamentRecorder, SqliteGameRecorder,
    SqliteTournamentRecorder,
};
use crate::interface::protocol::{ErrorCode, ServerMessage, TournamentSummary, VERSION};
use crate::domain::account::UsernameError;

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
    /// Optional broker for split-mode room ownership (Phase 3).
    /// Present when running as TIER=logic; None for TIER=all.
    broker: Option<Arc<Broker>>,
    /// Active Arena tournaments, by id (add-tournaments phase 1). A
    /// tournament's own task owns its live state; this map only routes
    /// `JoinTournament`/`ListTournaments` to the right mailbox.
    tournaments: Mutex<HashMap<String, TournamentHandle>>,
    tournament_recorder: Arc<dyn TournamentRecorder>,
    /// The transport that delivers the transactional messages (spec
    /// "Pluggable Email Delivery").
    email_sender: Arc<dyn EmailSender>,
    /// The attempt windows (spec "Authentication Rate Limiting").
    rate_limiter: Arc<RateLimiter>,
    /// The proxy networks whose forwarding header may be trusted.
    trusted_networks: Vec<Cidr>,
    /// A hash no account has, used to give an attempt with an unknown
    /// identifier the same cost as one with a wrong password (spec
    /// "Credential Attempts Do Not Reveal Account Existence").
    dummy_hash: std::sync::OnceLock<String>,
}

/// Who is making an authentication attempt (spec "Authentication Rate
/// Limiting").
#[derive(Debug, Clone, Default)]
pub struct AuthClient {
    /// The address the attempt is keyed by: the client's address as the
    /// reverse proxy saw it, or the peer's own address.
    pub address: String,
    /// The device the attempt is keyed by, when the client declared one.
    pub device_id: Option<String>,
}

/// The window a registration attempt is counted in.
const REGISTER_WINDOW_MS: i64 = 60 * 60 * 1000;
/// The window a sign-in attempt is counted in.
const LOGIN_WINDOW_MS: i64 = 60 * 1000;

impl RoomRegistry for App {
    fn remove_room(&self, code: &str) {
        self.rooms.lock().unwrap().remove(code);
        // Phase 3: release room ownership if we have a broker (split mode)
        if let Some(broker) = &self.broker {
            let code = code.to_string();
            let broker = Arc::clone(broker);
            tokio::spawn(async move {
                let _ = broker.release_room(&code).await;
            });
        }
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

/// Helper to set the broker on an already-constructed App (for split mode).
impl App {
    pub fn set_broker(&mut self, broker: Arc<Broker>) {
        self.broker = Some(broker);
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
            broker: None,
            tournaments: Mutex::new(HashMap::new()),
            tournament_recorder: Arc::new(NoopTournamentRecorder),
            // A run without a database delivers no mail: the tests that use
            // this constructor are not about mail.
            email_sender: Arc::new(LogEmailSender),
            rate_limiter: Arc::new(RateLimiter::new()),
            trusted_networks: Vec::new(),
            dummy_hash: std::sync::OnceLock::new(),
        }
    }

    /// Production constructor (spec "Server Persistence", design D6):
    /// open the configured database, run the versioned migrations, load
    /// the persisted per-device Glicko-2 state into the in-memory rating
    /// store, and attach the SQLite recorder. Fails fast — with an error,
    /// never a half-initialized app — when the database cannot be created,
    /// opened, or migrated.
    pub async fn init(config: Config) -> Result<Self, String> {
        // The transport is built from the configuration; the test hook below
        // is the only way to replace it.
        let email_sender = crate::infrastructure::email::from_config(&config)
            .map_err(|err| format!("the email transport is not usable: {err}"))?;
        Self::init_with_email_sender(config, email_sender).await
    }

    /// The production constructor with an explicit mail transport.
    ///
    /// A test injects a recording transport so it can read the messages the
    /// flows deliver; everything else behaves exactly as in `init`.
    pub async fn init_with_email_sender(
        config: Config,
        email_sender: Arc<dyn EmailSender>,
    ) -> Result<Self, String> {
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

        let rating_store = RatingStore::new(Arc::clone(&engine));
        for (device_id, rating, rating_deviation, volatility) in rows {
            rating_store.seed(
                &device_id,
                RatingState {
                    rating,
                    rating_deviation,
                    volatility,
                },
            );
        }
        let ratings = Arc::new(rating_store);

        let time: Arc<dyn TimeSource> = Arc::new(SystemTimeSource::new());
        let recorder: Arc<dyn GameRecorder> = Arc::new(SqliteGameRecorder::new(pool.clone()));
        let tournament_recorder: Arc<dyn TournamentRecorder> =
            Arc::new(SqliteTournamentRecorder::new(pool.clone()));

        // Authentication loads after the ratings and before anything can
        // accept a connection (task 9.1). The order matters in one place only:
        // expired session rows are purged from the database *first*, and what
        // is left is what the in-memory store is seeded with, so the two can
        // never disagree about which sessions exist.
        let started_at_ms = now_ms();
        let purged = SqliteAuthRecorder::purge_expired(&pool, started_at_ms)
            .await
            .map_err(|e| format!("purging expired sessions failed for `{database_url}`: {e}"))?;
        let purged_tokens = SqliteAuthRecorder::purge_expired_tokens(&pool, started_at_ms)
            .await
            .map_err(|e| format!("purging expired email tokens failed for `{database_url}`: {e}"))?;
        let (accounts, profiles, sessions, verification_tokens, reset_tokens) =
            SqliteAuthRecorder::load_all(&pool)
                .await
                .map_err(|e| {
                    format!("loading player accounts failed for `{database_url}`: {e}")
                })?;

        let trusted_networks = crate::infrastructure::net::trusted_networks(
            &config.auth_trusted_proxy_cidrs,
        );
        if config.production_mode && trusted_networks.is_empty() {
            // Behind the reverse proxy every player would share one bucket,
            // and the limit would fall on everybody (spec "Authentication
            // Rate Limiting").
            tracing::warn!(
                "AUTH_TRUSTED_PROXY_CIDRS is empty: every client behind the proxy shares one rate-limit bucket; set it to the proxy's network"
            );
        }

        let auth = Arc::new(PlayerAuthStore::new());
        for account in accounts {
            // A duplicate here is unreachable: `account_id` is the primary key
            // and the store starts empty. Logged rather than ignored silently
            // so a corrupt file is visible.
            if let Err(err) = auth.insert_account(account.clone()) {
                tracing::error!(
                    error = %err,
                    "a persisted account could not be loaded and was skipped"
                );
                continue;
            }
            // The pending-address reservations are rebuilt from the loaded
            // accounts: they are what keeps two accounts from claiming the
            // same address while their change is pending (spec "Email
            // Change").
            if let Some(key) = account.pending_email_key.clone() {
                auth.reserve_pending_email(&key, &account.account_id);
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
        // The email tokens that are still within their lifetime: a link sent
        // before a restart still works (spec "Email Verification", "Password
        // Reset by Email").
        for token in verification_tokens {
            auth.insert_verification_token(token);
        }
        for token in reset_tokens {
            auth.insert_reset_token(token);
        }
        if purged > 0 {
            tracing::info!(purged, "dropped expired sessions at startup");
        }
        if purged_tokens > 0 {
            tracing::info!(purged = purged_tokens, "dropped expired email tokens at startup");
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
            broker: None,
            email_sender,
            rate_limiter: Arc::new(RateLimiter::new()),
            trusted_networks,
            dummy_hash: std::sync::OnceLock::new(),
            tournaments: Mutex::new(HashMap::new()),
            tournament_recorder,
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

    /// The address an attempt is keyed by (spec "Authentication Rate
    /// Limiting").
    ///
    /// The forwarding header is used only when the peer is one of the
    /// configured trusted proxies; from any other peer the peer's own
    /// address is the answer.
    pub fn client_address(
        &self,
        peer: Option<std::net::SocketAddr>,
        forwarded: Option<&str>,
    ) -> String {
        crate::infrastructure::net::client_address(peer, forwarded, &self.trusted_networks)
    }

    /// The transport that delivers the transactional messages.
    pub fn email_sender(&self) -> &Arc<dyn EmailSender> {
        &self.email_sender
    }

    /// The authentication features this server supports (spec "Server Health
    /// and Configuration").
    ///
    /// The names are part of the contract: a client branches on them, so
    /// they cannot change once published.
    pub fn auth_features(&self) -> Vec<&'static str> {
        let mut features = vec![
            "password_reset",
            "email_verification",
            "email_change",
            "session_resumption",
        ];
        if self.config.hibp_enabled {
            features.push("breached_password_refusal");
        }
        features
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
    fn services(&self, this: &Arc<Self>, tournament: Option<TournamentHooks>) -> RoomServices {
        RoomServices {
            registry: Arc::clone(this) as Arc<dyn RoomRegistry>,
            ratings: Arc::clone(&self.ratings) as Arc<dyn Ratings>,
            engine: Arc::clone(&self.engine),
            time: Arc::clone(&self.time),
            reconnect_grace: self.config.reconnect_grace,
            recorder: Arc::clone(&self.recorder),
            rating_mirror: match &self.broker {
                Some(broker) => Arc::clone(broker) as Arc<dyn RatingMirror>,
                None => Arc::new(NoopRatingMirror),
            },
            auth: Arc::clone(&self.auth) as Arc<dyn PlayerAuth>,
            random_colors: self.config.random_colors,
            incremental_updates: self.config.incremental_updates,
            tournament,
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
        self.create_room_internal(player, time_control, None)
    }

    /// Create a room for an Arena pairing (add-tournaments phase 1): same
    /// path as `create_room_with_time_control`, except the spawned room's
    /// `RoomServices.tournament` is `Some`, so its terminal result updates
    /// the tournament's standings instead of the Glicko-2 rating. Called by
    /// the tournament actor, never by a client message directly.
    pub fn create_tournament_room(
        self: &Arc<Self>,
        player: impl Into<Identity>,
        time_control: TimeControl,
        hooks: TournamentHooks,
    ) -> Result<Conn, ServerMessage> {
        self.create_room_internal(player, time_control, Some(hooks))
    }

    fn create_room_internal(
        self: &Arc<Self>,
        player: impl Into<Identity>,
        time_control: TimeControl,
        tournament: Option<TournamentHooks>,
    ) -> Result<Conn, ServerMessage> {
        let identity = player.into();
        self.check_room_limit(&identity, None)?;
        let code = self.unique_code();
        let (mailbox_tx, mailbox_rx) = mpsc::unbounded_channel();
        let (out_tx, out_rx) = mpsc::unbounded_channel();

        let app = Arc::clone(self);
        let actor_code = code.clone();
        let services = self.services(&app, tournament);
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

    /// Create and start a new Arena tournament (add-tournaments phase 1):
    /// spawns its actor task and registers its handle so `JoinTournament`/
    /// `ListTournaments` can reach it. Manually triggered — there is no
    /// recurring scheduler yet (explicit non-goal of this phase).
    pub fn create_tournament(
        self: &Arc<Self>,
        time_control: TimeControl,
        starts_at_ms: i64,
        duration_ms: i64,
    ) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let tournament = Tournament {
            id: id.clone(),
            time_control,
            starts_at_ms,
            duration_ms,
            status: TournamentStatus::Scheduled,
        };
        let status = Arc::new(Mutex::new(TournamentStatus::Scheduled));
        let (tx, rx) = mpsc::unbounded_channel();
        self.tournaments.lock().unwrap().insert(
            id.clone(),
            TournamentHandle {
                mailbox: tx.clone(),
                status: Arc::clone(&status),
                time_control,
                starts_at_ms,
                duration_ms,
            },
        );
        let app = Arc::clone(self);
        let recorder = Arc::clone(&self.tournament_recorder);
        tokio::spawn(async move {
            run_tournament(tx, rx, app, tournament, status, recorder).await;
        });
        id
    }

    /// Join a tournament's waiting pool (add-tournaments phase 1): `paired_tx`
    /// is the channel the caller (the WebSocket handler) reads a `Conn` from
    /// once the tournament actor pairs this player into a game.
    pub fn join_tournament(
        &self,
        player: impl Into<Identity>,
        tournament_id: &str,
        paired_tx: mpsc::UnboundedSender<Conn>,
    ) -> Result<(), ServerMessage> {
        let identity = player.into();
        let mailbox = {
            let tournaments = self.tournaments.lock().unwrap();
            tournaments
                .get(tournament_id)
                .map(|handle| handle.mailbox.clone())
                .ok_or_else(|| ServerMessage::error(ErrorCode::TournamentNotFound))?
        };
        mailbox
            .send(TournamentMsg::Join { identity, paired_tx })
            .map_err(|_| ServerMessage::error(ErrorCode::TournamentNotFound))
    }

    /// Leave a tournament's waiting pool. Does not affect a game already in
    /// progress (leaving that uses `RoomMsg::Leave`, same as a casual room).
    pub fn leave_tournament(&self, player_id: &str, tournament_id: &str) {
        let mailbox = self
            .tournaments
            .lock()
            .unwrap()
            .get(tournament_id)
            .map(|handle| handle.mailbox.clone());
        if let Some(mailbox) = mailbox {
            let _ = mailbox.send(TournamentMsg::Leave {
                player_id: player_id.to_string(),
            });
        }
    }

    /// The tournaments the server currently knows about (no lobby UI yet;
    /// this is the discovery API for testing, add-tournaments phase 1).
    pub fn list_tournaments(&self) -> Vec<TournamentSummary> {
        self.tournaments
            .lock()
            .unwrap()
            .iter()
            .map(|(id, handle)| TournamentSummary {
                tournament_id: id.clone(),
                status: match *handle.status.lock().unwrap() {
                    TournamentStatus::Scheduled => "scheduled".to_string(),
                    TournamentStatus::Running => "running".to_string(),
                    TournamentStatus::Finished => "finished".to_string(),
                },
                time_control: handle.time_control.label().to_string(),
                starts_at_ms: handle.starts_at_ms,
                duration_ms: handle.duration_ms,
            })
            .collect()
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

/// Auth methods for the logic tier (extracted from ws.rs for reuse).
impl App {
    /// Register a new account (spec "Player Account Registration").
    ///
    /// The username, the email address, and the password are all required:
    /// the email is the account's private identity, and the address is where
    /// a password reset would be sent.
    pub async fn register(
        self: &Arc<Self>,
        username: &str,
        email: Option<&str>,
        password: &str,
        device_id: Option<String>,
        client: &AuthClient,
    ) -> ServerMessage {
        use crate::domain::account::{
            email_key, is_rejected_username, password_matches_username, validate_email,
            validate_password, validate_username,
        };

        // The limit runs first: a refused attempt must not cost an argon2
        // pass (spec "Authentication Rate Limiting").
        if let Some(refusal) = self.rate_limited(
            "register",
            client,
            self.config.auth_rate_limit_register_per_hour,
            REGISTER_WINDOW_MS,
        ) {
            return refusal;
        }

        // Validate before hashing, so a rejected input costs nothing (spec
        // "Credential and Session Data Protection").
        let username = match validate_username(username) {
            Ok(username) => username,
            Err(err) => return self.username_error_message(err),
        };
        let Some(raw_email) = email else {
            return ErrorCode::InvalidRequest
                .with_field("email", "an email address is required to create an account");
        };
        let email = match validate_email(raw_email) {
            Ok(email) => email,
            Err(err) => return self.email_error_message(err),
        };
        if validate_password(password).is_err() {
            return self.password_error_message();
        }
        if password_matches_username(password, &username) {
            return ErrorCode::InvalidRequest
                .with_field("password", "the password must not be the username");
        }
        if is_rejected_username(&username) {
            // The reject list is a policy, not a security boundary: the
            // refusal names the field and nothing else (spec "Username
            // Uniqueness and Rejection").
            tracing::info!(username = %username, "registration refused: the username is on the reject list");
            return ErrorCode::InvalidRequest
                .with_field("username", "that username cannot be used");
        }

        // The mail-domain check is advisory: it never refuses, and it must
        // not hold the request up, so it runs in a blocking task (spec
        // "Email Identity").
        {
            let address = email.clone();
            tokio::task::spawn_blocking(move || {
                crate::infrastructure::email::domain::check_and_record(&address);
            });
        }

        let store = self.auth();
        if store.account_by_username(&username).is_some() {
            // Checked before any hashing, so a taken username cannot be used
            // to make the server spend argon2 time.
            tracing::info!(username = %username, "registration refused: the username is taken");
            return ErrorCode::UsernameTaken.with_field("username", ErrorCode::UsernameTaken.message());
        }
        let email_key = email_key(&email);
        if store.account_by_email(&email_key).is_some() {
            tracing::info!("registration refused: the email address is taken");
            return ErrorCode::EmailTaken.with_field("email", ErrorCode::EmailTaken.message());
        }

        // The breach check (spec "Breached Password Refusal"). It runs
        // before the account is created, so a breached password never
        // reaches the database.
        if self.config.hibp_enabled && self.is_breached(password).await {
            return ErrorCode::InvalidRequest.with_field(
                "password",
                "that password appears in a known data breach; choose another one",
            );
        }

        let tooling = self.auth_tooling();
        let password_hash = match tooling.hasher.hash(password).await {
            Ok(hash) => hash,
            Err(err) => {
                tracing::error!(error = %err, "password hashing failed during registration");
                return ServerMessage::error(ErrorCode::InvalidRequest);
            }
        };

        let account = crate::domain::account::Account {
            account_id: uuid::Uuid::new_v4().to_string(),
            username: username.clone(),
            username_key: crate::domain::account::username_key(&username),
            password_hash,
            display_name: None,
            email: Some(email.clone()),
            email_key: Some(email_key),
            email_verified: false,
            pending_email: None,
            pending_email_key: None,
            created_at_ms: now_ms(),
        };
        if let Err(err) = store.insert_account(account.clone()) {
            tracing::info!(username = %username, error = %err, "registration refused");
            return match err {
                AuthError::DuplicateEmail => {
                    ErrorCode::EmailTaken.with_field("email", ErrorCode::EmailTaken.message())
                }
                _ => ErrorCode::UsernameTaken
                    .with_field("username", ErrorCode::UsernameTaken.message()),
            };
        }
        let recorder = self.auth_recorder().clone();
        let written = account.clone();
        self.persist_auth(recorder, account.account_id.clone(), move |recorder| {
            Box::pin(async move { recorder.record_account_created(&written).await })
        })
        .await;

        // A device known at registration time is bound right away (spec
        // "Device-to-Account Profile Linkage"); otherwise the first room
        // message carrying this session's token binds it.
        if let Some(device) = device_id.as_deref() {
            self.link_device_to(device, &account.account_id).await;
        }

        let (token, expires_at_ms) = self.issue_session(&account, device_id.as_deref()).await;

        // The verification message (spec "Email Verification"). A delivery
        // that fails does not undo the account: the player can ask again.
        let address = account.email.clone().unwrap_or_default();
        self.send_verification(&account, &address).await;

        tracing::info!(
            account_id = %account.account_id,
            username = %account.username,
            "registration succeeded"
        );
        self.session_message(&account, &token, expires_at_ms)
    }

    /// Sign in with an identifier (spec "Login and Session Issuance").
    ///
    /// The identifier is the account's username or its email address.
    pub async fn login(
        self: &Arc<Self>,
        identifier: &str,
        password: &str,
        device_id: Option<String>,
        client: &AuthClient,
    ) -> ServerMessage {
        use crate::domain::account::validate_password;

        if let Some(refusal) = self.rate_limited(
            "login",
            client,
            self.config.auth_rate_limit_login_per_min,
            LOGIN_WINDOW_MS,
        ) {
            return refusal;
        }

        // A password outside the accepted length never reaches argon2 (spec
        // "Credential and Session Data Protection").
        if validate_password(password).is_err() {
            // Reported as a credential failure, not a format failure: a login
            // asserts something about existing data, and "too short" would
            // tell an attacker a fact about the account they do not otherwise
            // get.
            tracing::info!("login refused: the password is outside the accepted length");
            return ServerMessage::error(ErrorCode::InvalidCredentials);
        }

        let store = self.auth();
        let account = store
            .account_by_username(identifier)
            .or_else(|| store.account_by_email(identifier));
        let Some(account) = account else {
            // The same work as a wrong password, so the time the reply takes
            // does not disclose whether the account exists (spec "Credential
            // Attempts Do Not Reveal Account Existence").
            self.dummy_verification(password).await;
            tracing::info!("login refused: no such account");
            return ServerMessage::error(ErrorCode::InvalidCredentials);
        };

        let tooling = self.auth_tooling();
        if !tooling
            .hasher
            .verify(password, &account.password_hash)
            .await
        {
            tracing::info!(account_id = %account.account_id, "login refused: wrong password");
            return ServerMessage::error(ErrorCode::InvalidCredentials);
        }

        // A device that belonged to another account follows this one (spec
        // "Guest Play Fallback", "Device-to-Account Profile Linkage").
        if let Some(device) = device_id.as_deref() {
            self.link_device_to(device, &account.account_id).await;
        }

        let (token, expires_at_ms) = self.issue_session(&account, device_id.as_deref()).await;
        tracing::info!(
            account_id = %account.account_id,
            username = %account.username,
            "login succeeded"
        );
        self.session_message(&account, &token, expires_at_ms)
    }

    /// Resume a session the client stored earlier (spec "Session
    /// Resumption").
    ///
    /// No new token is issued and the session's lifetime is not extended:
    /// this is the same session the client already had.
    pub fn resume_session(self: &Arc<Self>, token: &str) -> ServerMessage {
        let store = self.auth();
        let token_hash = hash_token(token);
        let now_ms = now_ms();

        let Some(session) = store.session_of_token_hash(&token_hash) else {
            tracing::info!("resumption presented a token the server has never issued");
            return ServerMessage::error(ErrorCode::NotAuthenticated);
        };
        match store.session_by_token_hash(&token_hash, now_ms) {
            SessionLookup::Usable => {}
            SessionLookup::Revoked => {
                tracing::info!(account_id = %session.account_id, "resumption with a revoked session");
                return ServerMessage::error(ErrorCode::NotAuthenticated);
            }
            SessionLookup::Absent => {
                tracing::info!(account_id = %session.account_id, "resumption with an expired session");
                return ServerMessage::error(ErrorCode::SessionExpired);
            }
        }

        let Some(account) = store.account_by_id(&session.account_id) else {
            return ServerMessage::error(ErrorCode::NotAuthenticated);
        };
        tracing::info!(account_id = %account.account_id, "session resumed");
        ServerMessage::SessionResumed {
            v: VERSION,
            account_id: account.account_id.clone(),
            username: account.username.clone(),
            display_name: account.reported_name().to_string(),
            expires_at_ms: session.expires_at_ms,
            email_verified: account.email_verified,
        }
    }

    /// Ask for a password-reset link (spec "Password Reset by Email").
    ///
    /// The answer is the same whether or not the address belongs to an
    /// account, so the requester cannot learn which addresses are
    /// registered.
    pub async fn forgot_password(self: &Arc<Self>, email: &str, client: &AuthClient) -> ServerMessage {
        use crate::domain::account::validate_email;

        if let Some(refusal) = self.rate_limited(
            "login",
            client,
            self.config.auth_rate_limit_login_per_min,
            LOGIN_WINDOW_MS,
        ) {
            return refusal;
        }

        // A malformed address is answered with the same confirmation: the
        // request is not about an account, it is about a message.
        let Ok(email) = validate_email(email) else {
            return ServerMessage::PasswordResetRequested { v: VERSION };
        };

        let store = self.auth();
        let Some(account) = store.account_by_email(&email) else {
            // Nothing is sent, and nothing is said: the answer is the same
            // one a registered address gets.
            tracing::info!("password reset requested for an address no account holds");
            return ServerMessage::PasswordResetRequested { v: VERSION };
        };
        let Some(raw_email) = account.email.clone() else {
            return ServerMessage::PasswordResetRequested { v: VERSION };
        };

        let token = generate_token();
        let token_hash = hash_token(&token);
        let created_at_ms = now_ms();
        let expires_at_ms = created_at_ms + RESET_TOKEN_TTL_SECS * 1000;
        let record = crate::domain::account::ResetToken {
            account_id: account.account_id.clone(),
            token_hash: token_hash.clone(),
            created_at_ms,
            expires_at_ms,
            used_at_ms: None,
        };
        store.insert_reset_token(record.clone());
        let recorder = self.auth_recorder().clone();
        let written = record.clone();
        let account_id = account.account_id.clone();
        self.persist_auth(recorder, account_id.clone(), move |recorder| {
            Box::pin(async move { recorder.record_reset_token(&written).await })
        })
        .await;

        let link = reset_link(&self.config.public_base_url, &token);
        let message = reset_email(&raw_email, &account.username, &link);
        // The delivery happens in the background: the answer must not take
        // longer for a registered address than for an unregistered one, or
        // the timing tells the requester which addresses exist (spec
        // "Password Reset by Email").
        let sender = Arc::clone(self.email_sender());
        let for_account = account_id.clone();
        tokio::spawn(async move {
            if let Err(err) = sender.send(&message).await {
                // The token exists but was not delivered: the player asks
                // again and a fresh token is issued.
                tracing::warn!(error = %err, account_id = %for_account, "the password reset message could not be delivered");
            }
        });
        tracing::info!(%account_id, "a password reset was requested");
        ServerMessage::PasswordResetRequested { v: VERSION }
    }

    /// Set a new password with a reset token (spec "Password Reset by
    /// Email"). No session is issued: the player signs in with the new
    /// password.
    pub async fn reset_password(self: &Arc<Self>, token: &str, password: &str) -> ServerMessage {
        use crate::domain::account::validate_password;

        if validate_password(password).is_err() {
            return self.password_error_message();
        }

        let store = self.auth();
        let token_hash = hash_token(token);
        let now_ms = now_ms();
        // The token is looked up, not spent, while the password is checked:
        // a refused password must not burn the player's only link (spec
        // "Password Reset by Email").
        let Some(record) = store.reset_token_by_hash(&token_hash, now_ms) else {
            tracing::info!("a password reset presented a token that is unknown, expired, or already used");
            return ServerMessage::error(ErrorCode::InvalidResetToken);
        };
        let Some(account) = store.account_by_id(&record.account_id) else {
            return ServerMessage::error(ErrorCode::InvalidResetToken);
        };
        if crate::domain::account::password_matches_username(password, &account.username) {
            return ErrorCode::InvalidRequest
                .with_field("password", "the password must not be the username");
        }

        if self.config.hibp_enabled && self.is_breached(password).await {
            return ErrorCode::InvalidRequest.with_field(
                "password",
                "that password appears in a known data breach; choose another one",
            );
        }

        // The policy passed: now the token is spent.
        if store.consume_reset_token(&token_hash, now_ms).is_none() {
            return ServerMessage::error(ErrorCode::InvalidResetToken);
        }

        let tooling = self.auth_tooling();
        let password_hash = match tooling.hasher.hash(password).await {
            Ok(hash) => hash,
            Err(err) => {
                tracing::error!(error = %err, "password hashing failed during a password reset");
                return ServerMessage::error(ErrorCode::InvalidRequest);
            }
        };
        store.set_password_hash(&account.account_id, &password_hash);
        let recorder = self.auth_recorder().clone();
        let account_id = account.account_id.clone();
        let hash = password_hash.clone();
        self.persist_auth(recorder, account_id.clone(), move |recorder| {
            Box::pin(async move { recorder.record_password_changed(&account_id, &hash).await })
        })
        .await;
        // The token is spent, in the database too.
        let recorder = self.auth_recorder().clone();
        let spent = token_hash.clone();
        self.persist_auth(recorder, account.account_id.clone(), move |recorder| {
            Box::pin(async move { recorder.record_token_used(&spent, now_ms).await })
        })
        .await;

        tracing::info!(account_id = %account.account_id, "the password was reset");
        ServerMessage::PasswordReset {
            v: VERSION,
            account_id: account.account_id,
        }
    }

    /// Confirm an email address (spec "Email Verification").
    pub async fn verify_email(self: &Arc<Self>, token: &str) -> ServerMessage {
        let store = self.auth();
        let token_hash = hash_token(token);
        let now_ms = now_ms();
        let Some(record) = store.consume_verification_token(&token_hash, now_ms) else {
            tracing::info!("a verification presented a token that is unknown, expired, or already used");
            return ErrorCode::InvalidVerificationToken
                .with_message("that confirmation link is no longer valid");
        };
        let Some(account) = store.account_by_id(&record.account_id) else {
            return ErrorCode::InvalidVerificationToken
                .with_message("that confirmation link is no longer valid");
        };

        // A token confirms the address it was mailed to, and no other: a
        // token sent to the old address cannot confirm a new one (spec
        // "Email Change").
        let pending = account.pending_email.clone();
        let current = account.email.clone();
        let confirmed_address = current.clone().filter(|address| Some(address) == pending.as_ref())
            .or_else(|| current.clone().filter(|address| address == &record.address))
            .or_else(|| pending.clone().filter(|address| address == &record.address));
        if confirmed_address.is_none() {
            tracing::info!(
                account_id = %account.account_id,
                "a confirmation presented a token that belongs to another address"
            );
            return ErrorCode::InvalidVerificationToken
                .with_message("that confirmation link is no longer valid");
        }

        // A pending address takes over when it is confirmed: it becomes the
        // account's address and the old one stops receiving its mail (spec
        // "Email Change"). Without a pending address, the confirmation is
        // the one the account was created with.
        let pending = account.pending_email.clone();
        let recorder = self.auth_recorder().clone();
        match pending {
            Some(confirmed) => {
                let key = crate::domain::account::email_key(&confirmed);
                store.confirm_pending_email(&account.account_id);
                let account_id = account.account_id.clone();
                self.persist_auth(recorder, account_id.clone(), move |recorder| {
                    Box::pin(async move {
                        recorder
                            .record_pending_email_confirmed(&account_id, &confirmed, &key)
                            .await
                    })
                })
                .await;
            }
            None => {
                store.set_email_verified(&account.account_id, true);
                let account_id = account.account_id.clone();
                self.persist_auth(recorder, account_id.clone(), move |recorder| {
                    Box::pin(async move { recorder.record_email_verified(&account_id, true).await })
                })
                .await;
            }
        }
        let recorder = self.auth_recorder().clone();
        let spent = token_hash.clone();
        self.persist_auth(recorder, account.account_id.clone(), move |recorder| {
            Box::pin(async move { recorder.record_token_used(&spent, now_ms).await })
        })
        .await;

        tracing::info!(account_id = %account.account_id, "the email address was confirmed");
        ServerMessage::EmailVerified {
            v: VERSION,
            account_id: account.account_id,
            email_verified: true,
        }
    }

    /// Ask for another verification message (spec "Email Verification").
    ///
    /// The new token retires the previous one, and the account keeps its
    /// state either way.
    pub async fn resend_verification(
        self: &Arc<Self>,
        token: &str,
        client: &AuthClient,
    ) -> ServerMessage {
        // A resend is a message to an address, so it is limited like the
        // other message-sending requests (spec "Authentication Rate
        // Limiting").
        if let Some(refusal) = self.rate_limited(
            "resend",
            client,
            self.config.auth_rate_limit_login_per_min,
            LOGIN_WINDOW_MS,
        ) {
            return refusal;
        }
        let Some(account) = self.account_of_token(token) else {
            return ServerMessage::error(ErrorCode::NotAuthenticated);
        };
        // The message goes to the address that is being confirmed: the
        // pending one when a change is in flight, the current one otherwise.
        let address = account
            .pending_email
            .clone()
            .or_else(|| account.email.clone());
        let Some(address) = address else {
            // The account has no address to confirm.
            return ServerMessage::error(ErrorCode::EmailNotVerified);
        };
        self.send_verification(&account, &address).await;
        tracing::info!(account_id = %account.account_id, "another verification message was requested");
        ServerMessage::EmailVerificationRequested { v: VERSION }
    }

    /// Ask for a different email address (spec "Email Change").
    ///
    /// The current address stays in force until the new one is confirmed.
    pub async fn change_email(
        self: &Arc<Self>,
        token: &str,
        email: &str,
        client: &AuthClient,
    ) -> ServerMessage {
        use crate::domain::account::{email_key, validate_email};

        // A change sends a message to an address the player chose, so it is
        // limited like the other message-sending requests: without a limit
        // it is a way to mail anyone (spec "Authentication Rate Limiting").
        if let Some(refusal) = self.rate_limited(
            "change",
            client,
            self.config.auth_rate_limit_login_per_min,
            LOGIN_WINDOW_MS,
        ) {
            return refusal;
        }
        let Some(account) = self.account_of_token(token) else {
            return ServerMessage::error(ErrorCode::NotAuthenticated);
        };
        let email = match validate_email(email) {
            Ok(email) => email,
            Err(err) => return self.email_error_message(err),
        };
        let email_key = email_key(&email);
        if self.auth().account_by_email(&email_key).is_some() {
            tracing::info!("an email change was refused: the address is taken");
            return ServerMessage::error(ErrorCode::EmailTaken);
        }

        if let Err(err) = self
            .auth()
            .set_pending_email(&account.account_id, &email, &email_key)
        {
            tracing::info!(error = %err, "an email change was refused: the address is taken");
            return ServerMessage::error(ErrorCode::EmailTaken);
        }
        let recorder = self.auth_recorder().clone();
        let account_id = account.account_id.clone();
        let key = email_key.clone();
        let address = email.clone();
        self.persist_auth(recorder, account_id.clone(), move |recorder| {
            Box::pin(async move {
                recorder
                    .record_pending_email(&account_id, &address, &key)
                    .await
            })
        })
        .await;

        // The confirmation token is stored, so the link that goes to the new
        // address can actually confirm it (spec "Email Change"). It replaces
        // the account's previous token, as a resend does.
        let token = generate_token();
        let token_hash = hash_token(&token);
        let created_at_ms = now_ms();
        let expires_at_ms = created_at_ms + VERIFICATION_TOKEN_TTL_SECS * 1000;
        let record = crate::domain::account::VerificationToken {
            account_id: account.account_id.clone(),
            // The token is bound to the address it is sent to.
            address: email.clone(),
            token_hash: token_hash.clone(),
            created_at_ms,
            expires_at_ms,
            used_at_ms: None,
        };
        self.auth().insert_verification_token(record.clone());
        let recorder = self.auth_recorder().clone();
        let written = record.clone();
        self.persist_auth(recorder, account.account_id.clone(), move |recorder| {
            Box::pin(async move { recorder.record_verification_token(&written).await })
        })
        .await;

        // The message goes to the address being moved to; the current one
        // stays in force until it is confirmed.
        let link = verification_link(&self.config.public_base_url, &token);
        let message = verification_email(&email, &account.username, &link);
        if let Err(err) = self.email_sender.send(&message).await {
            tracing::warn!(error = %err, "the confirmation message could not be delivered");
        }
        tracing::info!(account_id = %account.account_id, "an email change was requested");
        ServerMessage::EmailVerificationRequested { v: VERSION }
    }

    /// The account a session token resolves to, when the token is usable.
    fn account_of_token(&self, token: &str) -> Option<crate::domain::account::Account> {
        let store = self.auth();
        let token_hash = hash_token(token);
        let session = store.usable_session(&token_hash, now_ms())?;
        store.account_by_id(&session.account_id)
    }

    /// Deliver the verification message for an account (spec "Email
    /// Verification").
    ///
    /// The message goes to `address`, and the token is bound to it: a token
    /// confirms the address it was mailed to and no other.
    ///
    /// A delivery that fails is logged and the account survives: the player
    /// can ask for another message.
    async fn send_verification(
        &self,
        account: &crate::domain::account::Account,
        address: &str,
    ) {
        let token = generate_token();
        let token_hash = hash_token(&token);
        let created_at_ms = now_ms();
        let expires_at_ms = created_at_ms + VERIFICATION_TOKEN_TTL_SECS * 1000;
        let record = crate::domain::account::VerificationToken {
            account_id: account.account_id.clone(),
            address: address.to_string(),
            token_hash: token_hash.clone(),
            created_at_ms,
            expires_at_ms,
            used_at_ms: None,
        };
        self.auth().insert_verification_token(record.clone());
        let recorder = self.auth_recorder().clone();
        let written = record.clone();
        self.persist_auth(recorder, account.account_id.clone(), move |recorder| {
            Box::pin(async move { recorder.record_verification_token(&written).await })
        })
        .await;

        let link = verification_link(&self.config.public_base_url, &token);
        let message = verification_email(address, &account.username, &link);
        if let Err(err) = self.email_sender.send(&message).await {
            tracing::warn!(
                error = %err,
                account_id = %account.account_id,
                "the verification message could not be delivered"
            );
        }
    }

    /// Whether the password appears in a known breach (spec "Breached
    /// Password Refusal"). The blocking call runs in a blocking task, and a
    /// corpus that cannot be reached leaves the password alone.
    async fn is_breached(&self, password: &str) -> bool {
        let password = password.to_string();
        tokio::task::spawn_blocking(move || crate::infrastructure::breach::is_breached(&password))
            .await
            .unwrap_or(false)
    }

    /// A verification against a hash no account has, so an attempt with an
    /// unknown identifier costs the same as one with a wrong password (spec
    /// "Credential Attempts Do Not Reveal Account Existence").
    async fn dummy_verification(&self, password: &str) {
        let tooling = self.auth_tooling();
        let hash = match self.dummy_hash.get() {
            Some(hash) => hash.clone(),
            None => {
                let random = generate_token();
                match tooling.hasher.hash(&random).await {
                    Ok(hash) => {
                        let _ = self.dummy_hash.set(hash.clone());
                        hash
                    }
                    Err(_) => return,
                }
            }
        };
        let _ = tooling.hasher.verify(password, &hash).await;
    }

    /// Whether the client may make another attempt, recording it when it
    /// may (spec "Authentication Rate Limiting").
    fn rate_limited(
        &self,
        action: &str,
        client: &AuthClient,
        limit: u32,
        window_ms: i64,
    ) -> Option<ServerMessage> {
        if limit == 0 {
            return None;
        }
        let now_ms = now_ms();
        // The keys are namespaced per action: a player who signs in often
        // must not fill the registration window, and the other way round.
        let mut keys = vec![format!("{action}:addr:{}", client.address)];
        if let Some(device) = client.device_id.as_deref() {
            keys.push(format!("{action}:device:{device}"));
        }
        // The check and the record are one operation: concurrent attempts
        // would otherwise all pass the check before any of them was recorded.
        for key in &keys {
            if let RateDecision::Refused { retry_after_secs } =
                self.rate_limiter.check_and_record(key, limit, window_ms, now_ms)
            {
                return Some(ErrorCode::TooManyAttempts.with_message(format!(
                    "too many attempts; try again in {retry_after_secs} seconds"
                )));
            }
        }
        None
    }

    /// Logout (mirrors ws.rs::logout).
    pub async fn logout(self: &Arc<Self>, token: &str) -> ServerMessage {
        let store = self.auth();
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

        store.revoke_session(&token_hash, now_ms);
        let recorder = self.auth_recorder().clone();
        let hash = token_hash.clone();
        let revoked_for = session.account_id.clone();
        // The revocation is persisted before the answer: a crash between the
        // reply and the write would leave the session revoked in memory but
        // alive in the database, and it would authenticate again after a
        // restart.
        self.persist_auth(recorder, revoked_for, move |recorder| {
            Box::pin(async move { recorder.record_session_revoked(&hash, now_ms).await })
        })
        .await;

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

    /// Set profile (mirrors ws.rs::set_profile).
    pub async fn set_profile(
        self: &Arc<Self>,
        display_name: &str,
        token: &str,
    ) -> ServerMessage {
        use crate::domain::account::validate_display_name;

        let store = self.auth();
        let token_hash = hash_token(token);
        let now_ms = now_ms();

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

        let display_name = match validate_display_name(display_name) {
            Ok(name) => name,
            Err(err) => {
                tracing::info!(
                    account_id = %session.account_id,
                    reason = ?err,
                    "display name refused: outside the accepted format"
                );
                return ErrorCode::InvalidDisplayName
                    .with_field("display_name", ErrorCode::InvalidDisplayName.message());
            }
        };

        store.set_display_name(&session.account_id, &display_name);
        let recorder = self.auth_recorder().clone();
        let account_id = session.account_id.clone();
        let name = display_name.clone();
        self.persist_auth(recorder, account_id.clone(), move |recorder| {
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

    fn session_message(&self, account: &crate::domain::account::Account, token: &str, expires_at_ms: i64) -> ServerMessage {
        ServerMessage::Session {
            v: VERSION,
            account_id: account.account_id.clone(),
            username: account.username.clone(),
            display_name: account.reported_name().to_string(),
            token: token.to_string(),
            expires_at_ms,
            email_verified: account.email_verified,
        }
    }

    fn username_error_message(&self, err: crate::domain::account::UsernameError) -> ServerMessage {
        use crate::domain::account::{MAX_USERNAME_LEN, MIN_USERNAME_LEN};
        let detail = match err {
            UsernameError::Length => {
                format!("username must be {MIN_USERNAME_LEN} to {MAX_USERNAME_LEN} characters")
            }
            UsernameError::Charset => {
                "username may only contain letters, digits, underscore, and hyphen".to_string()
            }
            UsernameError::Shape => {
                "username must start with a letter, end with a letter or a digit, and carry no repeated separator"
                    .to_string()
            }
        };
        // The message names the offending field, and the error carries it
        // machine-readably (spec "Authentication Error Codes").
        ErrorCode::InvalidRequest.with_field("username", detail)
    }

    fn email_error_message(&self, err: crate::domain::account::EmailError) -> ServerMessage {
        use crate::domain::account::EmailError;
        let detail = match err {
            EmailError::Shape => "that is not a valid email address".to_string(),
            EmailError::Domain => "the email address must name a domain".to_string(),
            EmailError::Length => "that email address is too long".to_string(),
        };
        ErrorCode::InvalidRequest.with_field("email", detail)
    }

    fn password_error_message(&self) -> ServerMessage {
        use crate::domain::account::{MAX_PASSWORD_LEN, MIN_PASSWORD_LEN};
        ErrorCode::InvalidRequest.with_field(
            "password",
            format!("password must be {MIN_PASSWORD_LEN} to {MAX_PASSWORD_LEN} characters"),
        )
    }

    async fn issue_session(
        self: &Arc<Self>,
        account: &crate::domain::account::Account,
        device_id: Option<&str>,
    ) -> (String, i64) {
        use crate::infrastructure::password::{expires_at, generate_token, now_ms};

        let store = self.auth();
        let tooling = self.auth_tooling();
        let token = generate_token();
        let issued_at_ms = now_ms();
        let expires_at_ms = expires_at(issued_at_ms, tooling.session_ttl);
        let session = crate::domain::account::Session {
            session_id: uuid::Uuid::new_v4().to_string(),
            account_id: account.account_id.clone(),
            device_id: device_id.unwrap_or_default().to_string(),
            token_hash: hash_token(&token),
            created_at_ms: issued_at_ms,
            expires_at_ms,
            revoked_at_ms: None,
        };
        store.insert_session(session.clone());
        let recorder = self.auth_recorder().clone();
        let account_id = account.account_id.clone();
        self.persist_auth(recorder, account_id, move |recorder| {
            Box::pin(async move { recorder.record_session_issued(&session).await })
        })
        .await;
        (token, expires_at_ms)
    }

    async fn persist_auth<F, Fut>(&self, recorder: Arc<dyn crate::application::ports::AuthRecorder>, account_id: String, make: F)
    where
        F: FnOnce(Arc<dyn crate::application::ports::AuthRecorder>) -> Fut,
        Fut: std::future::Future<Output = Result<(), crate::application::ports::RecorderError>>,
    {
        if let Err(err) = make(recorder).await {
            tracing::error!(
                error = %err,
                %account_id,
                "failed to persist an authentication change; it is applied in memory only"
            );
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
            email: None,
            email_key: None,
            email_verified: false,
            pending_email: None,
            pending_email_key: None,
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
                email: None,
                email_key: None,
                email_verified: false,
                pending_email: None,
                pending_email_key: None,
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
