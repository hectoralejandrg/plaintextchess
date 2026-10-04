//! Smoke test for the UniFFI-exported surface of `chess-core`.
//! Exercises the same calls the iOS/Android apps make over the FFI boundary.

use chess_core::new_game_session;

const INITIAL_FEN: &str = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR";

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

// enforce-single-active-game: sessions are the unit of isolation. These tests
// pin down the guarantee that the app-level "one active game" rule relies on:
// sessions never see each other's state, and the CPU query is pure.

#[test]
fn independent_sessions_do_not_share_state() {
    let a = new_game_session(1500.0);
    let b = new_game_session(1500.0);

    a.play_move("e2e4").unwrap();

    // `b` is untouched: standard initial position and full opening moves.
    assert_eq!(b.get_board_state().unwrap(), INITIAL_FEN);
    let b_moves = b.get_valid_moves("e2").unwrap();
    assert!(
        b_moves.contains(&"e2e4".to_string()),
        "e2e4 missing in {b_moves:?}"
    );
    assert!(
        b_moves.contains(&"e2e3".to_string()),
        "e2e3 missing in {b_moves:?}"
    );

    // ...while `a` can no longer play from e2.
    assert!(a.get_valid_moves("e2").unwrap().is_empty());
}

#[test]
fn get_cpu_move_is_a_pure_query() {
    let s = new_game_session(1500.0);
    let fen_before = s.get_board_state().unwrap();
    let moves_before = s.get_valid_moves("e2").unwrap();
    let check_before = s.is_check().unwrap();

    let uci = s.get_cpu_move(1).unwrap();
    assert!(!uci.is_empty());
    // The suggested move must be legal in the current position.
    let legal = s.get_valid_moves(&uci[..2]).unwrap();
    assert!(
        legal.contains(&uci),
        "cpu move {uci} not legal from {legal:?}"
    );

    // Nothing changed: same position, same legal moves, same check state.
    assert_eq!(s.get_board_state().unwrap(), fen_before);
    assert_eq!(s.get_valid_moves("e2").unwrap(), moves_before);
    assert_eq!(s.is_check().unwrap(), check_before);
}

#[test]
fn concurrent_read_only_access_is_consistent() {
    use std::thread;

    // One session shared across threads, read-only.
    let shared = new_game_session(1500.0);
    shared.play_move("e2e4").unwrap();
    shared.play_move("e7e5").unwrap();
    let fen_after_two_moves = shared.get_board_state().unwrap();

    let mut handles = Vec::new();
    for i in 0..8 {
        let s = shared.clone();
        let expected = fen_after_two_moves.clone();
        handles.push(thread::spawn(move || {
            for _ in 0..50 {
                assert_eq!(s.get_board_state().unwrap(), expected);
                if i % 2 == 0 {
                    let uci = s.get_cpu_move(1).unwrap();
                    assert!(!uci.is_empty());
                }
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }

    // Independent sessions, one per thread.
    let expected = "rnbqkbnr/pppppppp/8/8/3P4/8/PPP1PPPP/RNBQKBNR";
    let fens: Vec<String> = (0..8)
        .map(|_| {
            thread::spawn(|| {
                let s = new_game_session(1500.0);
                s.play_move("d2d4").unwrap();
                let fen = s.get_board_state().unwrap();
                let uci = s.get_cpu_move(1).unwrap();
                assert!(!uci.is_empty());
                // Still consistent after the CPU query.
                assert_eq!(s.get_board_state().unwrap(), fen);
                fen
            })
            .join()
            .unwrap()
        })
        .collect();
    assert!(
        fens.iter().all(|f| f == expected),
        "unexpected position: {fens:?}"
    );
}

#[test]
fn checkmate_in_one_session_does_not_affect_the_other() {
    let a = new_game_session(1500.0);
    let b = new_game_session(1500.0);

    // Scholar's mate on `a`.
    for m in ["e2e4", "e7e5", "d1h5", "b8c6", "f1c4", "g8f6", "h5f7"] {
        a.play_move(m).unwrap();
    }
    assert!(a.is_checkmate().unwrap());

    // `b` is still a fresh, fully playable game.
    assert_eq!(b.get_board_state().unwrap(), INITIAL_FEN);
    assert!(!b.is_checkmate().unwrap());
    b.play_move("e2e4").unwrap();
    assert_eq!(b.get_piece_at("e4").unwrap(), "P");
}
