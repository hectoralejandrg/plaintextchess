//! Room actor: one task per room owns that room's authoritative game session
//! (design D2/D3). The actor is the single place where room state mutates, so
//! seat, turn, and timer logic is free of races.

use std::sync::Arc;

use chess_core::GameSession;
use tokio::sync::mpsc;
use tokio::time::{sleep_until, Instant};

use crate::app::App;
use crate::protocol::{Color, ErrorCode, ServerMessage, State, Status, VERSION};

const WHITE: usize = 0;
const BLACK: usize = 1;

/// Messages to the room actor. The connection handlers forward these; the
/// actor makes every decision.
#[derive(Debug)]
pub enum RoomMsg {
    /// A player is attaching to this room. `code == None` is only sent to a
    /// freshly spawned room: the sender created it and takes the White seat.
    /// `Some(code)` means join or re-attach (Black seat).
    Connect {
        player_id: String,
        code: Option<String>,
        out: mpsc::UnboundedSender<ServerMessage>,
    },
    Move {
        player_id: String,
        uci: String,
    },
    Resign {
        player_id: String,
    },
    /// Explicit leave: frees the seat (lobby/finished games) or counts as a
    /// resignation (game in progress), so a player cannot escape a finished
    /// result by just leaving.
    Leave {
        player_id: String,
    },
    /// The player's socket closed. The seat is held for the reconnect grace
    /// window (game in progress) or the room is dropped (lobby).
    Detach {
        player_id: String,
    },
}

#[derive(Debug)]
struct Seat {
    player_id: String,
    out: mpsc::UnboundedSender<ServerMessage>,
    connected: bool,
}

struct Room {
    code: String,
    app: Arc<App>,
    seats: [Option<Seat>; 2],
    session: Arc<GameSession>,
    move_list: Vec<String>,
    status: Status,
    /// Grace window for a disconnected seat: `(seat index, deadline)`.
    grace: Option<(usize, Instant)>,
}

impl Room {
    fn new(app: Arc<App>, code: String) -> Self {
        Self {
            code,
            app,
            seats: [None, None],
            session: chess_core::new_game_session(0.0),
            move_list: Vec::new(),
            status: Status::Playing,
            grace: None,
        }
    }

    fn started(&self) -> bool {
        self.seats[BLACK].is_some()
    }

    fn alive(&self) -> bool {
        self.seats[WHITE].is_some() || self.seats[BLACK].is_some()
    }

    fn seat_index_of(&self, player_id: &str) -> Option<usize> {
        self.seats
            .iter()
            .enumerate()
            .find(|(_, seat)| seat.as_ref().is_some_and(|s| s.player_id == player_id))
            .map(|(idx, _)| idx)
    }

    fn color_of(idx: usize) -> Color {
        if idx == WHITE {
            Color::White
        } else {
            Color::Black
        }
    }

    fn rating(&self, idx: usize) -> f64 {
        match &self.seats[idx] {
            Some(seat) => self.app.ratings().rating_of(&seat.player_id),
            None => crate::rating::DEFAULT_RATING,
        }
    }

    /// Full state snapshot for one seat (design D2): board placement, full
    /// move list, side to move, status, the seat's color, both ratings, and
    /// whether the opponent is connected.
    fn snapshot(&self, for_seat: usize) -> State {
        State {
            board_fen: self.session.get_board_state().expect("board state"),
            move_list: self.move_list.clone(),
            side_to_move: if self.move_list.len().is_multiple_of(2) {
                "w"
            } else {
                "b"
            }
            .into(),
            status: self.status.clone(),
            your_color: Self::color_of(for_seat),
            white_rating: self.rating(WHITE),
            black_rating: self.rating(BLACK),
            opponent_online: self.seats[1 - for_seat]
                .as_ref()
                .is_some_and(|seat| seat.connected),
        }
    }

    fn send_state(&self, idx: usize) {
        if let Some(seat) = &self.seats[idx] {
            if seat.connected {
                let _ = seat.out.send(ServerMessage::State {
                    v: VERSION,
                    state: self.snapshot(idx),
                });
            }
        }
    }

    fn send_state_both(&self) {
        self.send_state(WHITE);
        self.send_state(BLACK);
    }

    fn send_room_ready(&self, idx: usize) {
        if let Some(seat) = &self.seats[idx] {
            if seat.connected {
                let _ = seat.out.send(ServerMessage::RoomReady {
                    v: VERSION,
                    room_code: self.code.clone(),
                    your_color: Self::color_of(idx),
                    state: self.snapshot(idx),
                });
            }
        }
    }

    fn send_error(&self, idx: usize, code: ErrorCode) {
        if let Some(seat) = &self.seats[idx] {
            let _ = seat.out.send(ServerMessage::error(code));
        }
    }

    fn cancel_grace(&mut self) {
        self.grace = None;
    }

    fn apply_terminal_ratings(&self, winner_idx: usize) {
        let (Some(winner), Some(loser)) = (self.seats[winner_idx].as_ref(), self.seats[1 - winner_idx].as_ref())
        else {
            return;
        };
        let score = if matches!(self.status, Status::Drawn) {
            0.5
        } else {
            1.0
        };
        self.app
            .ratings()
            .apply_result(&winner.player_id, &loser.player_id, score);
    }

