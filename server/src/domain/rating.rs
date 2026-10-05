//! Rating policy (spec "Server Online Rating"): every device starts at the
//! default rating, and a terminal winner's point share depends on the result.

/// Default rating for devices the server has never seen (spec scenario
/// "A new player starts at 1500").
pub const DEFAULT_RATING: f64 = 1500.0;

/// The point share a terminal winner scores against the loser: 1.0 for a
/// decisive result (checkmate, resignation, forfeit, flag fall), 0.5 for a
/// draw (including a flag-fall draw).
pub fn winner_score(drawn: bool) -> f64 {
    if drawn {
        0.5
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_rating_is_1500() {
        assert_eq!(DEFAULT_RATING, 1500.0);
    }

    #[test]
    fn decisive_results_score_one_point_and_draws_half() {
        assert_eq!(winner_score(false), 1.0);
        assert_eq!(winner_score(true), 0.5);
    }
}
