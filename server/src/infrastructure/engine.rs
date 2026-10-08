//! The concrete chess engine (design D1): wraps the `chess-core` sessions
//! behind the application's `ChessEngine` port, mapping the core's FFI
//! errors to `EngineError`.

use std::sync::Arc;

use chess_core::domain::RatingSnapshot;
use chess_core::{new_game_session, new_game_session_from_rating_state, GameSession};

use crate::application::ports::{ChessEngine, EngineError, EngineSession, RatingSession, RatingState};

/// The production engine: the in-process `chess-core` crate.
pub struct CoreEngine;

impl ChessEngine for CoreEngine {
    fn new_game_session(&self) -> Box<dyn EngineSession> {
        Box::new(CoreEngineSession(new_game_session(0.0)))
    }

    fn new_rating_session(&self, rating: f64) -> Box<dyn RatingSession> {
        Box::new(CoreRatingSession(new_game_session(rating)))
    }

    fn new_rating_session_state(&self, state: &RatingState) -> Box<dyn RatingSession> {
        Box::new(CoreRatingSession(new_game_session_from_rating_state(RatingSnapshot {
            rating: state.rating,
            rating_deviation: state.rating_deviation,
            volatility: state.volatility,
        })))
    }
}

struct CoreEngineSession(Arc<GameSession>);

impl EngineSession for CoreEngineSession {
    fn play_move(&self, uci: &str) -> Result<(), EngineError> {
        self.0.play_move(uci).map_err(|e| e.to_string())
    }

    fn board_state(&self) -> Result<String, EngineError> {
        self.0.get_board_state().map_err(|e| e.to_string())
    }

    fn is_checkmate(&self) -> Result<bool, EngineError> {
        self.0.is_checkmate().map_err(|e| e.to_string())
    }

    fn is_draw(&self) -> Result<bool, EngineError> {
        self.0.is_draw().map_err(|e| e.to_string())
    }

    fn insufficient_material_for(&self, winner_is_white: bool) -> Result<bool, EngineError> {
        self.0
            .insufficient_material_for(winner_is_white)
            .map_err(|e| e.to_string())
    }
}

struct CoreRatingSession(Arc<GameSession>);

impl RatingSession for CoreRatingSession {
    fn current_rating(&self) -> Result<f64, EngineError> {
        self.0
            .get_current_rating()
            .map_err(|e| e.to_string())
    }

    fn update_rating(&self, opponent_rating: f64, score: f64) -> Result<(), EngineError> {
        self.0
            .update_player_rating(opponent_rating, score)
            .map_err(|e| e.to_string())
    }

    fn state(&self) -> Result<RatingState, EngineError> {
        let snapshot = self.0.get_rating_state();
        Ok(RatingState {
            rating: snapshot.rating,
            rating_deviation: snapshot.rating_deviation,
            volatility: snapshot.volatility,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_engine_applies_legal_moves_and_rejects_illegal_ones() {
        let engine = CoreEngine;
        let session = engine.new_game_session();
        let initial = session.board_state().expect("board state");
        session.play_move("e2e4").expect("legal move");
        assert_ne!(session.board_state().expect("board state"), initial);
        assert!(session.play_move("e2e4").is_err(), "a second e2e4 is illegal");
        assert!(!session.is_checkmate().expect("checkmate check"));
        assert!(!session.is_draw().expect("draw check"));
    }

    #[test]
    fn rating_sessions_start_at_the_seeded_rating_and_update() {
        let engine = CoreEngine;
        let session = engine.new_rating_session(1600.0);
        assert_eq!(session.current_rating().expect("rating read"), 1600.0);
        session
            .update_rating(1500.0, 1.0)
            .expect("rating update");
        assert!(
            session.current_rating().expect("rating read") > 1600.0,
            "a win against a weaker opponent gains rating"
        );
    }
}
