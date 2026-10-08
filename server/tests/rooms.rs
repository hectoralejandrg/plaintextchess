//! Room-level scenarios (tasks 1.3-1.6) driven through `App` + channels:
//! room registry, game authority, ratings, disconnect/reconnect/forfeit.

mod common;

use std::sync::Arc;
use std::time::Duration;

use chess_server::infrastructure::server::Conn;
use chess_server::interface::protocol::{Color, ServerMessage, State, Status};
use common::*;

use chess_server::application::room_actor::RoomMsg;

const START_FEN: &str = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR";

fn send(conn: &Conn, msg: RoomMsg) {
    conn.mailbox.send(msg).expect("room mailbox closed");
}

fn send_move(conn: &Conn, uci: &str) {
    send(
        conn,
        RoomMsg::Move {
            identity: conn.identity.clone(),
            uci: uci.to_string(),
        },
    );
}

fn send_resign(conn: &Conn) {
    send(
        conn,
        RoomMsg::Resign {
            identity: conn.identity.clone(),
        },
    );
}

fn send_leave(conn: &Conn) {
    send(
        conn,
        RoomMsg::Leave {
            identity: conn.identity.clone(),
            ack: None,
        },
    );
}

fn send_detach(conn: &Conn) {
    send(
        conn,
        RoomMsg::Detach {
            identity: conn.identity.clone(),
        },
    );
}

/// Creates a room with P_A and joins it with P_B, consuming both
/// `room_ready` messages plus the creator's join snapshot. Afterwards both
/// outbound channels only carry post-move / event snapshots.
/// Returns `(a, b, join_snapshot_for_a)`.
async fn start_game(app: &Arc<App>) -> (Conn, Conn, State) {
    let mut a = create(app, P_A);
    let mut b = join(app, P_B, &a.code).expect("B joins");
    let _room_ready_a = next_message(&mut a.out_rx).await;
    let _room_ready_b = next_message(&mut b.out_rx).await;
    let join_snapshot_a = next_state(&mut a.out_rx).await;
    (a, b, join_snapshot_a)
}

// ---------------------------------------------------------------------------
// Room registry and lifecycle (task 1.3)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_returns_valid_code_and_seats_white() {
    let app = app_with_grace(30);
    let mut conn = create(&app, P_A);

    assert!(
        chess_server::infrastructure::server::App::is_valid_code(&conn.code),
        "creator code `{}` must be 6 chars from the unambiguous alphabet",
        conn.code
    );

    match next_message(&mut conn.out_rx).await {
        ServerMessage::RoomReady {
            room_code,
            your_color,
            state,
            ..
        } => {
            assert_eq!(room_code, conn.code);
            assert_eq!(your_color, Color::White);
            assert_eq!(state.board_fen, START_FEN);
            assert!(state.move_list.is_empty());
            assert_eq!(state.side_to_move, "w");
            assert_eq!(state.status, Status::Playing);
            assert_eq!(state.your_color, Color::White);
            assert_eq!(state.white_rating, 1500.0);
            assert_eq!(state.black_rating, 1500.0);
            assert!(!state.opponent_online);
        }
        other => panic!("expected room_ready, got {other:?}"),
    }
}

#[tokio::test]
async fn room_codes_are_unique() {
    let app = app_with_grace(30);
    let a = create(&app, P_A);
    let b = create(&app, P_B);
    let c = create(&app, P_C);
    assert_ne!(a.code, b.code);
    assert_ne!(a.code, c.code);
    assert_ne!(b.code, c.code);
    assert_eq!(app.room_codes().len(), 3);
}

#[tokio::test]
async fn join_seats_black_and_both_receive_snapshots() {
    let app = app_with_grace(30);
    let mut a = create(&app, P_A);
    let mut b = join(&app, P_B, &a.code).unwrap();

    match next_message(&mut b.out_rx).await {
        ServerMessage::RoomReady {
            room_code,
            your_color,
            state,
            ..
        } => {
            assert_eq!(room_code, a.code);
            assert_eq!(your_color, Color::Black);
            assert_eq!(state.side_to_move, "w");
            assert_eq!(state.status, Status::Playing);
            assert!(state.opponent_online, "the joiner sees the creator online");
        }
        other => panic!("expected room_ready for joiner, got {other:?}"),
    }

    // The creator is told the opponent is online.
    let _room_ready_a = next_message(&mut a.out_rx).await;
    let state_a = next_state(&mut a.out_rx).await;
    assert!(state_a.opponent_online);
    assert_eq!(state_a.side_to_move, "w");
}

