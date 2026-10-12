//! Arena tournament model: an open time window where waiting players are
//! continuously paired by standing, with a points leaderboard separate from
//! the Glicko-2 rating (Lichess-style: win=2, draw=1, loss=0, doubled to
//! stay integer).

use crate::domain::time_control::TimeControl;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TournamentStatus {
    Scheduled,
    Running,
    Finished,
}

#[derive(Debug, Clone)]
pub struct Tournament {
    pub id: String,
    pub time_control: TimeControl,
    pub starts_at_ms: i64,
    pub duration_ms: i64,
    pub status: TournamentStatus,
}

impl Tournament {
    pub fn is_over(&self, now_ms: i64) -> bool {
        now_ms >= self.starts_at_ms + self.duration_ms
    }
}

/// One player's position in a tournament (points doubled: win=2, draw=1,
/// loss=0, so no float drift when summed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Standing {
    pub player_id: String,
    pub points: u32,
    pub games_played: u32,
}

/// The doubled point value of a result: 2 for a decisive win, 1 for a draw.
pub fn points_for(won: bool, drawn: bool) -> (u32, u32) {
    if drawn {
        (1, 1)
    } else if won {
        (2, 0)
    } else {
        (0, 2)
    }
}

/// Pair waiting players by standing (Arena-style, not a Swiss solver):
/// sort by points descending (ties broken by `player_id` for determinism),
/// pair adjacent players, and swap with the next pair to avoid an immediate
/// rematch when doing so doesn't need to reach past the next pair.
///
/// `# ponytail: O(n) single-pass rematch check, not a full assignment
/// solver — revisit if waiting groups exceed ~50 concurrent players.`
pub fn pair_waiting_players(
    standings: &[Standing],
    waiting: &[String],
    last_opponent: impl Fn(&str) -> Option<String>,
) -> Vec<(String, String)> {
    let points_of = |player_id: &str| -> u32 {
        standings
            .iter()
            .find(|s| s.player_id == player_id)
            .map(|s| s.points)
            .unwrap_or(0)
    };

    let mut sorted: Vec<String> = waiting.to_vec();
    sorted.sort_by(|a, b| points_of(b).cmp(&points_of(a)).then_with(|| a.cmp(b)));

    let mut pairs = Vec::with_capacity(sorted.len() / 2);
    let mut i = 0;
    while i + 1 < sorted.len() {
        let (a, b) = (sorted[i].clone(), sorted[i + 1].clone());
        let is_rematch = last_opponent(&a).as_deref() == Some(b.as_str());
        if is_rematch && i + 2 < sorted.len() {
            // Swap b with the next player to avoid the immediate rematch,
            // provided that next player isn't a's own rematch too.
            let c = sorted[i + 2].clone();
            if last_opponent(&a).as_deref() != Some(c.as_str()) {
                sorted.swap(i + 1, i + 2);
                pairs.push((a, sorted[i + 1].clone()));
                i += 2;
                continue;
            }
        }
        pairs.push((a, b));
        i += 2;
    }
    pairs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn standing(player_id: &str, points: u32) -> Standing {
        Standing {
            player_id: player_id.to_string(),
            games_played: 0,
            points,
        }
    }

    #[test]
    fn pairs_adjacent_by_points_descending() {
        let standings = vec![standing("a", 2), standing("b", 4), standing("c", 0), standing("d", 2)];
        let waiting = vec!["a".to_string(), "b".to_string(), "c".to_string(), "d".to_string()];
        let pairs = pair_waiting_players(&standings, &waiting, |_| None);
        // Sorted by points desc, tie broken by id: b(4), a(2), d(2), c(0).
        assert_eq!(pairs, vec![("b".to_string(), "a".to_string()), ("d".to_string(), "c".to_string())]);
    }

    #[test]
    fn ties_break_deterministically_by_player_id() {
        let standings = vec![standing("z", 0), standing("a", 0)];
        let waiting = vec!["z".to_string(), "a".to_string()];
        let pairs = pair_waiting_players(&standings, &waiting, |_| None);
        assert_eq!(pairs, vec![("a".to_string(), "z".to_string())]);
    }

    #[test]
    fn avoids_an_immediate_rematch_when_a_swap_is_available() {
        let standings = vec![standing("a", 2), standing("b", 2), standing("c", 2), standing("d", 2)];
        let waiting = vec!["a".to_string(), "b".to_string(), "c".to_string(), "d".to_string()];
        // a and b just played; sorted order is a,b,c,d (tie-break by id).
        let pairs = pair_waiting_players(&standings, &waiting, |p| {
            if p == "a" { Some("b".to_string()) } else { None }
        });
        assert_eq!(pairs, vec![("a".to_string(), "c".to_string()), ("b".to_string(), "d".to_string())]);
    }

    #[test]
    fn an_odd_player_out_sits_out_the_round() {
        let standings = vec![standing("a", 0), standing("b", 0), standing("c", 0)];
        let waiting = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let pairs = pair_waiting_players(&standings, &waiting, |_| None);
        assert_eq!(pairs.len(), 1);
    }
}