    /// Release a seat and any pending timer, unbinding the device.
    fn remove_seat(&mut self, idx: usize) {
        if self.grace.is_some_and(|(seat_idx, _)| seat_idx == idx) {
            self.cancel_grace();
        }
        if let Some(seat) = self.seats[idx].take() {
            self.app
                .untrack_player_if(&seat.player_id, &self.code);
        }
    }

    /// Drop the room from the registry (also frees any still-tracked seats).
    fn cleanup(&self) {
        self.app.remove_room(&self.code);
    }

    fn handle_connect(
        &mut self,
        player_id: String,
        join_code: Option<String>,
        out: mpsc::UnboundedSender<ServerMessage>,
    ) {
        if join_code.is_none() {
            // Creator: first message of a freshly spawned room.
            self.seats[WHITE] = Some(Seat {
                player_id,
                out,
                connected: true,
            });
            self.send_room_ready(WHITE);
            return;
        }

        // A finished game cannot seat anyone new (the forfeit path has
        // already removed the room from the registry, so this covers the
        // terminal-but-still-present rooms: mate/resign/draw while the
        // winner watches the result).
        if self.status.is_terminal() {
            let _ = out.send(ServerMessage::error(ErrorCode::RoomFull));
            self.app.untrack_player_if(&player_id, &self.code);
            return;
        }

        // Re-attach: the same device identifier returns to the seat it
        // occupied (spec: "Re-attachment MUST be limited to the player who
        // occupied the seat").
        for idx in [WHITE, BLACK] {
            if let Some(seat) = self.seats[idx].as_mut() {
                if seat.player_id == player_id {
                    seat.out = out;
                    seat.connected = true;
                    self.cancel_grace();
                    self.send_state(idx); // resync snapshot for the re-attached player
                    self.send_state(1 - idx); // opponent_online goes back to true
                    return;
                }
            }
        }

        // Seat is occupied by someone else.
        if self.seats[BLACK].is_some() {
            let _ = out.send(ServerMessage::error(ErrorCode::RoomFull));
            self.app.untrack_player_if(&player_id, &self.code);
            return;
        }

        // New Black player: the game starts.
        self.seats[BLACK] = Some(Seat {
            player_id,
            out,
            connected: true,
        });
        self.send_room_ready(BLACK);
        self.send_state(WHITE);
    }

    fn handle_move(&mut self, player_id: &str, uci: &str) {
        let Some(idx) = self.seat_index_of(player_id) else {
            return;
        };
        if !self.seats[idx].as_ref().is_some_and(|s| s.connected) {
            return;
        }
        if !self.started() {
            self.send_error(idx, ErrorCode::NotConnected);
            return;
        }
        if self.status.is_terminal() {
            self.send_error(idx, ErrorCode::GameOver);
            return;
        }
        let to_move = if self.move_list.len().is_multiple_of(2) {
            WHITE
        } else {
            BLACK
        };
        if idx != to_move {
            self.send_error(idx, ErrorCode::NotYourTurn);
            return;
        }
        if self.session.play_move(uci).is_err() {
            self.send_error(idx, ErrorCode::IllegalMove);
            return;
        }
        self.move_list.push(uci.to_string());

        let status = classify_mover_outcome(
            self.session.is_checkmate().expect("checkmate check"),
            self.session.is_draw().expect("draw check"),
            Self::color_of(idx),
        );
        self.status = status;
        if self.status.is_terminal() {
            self.apply_terminal_ratings(idx);
            self.cancel_grace(); // the game is over: nothing left to time out
        }
        self.send_state_both();
    }

    fn handle_resign(&mut self, player_id: &str) {
        let Some(idx) = self.seat_index_of(player_id) else {
            return;
        };
        if !self.started() {
            self.send_error(idx, ErrorCode::NotConnected);
            return;
        }
        if self.status.is_terminal() {
            self.send_error(idx, ErrorCode::GameOver);
            return;
        }
        let winner = 1 - idx;
        self.status = Status::Resigned {
            winner: Self::color_of(winner),
        };
        self.cancel_grace();
        self.apply_terminal_ratings(winner);
        self.send_state_both();
    }

    fn handle_leave(&mut self, player_id: &str) {
        let Some(idx) = self.seat_index_of(player_id) else {
            return;
        };
        if self.started() && !self.status.is_terminal() && !self.move_list.is_empty() {
            // Leaving a game with moves already played counts as a
            // resignation: the opponent wins, ratings update, the result is
            // final. Leaving before any move (lobby / first move not yet
            // made) just frees the seat: no result is recorded.
            let winner = 1 - idx;
            self.status = Status::Resigned {
                winner: Self::color_of(winner),
            };
            self.cancel_grace();
            self.apply_terminal_ratings(winner);
            self.remove_seat(idx);
            self.send_state_both();
            return;
        }
        // Lobby or finished game: just free the seat; the room drops when
        // the last seat is gone.
        self.remove_seat(idx);
        self.send_state_both();
    }

