//! Ports (design D1): the abstractions the application layer depends on
//! instead of concrete infrastructure — the authoritative chess engine, the
//! rating store, the room registry, and monotonic time.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use crate::domain::account::{Account, Identity, Profile, Session};
use crate::domain::game_record::FinishedGame;

pub use crate::domain::rating::RatingState;

/// A single engine failure, as seen from the application layer: the
/// concrete error is mapped to a string by the infrastructure adapter.
pub type EngineError = String;

/// The authoritative chess engine (the `chess-core` sessions behind the FFI).
pub trait ChessEngine: Send + Sync {
    /// A fresh game session from the standard start position.
    fn new_game_session(&self) -> Box<dyn EngineSession>;
    /// A rating-only session seeded at `rating` (Glicko-2 bookkeeping).
    fn new_rating_session(&self, rating: f64) -> Box<dyn RatingSession>;
    /// A rating-only session restored from a full persisted Glicko-2 state.
    fn new_rating_session_state(&self, state: &RatingState) -> Box<dyn RatingSession>;
}

/// One authoritative game session: board queries and move application.
pub trait EngineSession: Send + Sync {
    /// Play a UCI move. `Err` when the move is not legal.
    fn play_move(&self, uci: &str) -> Result<(), EngineError>;
    /// The 8-field board FEN of the current position.
    fn board_state(&self) -> Result<String, EngineError>;
    /// Whether the position is checkmate.
    fn is_checkmate(&self) -> Result<bool, EngineError>;
    /// Whether the position is a draw by the core's rules.
    fn is_draw(&self) -> Result<bool, EngineError>;
    /// Whether the side (`true` = White) cannot deliver checkmate with its
    /// material: the flag-fall draw judgment ("A flag fall with insufficient
    /// material is a draw").
    fn insufficient_material_for(&self, winner_is_white: bool) -> Result<bool, EngineError>;
}

/// One rating session: read the current rating and record finished games.
pub trait RatingSession: Send + Sync {
    fn current_rating(&self) -> Result<f64, EngineError>;
    /// Record a game scored `score` (0.0–1.0) against `opponent_rating`.
    fn update_rating(&self, opponent_rating: f64, score: f64) -> Result<(), EngineError>;
    /// The session's player's full Glicko-2 state.
    fn state(&self) -> Result<RatingState, EngineError>;
}

/// The application-level view of the rating store (spec "Server Online
/// Rating"): per-device ratings and finished-game results.
pub trait Ratings: Send + Sync {
    fn rating_of(&self, player_id: &str) -> f64;
    fn apply_result(&self, winner: &str, loser: &str, score: f64);
    /// The device's full Glicko-2 state, if the store knows it.
    fn rating_state(&self, player_id: &str) -> Option<RatingState>;
}

/// A one-line reason a finished-game commit failed (spec "Server
/// Persistence"): the actor logs it and continues (design D7).
pub type RecorderError = String;

/// Durable recording of finished games (spec "Server Persistence",
/// design D3/D4): one idempotent transaction per finished game. The
/// `NoopGameRecorder` keeps in-memory runs and tests behavior-identical.
pub trait GameRecorder: Send + Sync {
    /// Commit a finished game durably.
    fn record_finished_game<'a>(
        &'a self,
        game: &'a FinishedGame,
    ) -> Pin<Box<dyn Future<Output = Result<(), RecorderError>> + Send + 'a>>;
}

/// The room registry the actor cleans up (spec "Server Room Management"):
/// removing a room and unbinding a player.
///
/// Identity-aware rather than device-aware (design D8/D9): an authenticated
/// player is tracked under *two* keys — its device on the guest path and its
/// account on the per-account path — so unbinding has to be able to release
/// both, and only the ones still pointing at this room.
pub trait RoomRegistry: Send + Sync {
    fn remove_room(&self, code: &str);
    /// Release the device entry, and the account entry when the identity has
    /// one, each only if it still names `code`. A stale rejection arriving
    /// after the player moved to another room must not unbind them there.
    fn untrack_if(&self, identity: &Identity, code: &str);
}

/// Monotonic time in milliseconds: the clock math's only time input
/// (design D2/D9), so tests can drive the clocks with fake values.
pub trait TimeSource: Send + Sync {
    /// A monotonic millisecond mark; only differences are meaningful.
    fn now_ms(&self) -> u64;
}

// -------------------------------------------------------------------------
// player-auth (design D4)
// -------------------------------------------------------------------------
//
// The same split `add-sqlite-persistence` uses for ratings: the in-memory
// traits below are the runtime authority and answer synchronously, while
// durability is a separate async port the caller logs and forgets. That is
// what makes "a failed authentication write MUST NOT block play" structural
// rather than a rule every call site has to remember.

/// Why an account write was refused by the store. These are integrity
/// failures, distinct from the credential errors a client is told about
/// (spec "Player Account Registration").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthError {
    /// The case-folded username already belongs to an account (design D3).
    DuplicateUsername,
    /// An account with this `account_id` already exists.
    DuplicateAccount,
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthError::DuplicateUsername => write!(f, "that username is taken"),
            AuthError::DuplicateAccount => write!(f, "that account already exists"),
        }
    }
}

