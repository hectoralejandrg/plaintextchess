//! Flag-fall material check (spec "A flag fall with insufficient material
//! is a draw", design D4): when a player's time runs out the opponent wins,
//! unless the opponent's material cannot deliver checkmate, in which case
//! the game is a draw.

/// Whether the side `winner_is_white` in `fen` is unable to deliver
/// checkmate with its material, using the standard flag-fall rule
/// (deliberately simplified, design D4):
///
/// - any pawn, rook, or queen on the winner's side is always sufficient;
/// - a lone king, king + one minor, or exactly two bishops on the same
///   square color are insufficient;
/// - king + two knights (or a knight and a bishop) is theoretically
///   sufficient in flag-fall judgment, so it stays a win.
pub fn insufficient_to_mate(fen: &str, winner_is_white: bool) -> bool {
    let board = match fen.split(' ').next() {
        // A board must have exactly 8 ranks: anything else is not a FEN we
        // can trust, so the position is treated as sufficient (a win).
        Some(board) if board.split('/').count() == 8 => board,
        _ => return false,
    };

    let mut pawns: u8 = 0;
    let mut knights: u8 = 0;
    let mut rooks: u8 = 0;
    let mut queens: u8 = 0;
    let mut bishops_on_dark: u8 = 0;
    let mut bishops_on_light: u8 = 0;

    for (rank_index, rank) in board.split('/').enumerate() {
        let mut file = 0usize;
        for ch in rank.chars() {
            match ch {
                '1'..='9' => file += ch.to_digit(10).unwrap_or(0) as usize,
                _ => {
                    if ch.is_ascii_uppercase() == winner_is_white {
                        // Square color is stable under `file + rank`: it only
                        // needs to distinguish the two bishop complexes.
                        let on_dark_square = (file + rank_index).is_multiple_of(2);
                        match ch.to_ascii_uppercase() {
                            'P' => pawns += 1,
                            'N' => knights += 1,
                            'R' => rooks += 1,
                            'Q' => queens += 1,
                            'B' => {
                                if on_dark_square {
                                    bishops_on_dark += 1;
                                } else {
                                    bishops_on_light += 1;
                                }
                            }
                            _ => {}
                        }
                    }
                    file += 1;
                }
            }
        }
    }

    let minors = knights + bishops_on_dark + bishops_on_light;
    let only_same_color_bishops = knights == 0
        && ((bishops_on_dark == 2 && bishops_on_light == 0)
            || (bishops_on_light == 2 && bishops_on_dark == 0));

    pawns == 0 && rooks == 0 && queens == 0 && (minors <= 1 || only_same_color_bishops)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lone_king_cannot_checkmate() {
        assert!(insufficient_to_mate("4k3/8/8/8/8/8/8/4K3 w - - 0 1", true));
    }

    #[test]
    fn a_pawn_a_rook_or_a_queen_is_always_sufficient() {
        assert!(!insufficient_to_mate("4k3/8/8/8/8/8/P7/4K3 w - - 0 1", true));
        assert!(!insufficient_to_mate("4k3/8/8/8/8/8/8/R3K3 w - - 0 1", true));
        assert!(!insufficient_to_mate("4k3/8/8/8/8/8/8/Q3K3 w - - 0 1", true));
    }

    #[test]
    fn a_single_minor_is_insufficient() {
        assert!(insufficient_to_mate("4k3/8/8/8/8/8/8/3NK3 w - - 0 1", true));
        assert!(insufficient_to_mate("4k3/8/8/8/8/8/8/3BK3 w - - 0 1", true));
    }

    #[test]
    fn two_knights_or_a_knight_and_a_bishop_are_treated_as_sufficient() {
        // Standard flag-fall judgment: mate is possible (with the opponent's
        // help for NN), so these are wins rather than draws.
        assert!(!insufficient_to_mate("4k3/8/8/8/8/8/8/2NNK3 w - - 0 1", true));
        assert!(!insufficient_to_mate("4k3/8/8/8/8/8/8/2NBK3 w - - 0 1", true));
    }

    #[test]
    fn two_bishops_on_the_same_color_are_insufficient() {
        // c2 and d1 are both dark squares on the a1-is-dark convention.
        assert!(insufficient_to_mate("4k3/8/8/8/8/8/2B5/3B1K2 w - - 0 1", true));
    }

    #[test]
    fn two_bishops_on_opposite_colors_are_sufficient() {
        // b2 is light, d1 is dark: opposite complexes can mate.
        assert!(!insufficient_to_mate("4k3/8/8/8/8/8/1B6/3B1K2 w - - 0 1", true));
    }

    #[test]
    fn the_check_is_about_the_winners_material_only() {
        // Black is the winner and holds only a knight: insufficient, even
        // though White (the flagged side) still has a full army.
        assert!(insufficient_to_mate(
            "3nk3/8/8/8/8/8/PPPPPPPP/RNBQKBNR w - - 0 1",
            false
        ));
        // White is the winner with only a knight: also insufficient, even
        // though Black (the flagged side) has everything.
        assert!(insufficient_to_mate(
            "3NK3/8/8/8/8/8/pppppppp/rnbqkbnr w - - 0 1",
            true
        ));
        // A queen still wins for the side that holds it.
        assert!(!insufficient_to_mate(
            "3QK3/8/8/8/8/8/pppppppp/rnbqkbnr w - - 0 1",
            true
        ));
    }

    #[test]
    fn a_malformed_fen_is_treated_as_sufficient() {
        assert!(!insufficient_to_mate("", true));
        assert!(!insufficient_to_mate("not a fen", true));
    }
}