    fn handle_detach(&mut self, player_id: &str) {
        let Some(idx) = self.seat_index_of(player_id) else {
            return;
        };
        if !self.seats[idx].as_ref().is_some_and(|s| s.connected) {
            return; // already detached (double close)
        }
        self.seats[idx].as_mut().unwrap().connected = false;

        if !self.started() {
            // Lobby phase: the waiting player left, no result is recorded
            // (spec scenario "A lobby player who disconnects just leaves").
            self.remove_seat(idx);
            return;
        }
        if self.status.is_terminal() {
            // Game already over: nothing to forfeit; free the seat. The room
            // drops when the last seat is gone.
            self.remove_seat(idx);
            return;
        }
        if !self.seats[1 - idx].as_ref().is_some_and(|s| s.connected) {
            // Both players are gone: no result is recorded, the room is
            // dropped immediately (spec: "Empty rooms are cleaned up").
            self.remove_seat(idx);
            self.remove_seat(1 - idx);
            return;
        }
        // In play: hold the seat for the grace window and tell the opponent.
        let grace = self.app.config().reconnect_grace;
        self.grace = Some((idx, Instant::now() + grace));
        self.send_state(1 - idx);
    }

    fn handle_grace_expiry(&mut self, idx: usize) {
        let still_gone = self.seats[idx]
            .as_ref()
            .is_some_and(|s| !s.connected);
        let in_play = self.started() && !self.status.is_terminal();
        let opponent_connected = self
            .seats[1 - idx]
            .as_ref()
            .is_some_and(|s| s.connected);
        if !(still_gone && in_play && opponent_connected) {
            // Nothing to forfeit (re-attached meanwhile, game ended, or the
            // opponent is gone too — that path drops the room on detach).
            return;
        }
        let winner = 1 - idx;
        self.status = Status::Forfeited {
            winner: Self::color_of(winner),
        };
        self.apply_terminal_ratings(winner);
        self.send_state(winner); // final snapshot (forfeited, new ratings)
        self.remove_seat(idx); // the absent player can no longer re-attach
        self.app.remove_room(&self.code); // the room is gone from the registry
    }
}

/// The status a game carries after `mover`'s move is applied, given the
/// core's checkmate/draw verdicts (spec: "Checkmate or a draw ends the
/// game"). Mate wins for the mover; a core-reported draw ends the game as
/// a draw; otherwise the game keeps playing.
pub fn classify_mover_outcome(checkmated: bool, drawn: bool, mover: Color) -> Status {
    if checkmated {
        Status::Checkmated { winner: mover }
    } else if drawn {
        Status::Drawn
    } else {
        Status::Playing
    }
}

/// Run the room actor to completion (until every seat is gone).
pub async fn run(
    mut mailbox: mpsc::UnboundedReceiver<RoomMsg>,
    app: Arc<App>,
    code: String,
) {
    let mut room = Room::new(app, code);
    loop {
        tokio::select! {
            msg = mailbox.recv() => match msg {
                Some(RoomMsg::Connect {
                    player_id,
                    code: join_code,
                    out,
                }) => room.handle_connect(player_id, join_code, out),
                Some(RoomMsg::Move {
                    player_id,
                    uci,
                }) => room.handle_move(&player_id, &uci),
                Some(RoomMsg::Resign { player_id }) => room.handle_resign(&player_id),
                Some(RoomMsg::Leave { player_id }) => room.handle_leave(&player_id),
                Some(RoomMsg::Detach { player_id }) => room.handle_detach(&player_id),
                None => break, // both connection tasks gave up: room is dead
            },
            seat = grace_timeout(&room.grace) => room.handle_grace_expiry(seat),
        }
        if !room.alive() {
            room.cleanup();
            break;
        }
    }
}

/// Polls the grace window: resolves with the timed-out seat index, or stays
/// pending forever when no timer is armed.
async fn grace_timeout(grace: &Option<(usize, Instant)>) -> usize {
    match grace {
        Some((seat, deadline)) => {
            sleep_until(*deadline).await;
            *seat
        }
        None => std::future::pending::<usize>().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mate_is_classified_as_a_win_for_the_mover() {
        assert_eq!(
            classify_mover_outcome(true, false, Color::White),
            Status::Checkmated {
                winner: Color::White
            }
        );
        assert_eq!(
            classify_mover_outcome(true, false, Color::Black),
            Status::Checkmated {
                winner: Color::Black
            }
        );
    }

    #[test]
    fn a_core_reported_draw_ends_the_game_as_drawn() {
        assert_eq!(classify_mover_outcome(false, true, Color::White), Status::Drawn);
        assert_eq!(classify_mover_outcome(false, true, Color::Black), Status::Drawn);
    }

    #[test]
    fn an_unfinished_position_keeps_playing() {
        assert_eq!(classify_mover_outcome(false, false, Color::White), Status::Playing);
        assert_eq!(classify_mover_outcome(false, false, Color::Black), Status::Playing);
    }
}