/// Registered accounts and their device links (spec "player-auth").
///
/// Synchronous: this is in-memory state consulted while holding no lock
/// across an `await`, so a slow database never sits in the actor's path.
pub trait Accounts: Send + Sync {
    /// Look up by the *case-folded* username, which is what makes
    /// `Ana` and `ana` one account (design D3).
    fn account_by_username(&self, username: &str) -> Option<Account>;
    fn account_by_id(&self, account_id: &str) -> Option<Account>;
    /// Insert a new account, refusing a username or `account_id` that is
    /// already taken.
    fn insert_account(&self, account: Account) -> Result<(), AuthError>;
    /// The account a device is currently bound to, if any.
    fn account_id_of_device(&self, device_id: &str) -> Option<String>;
    /// Bind a device to an account, replacing any previous binding: a
    /// device follows the account it most recently signed in as (spec "Guest
    /// Play Fallback").
    fn link_device(&self, device_id: &str, account_id: &str);
    /// Set the account's display name (spec "Profile Display Name").
    fn set_display_name(&self, account_id: &str, display_name: &str);
}

/// The result of resolving a presented session token (design D7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionLookup {
    /// A live session: not expired, not revoked.
    Usable,
    /// No such session, or one whose expiry has passed. Both are reported
    /// identically so a token cannot be probed for existence.
    Absent,
    /// The session exists but has been revoked by a logout.
    Revoked,
}

/// Issued bearer sessions (spec "Session Lifetime, Reuse, and Revocation").
pub trait Sessions: Send + Sync {
    /// Resolve a token hash at `now_ms`, applying expiry as a check against
    /// `expires_at_ms` rather than as a stored flag, so there is no sweeper
    /// task whose opinion can drift from the row.
    fn session_by_token_hash(&self, token_hash: &str, now_ms: i64) -> SessionLookup;
    /// The session a usable token resolves to, for building an `Identity`.
    fn usable_session(&self, token_hash: &str, now_ms: i64) -> Option<Session>;
    /// The session record for a token hash regardless of whether it is still
    /// usable. Needed by logout, which must report *which* session ended even
    /// when it has already expired or been revoked.
    fn session_of_token_hash(&self, token_hash: &str) -> Option<Session>;
    fn insert_session(&self, session: Session);
    /// Revoke the session with this token hash, returning whether it was
    /// still live. Only the presented session is revoked, never the account's
    /// other sessions.
    fn revoke_session(&self, token_hash: &str, revoked_at_ms: i64) -> bool;
    /// Drop every session that expired at or before `now_ms`, returning how
    /// many went. Called once at startup (design D7).
    fn purge_expired(&self, now_ms: i64) -> usize;
}

/// The device-to-account links, kept as their own port so a consumer that
/// only needs to know *which account a device belongs to* is never handed a
/// token lookup by accident (design D4).
pub trait ProfileStore: Send + Sync {
    fn profile_of(&self, device_id: &str) -> Option<Profile>;
    fn create_profile(&self, profile: Profile);
}

/// Durable recording of authentication state (spec "Credential and Session
/// Data Protection", design D4): every method is one idempotent write, and a
/// failure is logged by the caller and never blocks play.
pub trait AuthRecorder: Send + Sync {
    fn record_account_created<'a>(
        &'a self,
        account: &'a Account,
    ) -> Pin<Box<dyn Future<Output = Result<(), RecorderError>> + Send + 'a>>;

    fn record_display_name_changed<'a>(
        &'a self,
        account_id: &'a str,
        display_name: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), RecorderError>> + Send + 'a>>;

    fn record_device_linked<'a>(
        &'a self,
        profile: &'a Profile,
    ) -> Pin<Box<dyn Future<Output = Result<(), RecorderError>> + Send + 'a>>;

    /// Persist an issued session. Only `session.token_hash` is written — the
    /// plaintext token exists in the response and nowhere else (design D6).
    fn record_session_issued<'a>(
        &'a self,
        session: &'a Session,
    ) -> Pin<Box<dyn Future<Output = Result<(), RecorderError>> + Send + 'a>>;

    fn record_session_revoked<'a>(
        &'a self,
        token_hash: &'a str,
        revoked_at_ms: i64,
    ) -> Pin<Box<dyn Future<Output = Result<(), RecorderError>> + Send + 'a>>;
}

/// The single authentication port `RoomServices` carries (design D4): one
/// field instead of three, so the construction fan-out is edited once.
pub trait PlayerAuth: Accounts + Sessions + ProfileStore {}

impl<T: Accounts + Sessions + ProfileStore> PlayerAuth for T {}

/// The room actor's view of the three authentication ports.
pub type SharedAuth = Arc<dyn PlayerAuth>;

/// Everything the room actor needs from the outside (design D1): the
/// registry, the ratings, the engine, the time source, the reconnect
/// grace window, the finished-game recorder, and authentication.
pub struct RoomServices {
    pub registry: Arc<dyn RoomRegistry>,
    pub ratings: Arc<dyn Ratings>,
    pub engine: Arc<dyn ChessEngine>,
    pub time: Arc<dyn TimeSource>,
    pub reconnect_grace: Duration,
    pub recorder: Arc<dyn GameRecorder>,
    pub auth: SharedAuth,
}