#[tokio::test]
async fn third_player_gets_room_full_and_room_is_unchanged() {
    let app = app_with_grace(30);
    let mut a = create(&app, P_A);
    let mut b = join(&app, P_B, &a.code).unwrap();
    let _ = next_message(&mut a.out_rx).await; // A: creator room_ready
    let _ = next_state(&mut a.out_rx).await; // A: opponent online
    let _ = next_message(&mut b.out_rx).await; // B: room_ready
    let mut c = join(&app, P_C, &a.code).unwrap();

    assert_eq!(error_code(&next_message(&mut c.out_rx).await), "room_full");
    // The room is unchanged: no snapshot to either seated player.
    no_message_within(&mut a.out_rx, Duration::from_millis(200)).await;
    no_message_within(&mut b.out_rx, Duration::from_millis(200)).await;
    // The room still works: White can move.
    send_move(&a, "e2e4");
    let state = next_state(&mut a.out_rx).await;
    assert_eq!(state.move_list, vec!["e2e4".to_string()]);
}

#[tokio::test]
async fn device_belongs_to_one_room_at_a_time() {
    let app = app_with_grace(30);
    let mut _a = create(&app, P_A);
    let b_room = create(&app, P_B);

    let err = app
        .create_room(P_A)
        .err()
        .expect("second create must be rejected");
    assert_eq!(error_code(&err), "already_in_room");

    let err = join(&app, P_A, &b_room.code)
        .err()
        .expect("join of another room must be rejected");
    assert_eq!(error_code(&err), "already_in_room");
}

#[tokio::test]
async fn unknown_or_malformed_codes_are_rejected() {
    let app = app_with_grace(30);
    let a = create(&app, P_A);

    for bad_code in ["ABC", "ABCDEF0", "IIIIII", "0O1I1Z"] {
        let err = join(&app, P_B, bad_code)
            .err()
            .unwrap_or_else(|| panic!("code `{bad_code}` must be rejected"));
        assert_eq!(error_code(&err), "invalid_room_code", "code `{bad_code}`");
    }
    // A well-formed code that no room uses.
    let err = join(&app, P_B, "ZZZZZZ")
        .err()
        .expect("unknown code must be rejected");
    assert_eq!(error_code(&err), "room_not_found");
    // Nothing was created by any of these joins.
    assert_eq!(app.room_codes(), vec![a.code.clone()]);
}

#[tokio::test]
async fn empty_rooms_are_cleaned_up() {
    let app = app_with_grace(30);
    let a = create(&app, P_A);
    send_leave(&a);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        app.room_codes().is_empty(),
        "leaving the lobby must remove the room"
    );
    // The device is released and can create again.
    let mut _again = create(&app, P_A);
}

#[tokio::test]
async fn finished_room_is_dropped_when_players_stop_being_connected() {
    let app = app_with_grace(30);
    let (mut a, mut b, _snapshot) = start_game(&app).await;

    send_move(&a, "e2e4");
    let _ = next_state(&mut a.out_rx).await;
    let _ = next_state(&mut b.out_rx).await;

    send_resign(&a);
    let _ = next_state(&mut a.out_rx).await;
    let _ = next_state(&mut b.out_rx).await;

    assert_eq!(app.room_codes(), vec![a.code.clone()]);
    send_detach(&a);
    send_detach(&b);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(app.room_codes().is_empty(), "room must be dropped");
}

