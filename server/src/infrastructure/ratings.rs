//! In-memory per-device rating sessions (spec "Server Online Rating",
//! design D5 of add-online-multiplayer): the application's `Ratings` port
//! backed by the engine's Glicko-2 sessions. Reset when the server restarts
//! (documented limitation of the in-memory MVP).

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::application::ports::{ChessEngine, Ratings, RatingSession};
use crate::domain::rating::DEFAULT_RATING;

/// In-memory per-device rating sessions.
pub struct RatingStore {
    engine: Arc<dyn ChessEngine>,
    sessions: RwLock<HashMap<String, Box<dyn RatingSession>>>,
}

impl RatingStore {
    pub fn new(engine: Arc<dyn ChessEngine>) -> Self {
        Self {
            engine,
            sessions: RwLock::new(HashMap::new()),
        }
    }

    /// The current rating of a player; unknown devices lazily start at the
    /// default.
    pub fn rating_of(&self, player_id: &str) -> f64 {
        let mut sessions = self.sessions.write().unwrap();
        sessions
            .entry(player_id.to_string())
            .or_insert_with(|| self.engine.new_rating_session(DEFAULT_RATING))
            .current_rating()
            .expect("rating read")
    }

    /// Record a finished game: the winner scores `score` (1.0 for a decisive
    /// result, 0.5 for a draw) against the loser's rating, and the loser
    /// scores `1.0 - score` against the winner's rating. Both updates are
    /// evaluated against the opponent's rating as it stood before the game.
    pub fn apply_result(&self, winner: &str, loser: &str, score: f64) {
        let mut sessions = self.sessions.write().unwrap();
        let winner_rating = sessions
            .entry(winner.to_string())
            .or_insert_with(|| self.engine.new_rating_session(DEFAULT_RATING))
            .current_rating()
            .expect("rating read");
        let loser_rating = sessions
            .entry(loser.to_string())
            .or_insert_with(|| self.engine.new_rating_session(DEFAULT_RATING))
            .current_rating()
            .expect("rating read");
        sessions
            .get_mut(winner)
            .unwrap()
            .update_rating(loser_rating, score)
            .expect("winner rating update");
        sessions
            .get_mut(loser)
            .unwrap()
            .update_rating(winner_rating, 1.0 - score)
            .expect("loser rating update");
    }
}

impl Ratings for RatingStore {
    fn rating_of(&self, player_id: &str) -> f64 {
        RatingStore::rating_of(self, player_id)
    }

    fn apply_result(&self, winner: &str, loser: &str, score: f64) {
        RatingStore::apply_result(self, winner, loser, score)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::engine::CoreEngine;

    fn store() -> RatingStore {
        RatingStore::new(Arc::new(CoreEngine))
    }

    #[test]
    fn new_devices_start_at_default() {
        let store = store();
        assert_eq!(store.rating_of("device-a"), DEFAULT_RATING);
        assert_eq!(store.rating_of("device-b"), DEFAULT_RATING);
    }

    #[test]
    fn decisive_result_moves_winner_up_and_loser_down() {
        let store = store();
        store.apply_result("device-a", "device-b", 1.0);
        assert!(
            store.rating_of("device-a") > DEFAULT_RATING,
            "winner should gain rating"
        );
        assert!(
            store.rating_of("device-b") < DEFAULT_RATING,
            "loser should lose rating"
        );
    }

    #[test]
    fn draw_between_unequal_ratings_moves_both() {
        // Give the two devices different ratings first (a decisive game),
        // then split the next point: the lower-rated player gains a fraction
        // and the higher-rated one loses one (Glicko-2 against the
        // opponent's rating).
        let store = store();
        store.apply_result("device-a", "device-b", 1.0);
        let high = store.rating_of("device-a");
        let low = store.rating_of("device-b");
        assert!(high > low);

        store.apply_result("device-b", "device-a", 0.5);
        assert!(
            store.rating_of("device-b") > low,
            "lower-rated player gains on a half point"
        );
        assert!(
            store.rating_of("device-a") < high,
            "higher-rated player loses on a half point"
        );
    }

    #[test]
    fn ratings_accumulate_across_games() {
        let store = store();
        store.apply_result("device-a", "device-b", 1.0);
        let after_first = store.rating_of("device-a");
        store.apply_result("device-a", "device-b", 1.0);
        let after_second = store.rating_of("device-a");
        assert!(
            after_second > after_first,
            "the second win builds on the first (no reset between games)"
        );
    }

    #[test]
    fn losing_streak_decreases_rating() {
        let store = store();
        store.apply_result("device-a", "device-b", 1.0);
        store.apply_result("device-b", "device-a", 1.0);
        store.apply_result("device-b", "device-a", 1.0);
        assert!(
            store.rating_of("device-a") < DEFAULT_RATING,
            "a device that keeps losing drops below the default"
        );
    }
}
