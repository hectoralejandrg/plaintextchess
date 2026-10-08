use glicko2::{GameResult, Glicko2Player};

/// The full Glicko-2 state of a player: rating, rating deviation, and
/// volatility. In-process consumers (the online server) read this snapshot
/// to persist rating state and restore it into a fresh manager.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RatingSnapshot {
    pub rating: f64,
    pub rating_deviation: f64,
    pub volatility: f64,
}

pub struct RatingManager {
    rating: Glicko2Player,
}

impl RatingManager {
    pub fn new(rating: f64) -> Self {
        let rating = Glicko2Player {
            rating,
            rating_deviation: 200.0,
            volatility: 0.06,
        };
        Self { rating }
    }

    pub fn get_rating(&self) -> f64 {
        self.rating.rating
    }

    /// The player's full Glicko-2 state (rating, deviation, volatility).
    pub fn state(&self) -> RatingSnapshot {
        RatingSnapshot {
            rating: self.rating.rating,
            rating_deviation: self.rating.rating_deviation,
            volatility: self.rating.volatility,
        }
    }

    /// Restore the full Glicko-2 state from a snapshot read earlier.
    pub fn set_state(&mut self, snapshot: RatingSnapshot) {
        self.rating = Glicko2Player {
            rating: snapshot.rating,
            rating_deviation: snapshot.rating_deviation,
            volatility: snapshot.volatility,
        };
    }

    /// `result`: 1.0 (win), 0.5 (draw), 0.0 (loss) against `opponent_rating`.
    pub fn update_after_game(&mut self, opponent_rating: f64, result: f64) {
        let opponent = Glicko2Player {
            rating: opponent_rating,
            rating_deviation: 200.0,
            volatility: 0.06,
        };
        let game_result = if result > 0.75 {
            GameResult::win(opponent)
        } else if result < 0.25 {
            GameResult::loss(opponent)
        } else {
            GameResult::draw(opponent)
        };
        self.rating = glicko2::new_rating(self.rating, &[game_result], 0.5);
    }

    /// Serialize as `rating:<r>,deviation:<d>,volatility:<v>`.
    pub fn serialize(&self) -> String {
        format!(
            "rating:{},deviation:{},volatility:{}",
            self.rating.rating, self.rating.rating_deviation, self.rating.volatility
        )
    }

    /// Restore from the format produced by `serialize`.
    pub fn deserialize(&mut self, data: &str) {
        let fields: Vec<&str> = data.split(':').collect();
        if fields.len() == 4 && fields[0] == "rating" {
            if let (Ok(r), Ok(d), Ok(v)) = (
                fields[1].parse::<f64>(),
                fields[2].parse::<f64>(),
                fields[3].parse::<f64>(),
            ) {
                self.rating = Glicko2Player {
                    rating: r,
                    rating_deviation: d,
                    volatility: v,
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_snapshot_restores_an_updated_state_to_a_fresh_manager() {
        let mut updated = RatingManager::new(1500.0);
        // A couple of games move the rating and tighten the deviation, so
        // every field of the snapshot differs from the defaults.
        updated.update_after_game(1500.0, 1.0);
        updated.update_after_game(1550.0, 0.5);
        let snapshot = updated.state();
        assert!(snapshot.rating_deviation < 200.0, "RD must tighten after games");

        let mut fresh = RatingManager::new(1500.0);
        assert_eq!(
            fresh.state(),
            RatingSnapshot {
                rating: 1500.0,
                rating_deviation: 200.0,
                volatility: 0.06
            },
            "the fresh manager starts at the defaults"
        );
        fresh.set_state(snapshot);
        assert_eq!(
            fresh.state(),
            snapshot,
            "the restored manager matches the snapshot exactly"
        );

        // Subsequent updates continue from the restored state.
        let continued = fresh.state();
        fresh.update_after_game(1400.0, 1.0);
        assert_ne!(
            fresh.state().rating,
            continued.rating,
            "rating updates continue from the restored state"
        );
    }
}
