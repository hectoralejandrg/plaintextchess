pub mod domain;

use std::sync::{Arc, Mutex};

/// Errors exposed across the FFI boundary.
/// Surface defined in `openspec/specs/shared/ffi-contracts.md`.
#[derive(uniffi::Error, Debug)]
pub enum ChessError {
    InvalidMove,
    BoardError,
    RatingError,
    FfiError,
}

impl std::fmt::Display for ChessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChessError::InvalidMove => write!(f, "Invalid move"),
            ChessError::BoardError => write!(f, "Board error"),
            ChessError::RatingError => write!(f, "Rating error"),
            ChessError::FfiError => write!(f, "FFI error"),
        }
    }
}

impl std::error::Error for ChessError {}

/// A chess game session: board state plus player rating.
///
/// Exposed to iOS/Android through UniFFI; see
/// `openspec/specs/shared/ffi-contracts.md` for the full surface.
/// The fields are mutex-protected because the FFI boundary shares the
/// session across calls via `Arc`.
#[derive(uniffi::Object)]
pub struct GameSession {
    board_manager: Mutex<domain::BoardManager>,
    rating_manager: Mutex<domain::RatingManager>,
}

fn lock<T>(mutex: &Mutex<T>) -> Result<std::sync::MutexGuard<'_, T>, ChessError> {
    mutex.lock().map_err(|_| ChessError::FfiError)
}

/// Create a new game session with the given starting rating.
#[uniffi::export]
pub fn new_game_session(initial_rating: f64) -> Arc<GameSession> {
    Arc::new(GameSession {
        board_manager: Mutex::new(domain::BoardManager::new()),
        rating_manager: Mutex::new(domain::RatingManager::new(initial_rating)),
    })
}

#[uniffi::export]
impl GameSession {
    /// Get the current board state as a FEN string.
    pub fn get_board_state(&self) -> Result<String, ChessError> {
        Ok(lock(&self.board_manager)?.get_fen())
    }

    /// Get the legal moves for a square (algebraic notation, e.g. "e2").
    pub fn get_valid_moves(&self, square: &str) -> Result<Vec<String>, ChessError> {
        Ok(lock(&self.board_manager)?.get_valid_moves(square))
    }

    /// Execute a UCI move (e.g. "e2e4").
    pub fn play_move(&self, uci_move: &str) -> Result<(), ChessError> {
        lock(&self.board_manager)?.play_move(uci_move).map_err(|_| ChessError::InvalidMove)
    }

    /// Whether the side to move is currently in check.
    pub fn is_check(&self) -> Result<bool, ChessError> {
        Ok(lock(&self.board_manager)?.is_check())
    }

    /// Whether the game is in checkmate.
    pub fn is_checkmate(&self) -> Result<bool, ChessError> {
        Ok(lock(&self.board_manager)?.is_checkmate())
    }

    /// Whether the game is in a draw.
    pub fn is_draw(&self) -> Result<bool, ChessError> {
        Ok(lock(&self.board_manager)?.is_draw())
    }

    /// Get the piece at a square ("K", "q", ... or "" when empty).
    pub fn get_piece_at(&self, square: &str) -> Result<String, ChessError> {
        Ok(lock(&self.board_manager)?.get_piece_at(square))
    }

    /// Update the player rating after a game.
    /// `result`: 1.0 (win), 0.5 (draw), 0.0 (loss).
    pub fn update_player_rating(&self, opponent_rating: f64, result: f64) -> Result<(), ChessError> {
        lock(&self.rating_manager)?
            .update_after_game(opponent_rating, result);
        Ok(())
    }

    /// Get the current player rating.
    pub fn get_current_rating(&self) -> Result<f64, ChessError> {
        Ok(lock(&self.rating_manager)?.get_rating())
    }
}

impl GameSession {
    /// Serialize the session state (internal; not part of the FFI surface).
    pub fn serialize(&self) -> String {
        let board = self.board_manager.lock().unwrap();
        let rating = self.rating_manager.lock().unwrap();
        format!("board:{},rating:{}", board.get_fen(), rating.serialize())
    }

    /// Restore a session from serialized data (internal; not part of the FFI surface).
    pub fn deserialize(&self, data: &str) {
        let parts: Vec<&str> = data.split(',').collect();
        if parts.len() == 2 {
            let board_part = parts[0];
            let rating_part = parts[1];
            if board_part.starts_with("board:") {
                let _fen = &board_part[6..];
                *self.board_manager.lock().unwrap() = domain::BoardManager::new();
            }
            self.rating_manager.lock().unwrap().deserialize(rating_part);
        }
    }
}

uniffi::setup_scaffolding!();
