//! Ports (design D1): the abstractions the application layer depends on
//! instead of concrete infrastructure — the authoritative chess engine, the
//! rating store, the room registry, and monotonic time.

use std::sync::Arc;
use std::time::Duration;

/// A single engine failure, as seen from the application layer: the
/// concrete error is mapped to a string by the infrastructure adapter.
pub type EngineError = String;

/// The authoritative chess engine (the `chess-core` sessions behind the FFI).
pub trait ChessEngine: Send + Sync {
    /// A fresh game session from the standard start position.
    fn new_game_session(&self) -> Box<dyn EngineSession>;
    /// A rating-only session seeded at `rating` (Glicko-2 bookkeeping).
    fn new_rating_session(&self, rating: f64) -> Box<dyn RatingSession>;
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
}

/// One rating session: read the current rating and record finished games.
pub trait RatingSession: Send + Sync {
    fn current_rating(&self) -> Result<f64, EngineError>;
    /// Record a game scored `score` (0.0–1.0) against `opponent_rating`.
    fn update_rating(&self, opponent_rating: f64, score: f64) -> Result<(), EngineError>;
}

/// The application-level view of the rating store (spec "Server Online
/// Rating"): per-device ratings and finished-game results.
pub trait Ratings: Send + Sync {
    fn rating_of(&self, player_id: &str) -> f64;
    fn apply_result(&self, winner: &str, loser: &str, score: f64);
}

/// The room registry the actor cleans up (spec "Server Room Management"):
/// removing a room and unbinding a device.
pub trait RoomRegistry: Send + Sync {
    fn remove_room(&self, code: &str);
    fn untrack_player_if(&self, player_id: &str, code: &str);
}

/// Monotonic time in milliseconds: the clock math's only time input
/// (design D2/D9), so tests can drive the clocks with fake values.
pub trait TimeSource: Send + Sync {
    /// A monotonic millisecond mark; only differences are meaningful.
    fn now_ms(&self) -> u64;
}

/// Everything the room actor needs from the outside (design D1): the
/// registry, the ratings, the engine, the time source, and the reconnect
/// grace window.
pub struct RoomServices {
    pub registry: Arc<dyn RoomRegistry>,
    pub ratings: Arc<dyn Ratings>,
    pub engine: Arc<dyn ChessEngine>,
    pub time: Arc<dyn TimeSource>,
    pub reconnect_grace: Duration,
}