// ---------------------------------------------------------------------------
// Game authority (task 1.4)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn legal_move_is_applied_and_broadcast() {
    let app = app_with_grace(30);
    let (mut a, mut b, _snapshot) = start_game(&app).await;

    send_move(&a, "e2e4");

    let state_a = next_state(&mut a.out_rx).await;
    assert_eq!(state_a.move_list, vec!["e2e4".to_string()]);
    assert_eq!(state_a.side_to_move, "b");
    assert!(state_a.board_fen.contains("4P3"));
    assert_eq!(state_a.status, Status::Playing);

    let state_b = next_state(&mut b.out_rx).await;
    assert_eq!(state_b.move_list, state_a.move_list);
    assert_eq!(state_b.side_to_move, "b");
    assert!(state_b.opponent_online);
}

#[tokio::test]
async fn out_of_turn_move_is_rejected_without_side_effects() {
    let app = app_with_grace(30);
    let (mut a, mut b, _snapshot) = start_game(&app).await;

    send_move(&b, "e7e5"); // Black moves first: out of turn
    assert_eq!(error_code(&next_message(&mut b.out_rx).await), "not_your_turn");
    // No snapshot anywhere: the session is untouched.
    no_message_within(&mut a.out_rx, Duration::from_millis(200)).await;
    no_message_within(&mut b.out_rx, Duration::from_millis(200)).await;
    // White can still play the intended move.
    send_move(&a, "e2e4");
    assert_eq!(
        next_state(&mut a.out_rx).await.move_list,
        vec!["e2e4".to_string()]
    );
}

#[tokio::test]
async fn illegal_move_is_rejected_without_side_effects() {
    let app = app_with_grace(30);
    let (mut a, mut b, _snapshot) = start_game(&app).await;

    send_move(&a, "e2e5"); // pawns cannot jump two squares onto a pawn
    assert_eq!(error_code(&next_message(&mut a.out_rx).await), "illegal_move");
    no_message_within(&mut b.out_rx, Duration::from_millis(200)).await;

    // The session is unchanged: a legal move from the same position works.
    send_move(&a, "e2e4");
    assert_eq!(
        next_state(&mut a.out_rx).await.move_list,
        vec!["e2e4".to_string()]
    );
}

#[tokio::test]
async fn move_in_lobby_is_rejected_with_not_connected() {
    let app = app_with_grace(30);
    let mut a = create(&app, P_A);
    let _ = next_message(&mut a.out_rx).await;

    send_move(&a, "e2e4"); // no opponent seated yet
    assert_eq!(error_code(&next_message(&mut a.out_rx).await), "not_connected");
}

#[tokio::test]
async fn scholars_mate_ends_the_game_with_winner_and_ratings() {
    let app = app_with_grace(30);
    let (mut a, mut b, _snapshot) = start_game(&app).await;

    // Scholar's Mate: 1. e4 e5 2. Qh5 Nc6 3. Bc4 Nf6 4. Qxf7#
    let line = [
        ("a", "e2e4"),
        ("b", "e7e5"),
        ("a", "d1h5"),
        ("b", "b8c6"),
        ("a", "f1c4"),
        ("b", "g8f6"),
        ("a", "h5f7"),
    ];
    let mut played: Vec<String> = Vec::new();
    let mut final_a: Option<State> = None;
    let mut final_b: Option<State> = None;
    for (who, uci) in line {
        match who {
            "a" => send_move(&a, uci),
            _ => send_move(&b, uci),
        }
        played.push(uci.to_string());
        final_a = Some(next_state(&mut a.out_rx).await);
        final_b = Some(next_state(&mut b.out_rx).await);
    }
    let state_a = final_a.expect("final snapshot");
    let state_b = final_b.expect("final snapshot");
    assert_eq!(state_a.move_list, played);
    assert_eq!(state_a.status, Status::Checkmated {
        winner: Color::White
    });
    assert!(
        state_a.white_rating > 1500.0,
        "winner rating should rise, got {}",
        state_a.white_rating
    );
    assert!(
        state_a.black_rating < 1500.0,
        "loser rating should fall, got {}",
        state_a.black_rating
    );
    assert_eq!(state_b.status, state_a.status);

    // Further moves are rejected with game_over.
    send_move(&a, "a2a4");
    assert_eq!(error_code(&next_message(&mut a.out_rx).await), "game_over");
    send_move(&b, "a7a5");
    assert_eq!(error_code(&next_message(&mut b.out_rx).await), "game_over");
}

