//! Flag-fall material judgment (spec "A flag fall with insufficient material
//! is a draw"): whether a side can still deliver checkmate with the material
//! it holds. This is the single, shared implementation used by the online
//! server (through the FFI `GameSession`), so the rule lives next to the
//! board it inspects instead of being re-derived from a FEN elsewhere.

use shakmaty::{Board, Color, Piece, Role, Square};

/// Whether the side `winner_is_white` on `board` is unable to deliver
/// checkmate with its material, using the standard flag-fall rule
/// (deliberately simplified):
///
/// - any pawn, rook, or queen on that side is always sufficient;
/// - a lone king, king + one minor, or exactly two bishops on the same
///   square color are insufficient;
/// - king + two knights (or a knight and a bishop) is theoretically
///   sufficient in flag-fall judgment, so it stays a win.
pub fn insufficient_to_mate(board: &Board, winner_is_white: bool) -> bool {
    let winner = if winner_is_white {
        Color::White
    } else {
        Color::Black
    };
    let (mut pawns, mut knights, mut rooks, mut queens) = (0u32, 0u32, 0u32, 0u32);
    let (mut bishops_dark, mut bishops_light) = (0u32, 0u32);

    for index in 0..64i8 {
        let square = Square::new(index);
        let Some(Piece { color, role }) = board.piece_at(square) else {
            continue;
        };
        if color != winner {
            continue;
        }
        match role {
            Role::Pawn => pawns += 1,
            Role::Knight => knights += 1,
            Role::Rook => rooks += 1,
            Role::Queen => queens += 1,
            Role::Bishop => {
                // The two bishop complexes: parity of file + rank. A global
                // flip of the convention does not change "same vs opposite".
                if (square.file() + square.rank()) % 2 == 0 {
                    bishops_dark += 1;
                } else {
                    bishops_light += 1;
                }
            }
            Role::King => {}
        }
    }

    let minors = knights + bishops_dark + bishops_light;
    let only_same_color_bishops = knights == 0
        && ((bishops_dark == 2 && bishops_light == 0)
            || (bishops_light == 2 && bishops_dark == 0));

    pawns == 0 && rooks == 0 && queens == 0 && (minors <= 1 || only_same_color_bishops)
}

#[cfg(test)]
mod tests {
    use super::*;
    use shakmaty::Setup;

    fn board(fen: &str) -> Board {
        let parsed: shakmaty::fen::Fen = fen.parse().expect("valid FEN");
        let chess: shakmaty::Chess = parsed.position().expect("valid position");
        chess.board().clone()
    }

    #[test]
    fn a_lone_king_cannot_checkmate() {
        assert!(insufficient_to_mate(
            &board("4k3/8/8/8/8/8/8/4K3 w - - 0 1"),
            true
        ));
    }

    #[test]
    fn a_pawn_a_rook_or_a_queen_is_always_sufficient() {
        assert!(!insufficient_to_mate(
            &board("4k3/8/8/8/8/8/P7/4K3 w - - 0 1"),
            true
        ));
        assert!(!insufficient_to_mate(
            &board("4k3/8/8/8/8/8/8/R3K3 w - - 0 1"),
            true
        ));
        assert!(!insufficient_to_mate(
            &board("4k3/8/8/8/8/8/8/Q3K3 w - - 0 1"),
            true
        ));
    }

    #[test]
    fn a_single_minor_is_insufficient() {
        assert!(insufficient_to_mate(
            &board("4k3/8/8/8/8/8/8/3NK3 w - - 0 1"),
            true
        ));
        assert!(insufficient_to_mate(
            &board("4k3/8/8/8/8/8/8/3BK3 w - - 0 1"),
            true
        ));
    }

    #[test]
    fn two_knights_or_a_knight_and_a_bishop_are_treated_as_sufficient() {
        // Standard flag-fall judgment: mate is possible (with the opponent's
        // help for NN), so these are wins rather than draws.
        assert!(!insufficient_to_mate(
            &board("4k3/8/8/8/8/8/8/2NNK3 w - - 0 1"),
            true
        ));
        assert!(!insufficient_to_mate(
            &board("4k3/8/8/8/8/8/8/2NBK3 w - - 0 1"),
            true
        ));
    }

    #[test]
    fn two_bishops_on_the_same_color_are_insufficient() {
        // c2 and d1 are both dark squares on the a1-is-dark convention.
        assert!(insufficient_to_mate(
            &board("4k3/8/8/8/8/8/2B5/3B1K2 w - - 0 1"),
            true
        ));
    }

    #[test]
    fn two_bishops_on_opposite_colors_are_sufficient() {
        // b2 is light, d1 is dark: opposite complexes can mate.
        assert!(!insufficient_to_mate(
            &board("4k3/8/8/8/8/8/1B6/3B1K2 w - - 0 1"),
            true
        ));
    }

    #[test]
    fn the_check_is_about_the_winners_material_only() {
        // Black is the winner and holds only a knight: insufficient, even
        // though White (the flagged side) still has a full army.
        assert!(insufficient_to_mate(
            &board("3nk3/8/8/8/8/8/PPPPPPPP/RNBQKBNR w - - 0 1"),
            false
        ));
        // White is the winner with only a knight: also insufficient, even
        // though Black (the flagged side) has everything.
        assert!(insufficient_to_mate(
            &board("3NK3/8/8/8/8/8/pppppppp/rnbqkbnr w - - 0 1"),
            true
        ));
        // A queen still wins for the side that holds it.
        assert!(!insufficient_to_mate(
            &board("3QK3/8/8/8/8/8/pppppppp/rnbqkbnr w - - 0 1"),
            true
        ));
    }
}
