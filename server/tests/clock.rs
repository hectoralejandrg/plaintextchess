//! Clock scenarios (tasks 2.4/2.5): server-authoritative Fischer timing, flag
//! fall, in-flight semantics, and disconnect-aware settlement, driven through
//! `App` + channels with deliberately short controls so the tests stay fast
//! (the real presets are exercised by the domain unit tests and the E2E runs).

mod common;

use std::sync::Arc;
use std::time::Duration;

use chess_server::application::room_actor::RoomMsg;
use chess_server::domain::time_control::TimeControl;
use chess_server::infrastructure::server::Conn;
use chess_server::interface::protocol::{Color, ServerMessage, State, Status};
use common::*;
use tokio::sync::mpsc;
use tokio::time::timeout;

/// A ~1.5 s test control: short enough to flag quickly, long enough to play a
/// scripted line first. Kept generous so a loaded CI machine never flags a
/// side mid-setup.
const SHORT: TimeControl = TimeControl {
    base_ms: 1_500,
    increment_ms: 0,
};

fn send(conn: &Conn, msg: RoomMsg) {
    conn.mailbox.send(msg).expect("room mailbox closed");
}

fn send_move(conn: &Conn, uci: &str) {
    send(
        conn,
        RoomMsg::Move {
            player_id: conn.player_id.clone(),
            uci: uci.to_string(),
        },
    );
}

fn detach(conn: &Conn) {
    send(
        conn,
        RoomMsg::Detach {
            player_id: conn.player_id.clone(),
        },
    );
}

/// Creates a room with P_A and joins it with P_B under `time_control`,
/// consuming both `room_ready` messages plus the creator's join snapshot.
async fn start_timed_game(app: &Arc<App>, time_control: TimeControl) -> (Conn, Conn) {
    let mut a = app
        .create_room_with_time_control(P_A, time_control)
        .expect("create");
    let mut b = join(app, P_B, &a.code).expect("B joins");
    let _ = next_message(&mut a.out_rx).await; // room_ready for A
    let _ = next_message(&mut b.out_rx).await; // room_ready for B
    let _ = next_state(&mut a.out_rx).await; // A's join snapshot
    (a, b)
}

/// Reads snapshots until one is terminal, with a wall-clock guard so a broken
/// clock fails the test instead of hanging it.
async fn terminal_state(rx: &mut mpsc::UnboundedReceiver<ServerMessage>, within: Duration) -> State {
    timeout(within, async {
        loop {
            let state = next_state(rx).await;
            if state.status.is_terminal() {
                return state;
            }
        }
    })
    .await
    .expect("a terminal snapshot should arrive")
}

#[tokio::test]
async fn the_lobby_snapshot_carries_the_base_time_and_control() {
    let app = app_with_grace(30);
    let mut a = app
        .create_room_with_time_control(P_A, SHORT)
        .expect("create");

    let ready = next_state(&mut a.out_rx).await;
    assert_eq!(ready.status, Status::Playing);
    assert_eq!(ready.time_control, SHORT.label());
    assert_eq!(ready.white_time_ms, SHORT.base_ms);
    assert_eq!(ready.black_time_ms, SHORT.base_ms);
}

#[tokio::test]
async fn flag_fall_ends_the_game_with_the_opponent_winning() {
    let app = app_with_grace(120);
    let (mut a, mut b) = start_timed_game(&app, SHORT).await;

    // White stalls; the deadline tick settles the flag within ~1 s.
    let black_view = terminal_state(&mut b.out_rx, Duration::from_secs(5)).await;
    assert_eq!(
        black_view.status,
        Status::TimedOut {
            winner: Color::Black
        }
    );
    let white_view = terminal_state(&mut a.out_rx, Duration::from_secs(5)).await;
    assert_eq!(white_view.status, black_view.status);
    assert_eq!(white_view.white_time_ms, 0, "the flagged side is at zero");
    assert!(
        white_view.black_rating > 1500.0,
        "the winner's rating should rise, got {}",
        white_view.black_rating
    );
    assert!(
        white_view.white_rating < 1500.0,
        "the flagged player's rating should fall"
    );

    // Further moves are rejected with game_over.
    send_move(&a, "e2e4");
    assert_eq!(error_code(&next_message(&mut a.out_rx).await), "game_over");
}

#[tokio::test]
async fn a_completed_move_adds_the_fischer_increment() {
    let app = app_with_grace(30);
    let control = TimeControl {
        base_ms: 1_000,
        increment_ms: 2_000,
    };
    let (mut a, mut b) = start_timed_game(&app, control).await;

    // White takes a moment, moves, and earns the increment on completion.
    tokio::time::sleep(Duration::from_millis(150)).await;
    send_move(&a, "e2e4");
    let after_a = next_state(&mut a.out_rx).await;
    let after_b = next_state(&mut b.out_rx).await;

    // 1000 ms base − ~150 ms thinking + 2000 ms increment > base.
    assert!(
        after_a.white_time_ms > control.base_ms,
        "the increment must be added on completion, got {}",
        after_a.white_time_ms
    );
    assert_eq!(after_b.white_time_ms, after_a.white_time_ms);
    assert_eq!(after_a.time_control, control.label());
}