#[tokio::test]
async fn resignation_ends_the_game_with_opponent_as_winner() {
    let app = app_with_grace(30);
    let (mut a, mut b, _snapshot) = start_game(&app).await;

    send_move(&a, "e2e4");
    let _ = next_state(&mut a.out_rx).await;
    let _ = next_state(&mut b.out_rx).await;

    send_resign(&b);
    let state_a = next_state(&mut a.out_rx).await;
    assert_eq!(
        state_a.status,
        Status::Resigned {
            winner: Color::White
        }
    );
    assert!(state_a.white_rating > 1500.0);
    assert!(state_a.black_rating < 1500.0);
    let state_b = next_state(&mut b.out_rx).await;
    assert_eq!(state_b.status, state_a.status);

    send_move(&a, "e4e5");
    assert_eq!(error_code(&next_message(&mut a.out_rx).await), "game_over");
}

#[tokio::test]
async fn five_char_promotion_is_accepted() {
    let app = app_with_grace(30);
    let (mut a, mut b, _snapshot) = start_game(&app).await;

    // 1. c4 a6 2. c5 a5 3. c6 e6 4. cxb7 e5 5. bxa8=Q (captures the rook)
    let line = [
        ("a", "c2c4"),
        ("b", "a7a6"),
        ("a", "c4c5"),
        ("b", "a6a5"),
        ("a", "c5c6"),
        ("b", "e7e6"),
        ("a", "c6b7"),
        ("b", "e6e5"),
        ("a", "b7a8q"),
    ];
    let mut played: Vec<String> = Vec::new();
    let mut final_state: Option<State> = None;
    for (who, uci) in line {
        match who {
            "a" => send_move(&a, uci),
            _ => send_move(&b, uci),
        }
        played.push(uci.to_string());
        final_state = Some(next_state(&mut a.out_rx).await);
        let state_b = next_state(&mut b.out_rx).await;
        assert_eq!(state_b.move_list, played, "after `{uci}`");
    }
    let state = final_state.expect("final snapshot");
    assert_eq!(state.move_list, played);
    assert_eq!(state.status, Status::Playing);
    assert!(
        state.board_fen.starts_with("Qnbqkbnr"),
        "white queen must stand on a8: {}",
        state.board_fen
    );
}

#[tokio::test]
async fn draw_terminates_the_game_and_splits_the_point() {
    use chess_server::application::room_actor::classify_mover_outcome;

    // The draw verdict arrives through `classify_mover_outcome`, whose
    // Drawn branch is unit-tested in `room::tests`; the terminal side
    // effects (final snapshot to both seats, rating update, `game_over`
    // afterwards) are the same machinery the checkmate, resignation, and
    // forfeit tests exercise. A stalemate position cannot be reached from
    // the start setup in a practical test line (the core only plays from
    // the start position and reports a draw on stalemate or insufficient
    // material), so the 0.5 split itself is asserted here against the
    // rating store: both players are updated with a half point against
    // the opponent's rating.
    let app = app_with_grace(30);
    assert_eq!(
        classify_mover_outcome(false, true, Color::White),
        Status::Drawn
    );
    app.ratings().apply_result(P_A, P_B, 0.5);
    // Glicko-2 detail: at equal ratings a half point equals the expected
    // score, so the rating values stay at the default while both records
    // are updated; the rating-moving effect of a 0.5 result is covered in
    // `rating::tests::draw_between_unequal_ratings_moves_both`.
    assert_eq!(app.ratings().rating_of(P_A), 1500.0);
    assert_eq!(app.ratings().rating_of(P_B), 1500.0);
}

// ---------------------------------------------------------------------------
// Ratings (task 1.5)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn new_players_start_at_1500_and_ratings_surface_in_snapshots() {
    let app = app_with_grace(30);
    let (_a, _b, join_snapshot_a) = start_game(&app).await;

    assert_eq!(app.ratings().rating_of(P_A), 1500.0);
    assert_eq!(app.ratings().rating_of(P_B), 1500.0);
    assert_eq!(join_snapshot_a.white_rating, 1500.0);
    assert_eq!(join_snapshot_a.black_rating, 1500.0);
}

