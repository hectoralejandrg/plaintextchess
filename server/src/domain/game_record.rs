//! The finished-game record (spec "Server Persistence"): the complete,
//! timestamp-free description of a finished online game, captured at the
//! terminal transition and committed durably in one transaction.

use crate::domain::rating::RatingState;

/// A finished online game (add-sqlite-persistence D3/D4). The `game_id`
/// is the commit's idempotency key: replaying the same record changes
/// nothing.
#[derive(Debug, Clone, PartialEq)]
pub struct FinishedGame {
    /// A unique identifier (UUIDv4): the idempotency key of the commit.
    pub game_id: String,
    /// The room's code. Not unique across time; kept on the row for audit.
    pub room_code: String,
    /// The room's time-control label (e.g. `"15+10"`).
    pub time_control: String,
    pub white_device: String,
    pub black_device: String,
    /// The terminal status' wire name (e.g. `"checkmated"`, `"drawn"`).
    pub status: String,
    /// The winner's color (`"white"` / `"black"`), `None` for a draw.
    pub winner: Option<String>,
    /// The number of moves in the game.
    pub move_count: u32,
    /// Ratings as they stood before the final result was applied.
    pub white_rating_before: f64,
    pub black_rating_before: f64,
    /// Both players' post-game Glicko-2 state.
    pub white_state: RatingState,
    pub black_state: RatingState,
}