/// A legal 41-ply cooperative line that leaves White with a lone king (all
/// of White's other material captured); it is Black's turn at the end.
const STRIP_WHITE_TO_KING: &str = "g2g3 g8h6 g3g4 h6g4 e2e3 g4e3 f1a6 b7a6 d1g4 e3g4 \
    g1h3 g4h2 h3g5 h8g8 g5e6 f7e6 h1f1 h2f1 b1c3 f1d2 c3d5 e6d5 a1b1 d2b1 c1h6 g7h6 \
    c2c3 b1c3 b2b3 c3a2 b3b4 a2b4 f2f3 g8h8 f3f4 h8g8 f4f5 g8h8 f5f6 e7f6 e1f2";

#[tokio::test]
async fn a_flag_fall_with_insufficient_material_is_a_draw() {
    let app = app_with_grace(120);
    let control = TimeControl {
        base_ms: 10_000,
        increment_ms: 0,
    };
    let (mut a, mut b) = start_timed_game(&app, control).await;

    // Play the cooperative line: White is reduced to a lone king, and it is
    // Black to move. (A generous base keeps the 41-ply setup from ever
    // flagging a side on a loaded machine.)
    let line: Vec<String> = STRIP_WHITE_TO_KING
        .split_whitespace()
        .map(str::to_string)
        .collect();
    for (ply, uci) in line.iter().enumerate() {
        if ply % 2 == 0 {
            send_move(&a, uci);
        } else {
            send_move(&b, uci);
        }
        let _ = next_state(&mut a.out_rx).await;
        let _ = next_state(&mut b.out_rx).await;
    }

    // Black now stalls with a lone White king on the board: flag fall, but
    // White cannot checkmate, so the game is drawn.
    let black_view = terminal_state(&mut b.out_rx, Duration::from_secs(20)).await;
    assert_eq!(black_view.status, Status::Drawn);
    assert_eq!(black_view.move_list.len(), line.len());
    let white_view = terminal_state(&mut a.out_rx, Duration::from_secs(20)).await;
    assert_eq!(white_view.status, Status::Drawn);
}

#[tokio::test]
async fn reattach_resumes_with_the_remaining_time() {
    let app = app_with_grace(30);
    let control = TimeControl {
        base_ms: 5_000,
        increment_ms: 0,
    };
    let (mut a, mut b) = start_timed_game(&app, control).await;

    // White moves; Black is now on the clock with a full reading.
    send_move(&a, "e2e4");
    let _ = next_state(&mut a.out_rx).await;
    let started = next_state(&mut b.out_rx).await;
    assert_eq!(started.black_time_ms, control.base_ms);

    // Black disconnects and ~1 s elapses on its clock.
    detach(&b);
    let offline = next_state(&mut a.out_rx).await;
    assert!(!offline.opponent_online);
    tokio::time::sleep(Duration::from_millis(1_000)).await;

    // Black re-attaches: the resync snapshot carries the reduced time, with
    // no time granted.
    let mut b2 = join(&app, P_B, &a.code).expect("re-attach");
    let resync = next_state(&mut b2.out_rx).await;
    assert_eq!(resync.your_color, Color::Black);
    assert!(
        resync.black_time_ms < control.base_ms,
        "time must have elapsed while disconnected, got {}",
        resync.black_time_ms
    );
    assert!(
        resync.black_time_ms > 0,
        "no flag should have fallen within the grace window"
    );
    assert!(resync.opponent_online);
    assert_eq!(resync.move_list, vec!["e2e4".to_string()]);
}

#[tokio::test]
async fn a_flag_that_fell_while_disconnected_is_settled_on_reattach() {
    let app = app_with_grace(30);
    let control = TimeControl {
        base_ms: 700,
        increment_ms: 0,
    };
    let (a, mut b) = start_timed_game(&app, control).await;

    // It is White's turn at the start: White disconnects and stalls out.
    detach(&a);
    let offline = next_state(&mut b.out_rx).await;
    assert!(!offline.opponent_online);

    // White's clock keeps counting while away; the connected side sees no
    // terminal (the flag is deferred, design D8).
    tokio::time::sleep(Duration::from_millis(1_200)).await;

    // Re-attach: the deferred flag settles immediately as a loss on time.
    let mut a2 = join(&app, P_A, &a.code).expect("re-attach");
    let resync = terminal_state(&mut a2.out_rx, Duration::from_secs(3)).await;
    assert_eq!(
        resync.status,
        Status::TimedOut {
            winner: Color::Black
        }
    );
}

#[tokio::test]
async fn a_disconnect_that_outlives_the_grace_is_a_forfeit_even_with_time_left() {
    let app = app_with_grace(1); // 1 s grace window
    let control = TimeControl {
        base_ms: 10_000,
        increment_ms: 0, // nowhere near flagging
    };
    let (a, mut b) = start_timed_game(&app, control).await;

    // White disconnects on its turn with plenty of time left.
    detach(&a);
    let _ = next_state(&mut b.out_rx).await; // opponent offline

    // The grace window expires: the result is the disconnect forfeit, not a
    // flag fall.
    let final_view = terminal_state(&mut b.out_rx, Duration::from_secs(5)).await;
    assert_eq!(
        final_view.status,
        Status::Forfeited {
            winner: Color::Black
        }
    );
}