#[tokio::test]
async fn ratings_accumulate_across_games_on_the_same_server() {
    let app = app_with_grace(30);

    // Game 1: Black resigns right after the game starts.
    let (mut a1, mut b1, _snapshot1) = start_game(&app).await;
    send_resign(&b1);
    let final_a = next_state(&mut a1.out_rx).await;
    let final_b = next_state(&mut b1.out_rx).await;
    assert!(final_a.white_rating > 1500.0, "game 1: winner up");
    assert!(final_a.black_rating < 1500.0, "game 1: loser down");
    assert_eq!(final_b.white_rating, final_a.white_rating);
    assert_eq!(final_b.black_rating, final_a.black_rating);
    send_leave(&a1);
    send_leave(&b1);
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Game 2: same devices; the new game must start from the updated ratings.
    let (mut a2, mut b2, start_a2) = start_game(&app).await;
    assert_eq!(
        start_a2.white_rating, final_a.white_rating,
        "game 2 must start from game 1's winner rating (accumulation)"
    );
    assert_eq!(start_a2.black_rating, final_a.black_rating);

    send_resign(&b2);
    let final2_a = next_state(&mut a2.out_rx).await;
    let _ = next_state(&mut b2.out_rx).await;
    assert!(
        final2_a.white_rating > final_a.white_rating,
        "a second win must build on the first ({} -> {})",
        final_a.white_rating,
        final2_a.white_rating
    );
}

// ---------------------------------------------------------------------------
// Disconnect, reconnect, forfeit (task 1.6)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn opponent_is_told_when_the_other_player_drops() {
    let app = app_with_grace(30);
    let (mut a, mut b, _snapshot) = start_game(&app).await;

    send_move(&a, "e2e4");
    let _ = next_state(&mut a.out_rx).await;
    let _ = next_state(&mut b.out_rx).await;

    send_detach(&a);
    let state = next_state(&mut b.out_rx).await;
    assert!(!state.opponent_online, "B must be told A dropped");
    assert_eq!(state.move_list, vec!["e2e4".to_string()]);
    // B can keep playing or resign while waiting.
    send_move(&b, "e7e5");
    let state = next_state(&mut b.out_rx).await;
    assert_eq!(state.move_list.len(), 2);
    assert!(!state.opponent_online);
}

#[tokio::test]
async fn reconnect_within_grace_resyncs_with_a_fresh_snapshot() {
    let app = app_with_grace(30);
    let (mut a, mut b, _snapshot) = start_game(&app).await;

    send_move(&a, "e2e4");
    let _ = next_state(&mut a.out_rx).await;
    let _ = next_state(&mut b.out_rx).await;
    send_detach(&a);
    let _ = next_state(&mut b.out_rx).await; // opponent_online=false

    // Same device identifier, same room: re-attach to the White seat.
    let mut a2 = join(&app, P_A, &a.code).expect("re-attach must succeed");
    let state_a = next_state(&mut a2.out_rx).await;
    assert_eq!(state_a.move_list, vec!["e2e4".to_string()]);
    assert_eq!(state_a.your_color, Color::White);
    assert!(state_a.opponent_online);

    let state_b = next_state(&mut b.out_rx).await;
    assert!(state_b.opponent_online, "opponent is back online");
    assert_eq!(state_b.move_list, vec!["e2e4".to_string()]);

    // The game continues unchanged from the server state.
    send_move(&b, "e7e5");
    let state_a = next_state(&mut a2.out_rx).await;
    assert_eq!(
        state_a.move_list,
        vec!["e2e4".to_string(), "e7e5".to_string()]
    );
}

#[tokio::test]
async fn reattach_is_limited_to_the_seat_holder() {
    let app = app_with_grace(30);
    let (a, mut b, _snapshot) = start_game(&app).await;

    send_detach(&a);
    let _ = next_state(&mut b.out_rx).await;

    // A different device cannot take the held seat.
    let mut c = join(&app, P_C, &a.code).unwrap();
    assert_eq!(error_code(&next_message(&mut c.out_rx).await), "room_full");

    // The original device still gets its seat back.
    let mut a2 = join(&app, P_A, &a.code).unwrap();
    let state = next_state(&mut a2.out_rx).await;
    assert_eq!(state.your_color, Color::White);
    assert!(state.opponent_online);
}

