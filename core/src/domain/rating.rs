use glicko2::{GameResult, Glicko2Player};

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
