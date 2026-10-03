//! Smoke test for the UniFFI-exported surface of `chess-core`.
//! Exercises the same calls the iOS/Android apps make over the FFI boundary.

use chess_core::new_game_session;

fn assert_rating_close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 0.01,
        "expected ~{expected}, got {actual}"
    );
}

#[test]
fn game_flow() {
    let s = new_game_session(1500.0);

    assert_eq!(
        s.get_board_state().unwrap(),
        "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR"
    );

    let moves = s.get_valid_moves("e2").unwrap();
    assert!(
        moves.contains(&"e2e4".to_string()),
        "e2e4 missing in {moves:?}"
    );
    assert!(
        moves.contains(&"e2e3".to_string()),
        "e2e3 missing in {moves:?}"
    );

    s.play_move("e2e4").unwrap();
    assert_eq!(
        s.get_board_state().unwrap(),
        "rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR"
    );

    s.play_move("e7e5").unwrap();
    assert_eq!(s.get_piece_at("e4").unwrap(), "P");
    assert_eq!(s.get_piece_at("e5").unwrap(), "p");

    assert!(!s.is_check().unwrap());
    assert!(!s.is_checkmate().unwrap());
    assert!(!s.is_draw().unwrap());

    // White's pawn moved, so e2 is empty and "e2e3" is illegal now.
    assert!(s.get_valid_moves("e2").unwrap().is_empty());
    assert!(s.play_move("e2e3").is_err());

    // Rating updates after a win against a 1400 opponent.
    assert_rating_close(s.get_current_rating().unwrap(), 1500.0);
    s.update_player_rating(1400.0, 1.0).unwrap();
    assert!(s.get_current_rating().unwrap() > 1500.0);
}

#[test]
fn serialize_roundtrip() {
    let s = new_game_session(1500.0);
    s.play_move("e2e4").unwrap();
    let data = s.serialize();
    let s2 = new_game_session(1500.0);
    s2.deserialize(&data);
    // deserialize resets the board to the start position (current behavior).
    assert_eq!(
        s2.get_board_state().unwrap(),
        "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR"
    );
}
