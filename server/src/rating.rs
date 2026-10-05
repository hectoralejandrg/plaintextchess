//! Online ratings keyed by device identifier (spec: "Server Online Rating",
//! design D5). The core's Glicko2 logic does the math; this store only maps
//! player ids to rating sessions and records results.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use chess_core::{new_game_session, GameSession};

/// Default rating for devices the server has never seen (spec scenario
/// "A new player starts at 1500").
pub const DEFAULT_RATING: f64 = 1500.0;

/// In-memory per-device rating sessions. Reset when the server restarts
/// (documented limitation of the in-memory MVP).
pub struct RatingStore {
    sessions: RwLock<HashMap<String, Arc<GameSession>>>,
}

impl RatingStore {
    pub fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
        }
    }

    fn session(&self, player_id: &str) -> Arc<GameSession> {
        let mut sessions = self.sessions.write().unwrap();
        sessions
            .entry(player_id.to_string())
            .or_insert_with(|| new_game_session(DEFAULT_RATING))
            .clone()
    }

    /// The current rating of a player; unknown devices lazily start at the
    /// default.
    pub fn rating_of(&self, player_id: &str) -> f64 {
        self.session(player_id).get_current_rating().expect("rating read")
    }

    /// Record a finished game: the winner scores `score` (1.0 for a decisive
    /// result, 0.5 for a draw) against the loser's rating, and the loser
    /// scores `1.0 - score` against the winner's rating. Both updates are
    /// evaluated against the opponent's rating as it stood before the game.
    pub fn apply_result(&self, winner: &str, loser: &str, score: f64) {
        let mut sessions = self.sessions.write().unwrap();
        let winner_session = sessions
            .entry(winner.to_string())
            .or_insert_with(|| new_game_session(DEFAULT_RATING))
            .clone();
        let loser_session = sessions
            .entry(loser.to_string())
            .or_insert_with(|| new_game_session(DEFAULT_RATING))
            .clone();
        let winner_rating = winner_session.get_current_rating().expect("rating read");
        let loser_rating = loser_session.get_current_rating().expect("rating read");
        winner_session
            .update_player_rating(loser_rating, score)
            .expect("winner rating update");
        loser_session
            .update_player_rating(winner_rating, 1.0 - score)
            .expect("loser rating update");
    }
}

impl Default for RatingStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_devices_start_at_default() {
        let store = RatingStore::new();
        assert_eq!(store.rating_of("device-a"), DEFAULT_RATING);
        assert_eq!(store.rating_of("device-b"), DEFAULT_RATING);
    }

    #[test]
    fn decisive_result_moves_winner_up_and_loser_down() {
        let store = RatingStore::new();
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
        let store = RatingStore::new();
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
        let store = RatingStore::new();
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
        let store = RatingStore::new();
        store.apply_result("device-a", "device-b", 1.0);
        store.apply_result("device-b", "device-a", 1.0);
        store.apply_result("device-b", "device-a", 1.0);
        assert!(
            store.rating_of("device-a") < DEFAULT_RATING,
            "a device that keeps losing drops below the default"
        );
    }
}