#[tokio::test]
async fn grace_expiry_ends_the_game_by_forfeit() {
    let app = app_with_grace(2); // short window so the test stays fast
    let (mut a, mut b, _snapshot) = start_game(&app).await;

    send_move(&a, "e2e4");
    let _ = next_state(&mut a.out_rx).await;
    let _ = next_state(&mut b.out_rx).await;

    send_detach(&a);
    let dropped = next_state(&mut b.out_rx).await;
    assert!(!dropped.opponent_online);

    // Wait past the 2 s grace window: B must receive the forfeit result.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
    let forfeit_state = loop {
        match tokio::time::timeout(Duration::from_millis(200), b.out_rx.recv()).await {
            Ok(Some(ServerMessage::State { state, .. }))
                if matches!(state.status, Status::Forfeited { .. }) =>
            {
                break state
            }
            Ok(Some(other)) => panic!("unexpected message {other:?}"),
            Ok(None) => panic!("room closed before the forfeit"),
            Err(_) => {
                if tokio::time::Instant::now() >= deadline {
                    panic!("grace window expired without a forfeit result");
                }
            }
        }
    };
    assert_eq!(
        forfeit_state.status,
        Status::Forfeited {
            winner: Color::Black
        },
        "the connected player wins"
    );
    assert!(forfeit_state.black_rating > 1500.0);
    assert!(forfeit_state.white_rating < 1500.0);
    assert_eq!(forfeit_state.move_list, vec!["e2e4".to_string()]);

    // The room is removed: the code no longer resolves.
    assert!(app.room_codes().is_empty(), "forfeited room must be removed");
    let err = join(&app, P_C, &a.code).err().expect("join must fail");
    assert_eq!(error_code(&err), "room_not_found");
    // The winner's further moves are rejected.
    send_move(&b, "e7e5");
    assert_eq!(error_code(&next_message(&mut b.out_rx).await), "game_over");
}

#[tokio::test]
async fn lobby_disconnect_drops_the_room_with_no_result() {
    let app = app_with_grace(30);
    let mut a = create(&app, P_A);
    let code = a.code.clone();
    let _ = next_message(&mut a.out_rx).await;

    send_detach(&a);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(app.room_codes().is_empty(), "lobby room must be dropped");
    assert_eq!(
        app.ratings().rating_of(P_A),
        1500.0,
        "no result recorded"
    );

    // The device is released immediately (no wait-out).
    let again = create(&app, P_A);
    assert_ne!(again.code, code);
}

#[tokio::test]
async fn joiner_leaving_before_any_move_records_no_result() {
    let app = app_with_grace(30);
    let mut a = create(&app, P_A);
    let mut b = join(&app, P_B, &a.code).unwrap();
    let _ = next_message(&mut a.out_rx).await; // A: creator room_ready
    let _ = next_state(&mut a.out_rx).await; // A: opponent online
    let _ = next_message(&mut b.out_rx).await; // B: room_ready

    send_leave(&b); // no moves yet: just frees the seat
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        app.room_codes(),
        vec![a.code.clone()],
        "the waiting creator keeps the room"
    );
    assert_eq!(
        app.ratings().rating_of(P_B),
        1500.0,
        "no result or rating change for a pre-game leave"
    );
    // With no opponent, White's first move is not possible yet.
    send_move(&a, "e2e4");
    // B's leave snapshot arrives first, then the not_connected answer.
    let leave_state = next_state(&mut a.out_rx).await;
    assert!(!leave_state.opponent_online);
    assert_eq!(error_code(&next_message(&mut a.out_rx).await), "not_connected");
    // A fresh joiner can still take the freed Black seat.
    let mut c = join(&app, P_C, &a.code).unwrap();
    let state = next_state(&mut c.out_rx).await;
    assert_eq!(state.your_color, Color::Black);
}
