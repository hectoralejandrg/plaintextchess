//! Per-device Glicko-2 rating sessions (spec "Server Online Rating",
//! design D5 of add-online-multiplayer): the application's `Ratings` port
//! backed by the engine's Glicko-2 sessions. Ratings persist in SQLite
//! (loaded at server startup, committed at game end, design D1/D4).

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::application::ports::{ChessEngine, Ratings, RatingSession, RatingState};
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

    /// The device's full Glicko-2 state, if a session exists for it
    /// (unknown devices are not created here, unlike `rating_of`).
    pub fn rating_state(&self, player_id: &str) -> Option<RatingState> {
        let sessions = self.sessions.read().unwrap();
        sessions.get(player_id).map(|s| s.state().expect("state read"))
    }

    /// Seed a device's rating session from a persisted state (startup
    /// load, spec "Server Persistence"). Replaces any in-memory session.
    pub fn seed(&self, device_id: &str, state: RatingState) {
        let mut sessions = self.sessions.write().unwrap();
        sessions.insert(
            device_id.to_string(),
            self.engine.new_rating_session_state(&state),
        );
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

    fn rating_state(&self, player_id: &str) -> Option<RatingState> {
        RatingStore::rating_state(self, player_id)
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

    #[test]
    fn seeding_a_non_default_state_is_read_back() {
        let store = store();
        let seeded = RatingState {
            rating: 1800.0,
            rating_deviation: 150.0,
            volatility: 0.03,
        };
        store.seed("device-seed", seeded);
        assert_eq!(store.rating_of("device-seed"), 1800.0);
        assert_eq!(store.rating_state("device-seed"), Some(seeded));
        // Unknown devices have no session: no state, but a default rating.
        assert_eq!(store.rating_state("device-unknown"), None);
        assert_eq!(store.rating_of("device-unknown"), DEFAULT_RATING);
    }

    #[test]
    fn apply_result_continues_from_the_seeded_state() {
        let store = store();
        store.seed(
            "device-seed",
            RatingState {
                rating: 2000.0,
                rating_deviation: 200.0,
                volatility: 0.06,
            },
        );
        // A win against an unrival (1500) must start from 2000, not 1500.
        store.apply_result("device-seed", "device-rival", 1.0);
        let state = store.rating_state("device-seed").expect("seeded state");
        assert!(
            state.rating > 2000.0,
            "the winner gains from its seeded 2000, got {state:?}"
        );
        assert!(state.rating_deviation < 200.0, "RD tightens after a game");
        // The rival (never seen) lost from the default 1500.
        assert!(store.rating_of("device-rival") < DEFAULT_RATING);
    }
}
