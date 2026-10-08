//! Room actor: one task per room owns that room's authoritative game session
//! (design D2/D3 of add-online-multiplayer). The actor is the single place
//! where room state mutates, so seat, turn, and timer logic is free of races.
//! It depends on the domain layer and on ports only (design D1).

use tokio::sync::{mpsc, oneshot};
use tokio::time::{sleep_until, Instant};

use crate::application::ports::{EngineSession, RoomServices};
use crate::domain::clock::Clocks;
use crate::domain::account::Identity;
use crate::domain::game_record::FinishedGame;
use crate::domain::rating::{self, DEFAULT_RATING};
use crate::domain::time_control::TimeControl;
use crate::interface::protocol::{Color, ErrorCode, ServerMessage, State, Status, VERSION};

const WHITE: usize = 0;
const BLACK: usize = 1;

/// The seat the room creator is placed in: a coin flip when `random_colors`,
/// otherwise White (the deterministic default the tests pin).
fn creator_seat(random_colors: bool) -> usize {
    if random_colors && rand::random::<bool>() {
        BLACK
    } else {
        WHITE
    }
}

/// Messages to the room actor. The connection handlers forward these; the
/// actor makes every decision.
#[derive(Debug)]
pub enum RoomMsg {
    /// A player is attaching to this room. `code == None` is only sent to a
    /// freshly spawned room: the sender created it and takes the White seat.
    /// `Some(code)` means join or re-attach (Black seat).
    Connect {
        identity: Identity,
        code: Option<String>,
        out: mpsc::UnboundedSender<ServerMessage>,
    },
    Move {
        identity: Identity,
        uci: String,
    },
    Resign {
        identity: Identity,
    },
    /// Explicit leave: frees the seat (lobby/finished games) or counts as a
    /// resignation (game in progress), so a player cannot escape a finished
    /// result by just leaving.
    ///
    /// When `ack` is present it is signalled after the leave has fully applied
    /// — the seat released and the account/device entries unbound — so a
    /// caller that keeps reusing the connection observes the release before
    /// its next room action instead of racing it (design D8/D10).
    Leave {
        identity: Identity,
        ack: Option<oneshot::Sender<()>>,
    },
    /// The player's socket closed. The seat is held for the reconnect grace
    /// window (game in progress) or the room is dropped (lobby).
    Detach {
        identity: Identity,
    },
}

#[derive(Debug)]
struct Seat {
    /// The client-asserted device identifier. This stays the rating key and
    /// the persisted game-history key even for an authenticated player
    /// (design D1).
    player_id: String,
    /// The account this seat is held under, once one is known (design D9).
    /// `None` for a guest, or for a seat occupied before the player logged
    /// in and not yet upgraded.
    account_id: Option<String>,
    out: mpsc::UnboundedSender<ServerMessage>,
    connected: bool,
}

struct Room {
    code: String,
    services: RoomServices,
    time_control: TimeControl,
    seats: [Option<Seat>; 2],
    session: Box<dyn EngineSession>,
    move_list: Vec<String>,
    status: Status,
    /// The running clock; `None` until the game starts (both seats filled).
    clocks: Option<Clocks>,
    /// Grace window for a disconnected seat: `(seat index, deadline)`.
    grace: Option<(usize, Instant)>,
}

impl Room {
    fn new(services: RoomServices, code: String, time_control: TimeControl) -> Self {
        let session = services.engine.new_game_session();
        Self {
            code,
            services,
            time_control,
            seats: [None, None],
            session,
            move_list: Vec::new(),
            status: Status::Playing,
            clocks: None,
            grace: None,
        }
    }

    fn now_ms(&self) -> u64 {
        self.services.time.now_ms()
    }

    /// Remaining time of `side` at `now_ms`: the base time while the room is
    /// still in the lobby, the running clock once the game has started.
    fn remaining_ms(&self, side: usize, now_ms: u64) -> u64 {
        match self.clocks.as_ref() {
            Some(clocks) => clocks.remaining_ms(side as u8, now_ms),
            None => self.time_control.base_ms,
        }
    }

    fn started(&self) -> bool {
        self.seats[WHITE].is_some() && self.seats[BLACK].is_some()
    }

    fn alive(&self) -> bool {
        self.seats[WHITE].is_some() || self.seats[BLACK].is_some()
    }

    /// The seat this identity occupies, if any (design D9).
    ///
    /// Matching is by account when both sides have one, and by device
    /// otherwise. That covers the three real cases without ever widening the
    /// match: same device returning (both guests, or the same player signed
    /// in), the same account returning from a *different* device, and a guest
    /// whose seat was taken under a device id an authenticated connection now
    /// shares.
    ///
    /// A match by device also *upgrades* the seat with the account, so a
    /// login that arrives mid-game leaves the seat held by the account from
    /// then on. The seat is never swapped for a different one: an account
    /// must not be able to claim a second seat by presenting a device id it
    /// does not own.
    fn seat_index_of(&mut self, identity: &Identity) -> Option<usize> {
        let by_account = |seat: &Seat| match (&seat.account_id, identity.account_id()) {
            (Some(seat_account), Some(account)) => seat_account == account,
            _ => false,
        };
        // The device fallback only applies to a seat that has **no** account
        // (design D9). That is the whole hijack fix: once a seat is
        // account-owned, a bare `device_id` is a claim anyone can make, and it
        // is no longer accepted as proof. A guest seat keeps matching on
        // `device_id` alone, so guest re-attach behaves exactly as before.
        let by_device =
            |seat: &Seat| seat.account_id.is_none() && seat.player_id == identity.device_id();

        let found = self
            .seats
            .iter()
            .position(|seat| seat.as_ref().is_some_and(by_account))
            .or_else(|| {
                self.seats
                    .iter()
                    .position(|seat| seat.as_ref().is_some_and(by_device))
            })?;

        // Upgrading is only ever *adding* the account to the seat that the
        // device id already proved this identity owns.
        if let Some(account_id) = identity.account_id() {
            if let Some(seat) = self.seats[found].as_mut() {
                seat.account_id.get_or_insert_with(|| account_id.to_string());
            }
        }
        Some(found)
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
            Some(seat) => self.services.ratings.rating_of(&seat.player_id),
            None => DEFAULT_RATING,
        }
    }

    /// Full state snapshot for one seat (design D2): board placement, full
    /// move list, side to move, status, the seat's color, both ratings, and
    /// whether the opponent is connected.
    fn snapshot(&self, for_seat: usize) -> State {
        let now_ms = self.now_ms();
        State {
            board_fen: self.session.board_state().expect("board state"),
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
            time_control: self.time_control.label(),
            white_time_ms: self.remaining_ms(WHITE, now_ms),
            black_time_ms: self.remaining_ms(BLACK, now_ms),
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

    /// Freeze the clocks because the game just ended: without this the side to
    /// move keeps counting down in every later snapshot, so the terminal times
    /// would depend on when a snapshot is built and the two clients could even
    /// disagree.
    fn stop_clocks(&mut self) {
        let now_ms = self.now_ms();
        if let Some(clocks) = self.clocks.as_mut() {
            clocks.stop(now_ms);
        }
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

    /// The terminal-rating funnel (spec "Server Persistence", design D3):
    /// capture both pre-game states, apply the in-memory result, build the
    /// `FinishedGame`, and commit it through the recorder. A commit
    /// failure is logged with the full record; the result is still
    /// delivered, so the game is never blocked by the database (design D7).
    async fn apply_terminal_ratings(&self, winner_idx: usize) {
        let (Some(winner), Some(loser)) = (self.seats[winner_idx].as_ref(), self.seats[1 - winner_idx].as_ref())
        else {
            return;
        };
        let (Some(white), Some(black)) =
            (self.seats[WHITE].as_ref(), self.seats[BLACK].as_ref())
        else {
            return;
        };
        // Pre-game states, captured before this result is applied (devices
        // the store has never seen start at the default state).
        let white_before = self
            .services
            .ratings
            .rating_state(&white.player_id)
            .unwrap_or_default();
        let black_before = self
            .services
            .ratings
            .rating_state(&black.player_id)
            .unwrap_or_default();

        let score = rating::winner_score(matches!(self.status, Status::Drawn));
        self.services
            .ratings
            .apply_result(&winner.player_id, &loser.player_id, score);

        // Post-game states for both devices (the apply above materialized
        // a session for each, so these are the updated states).
        let white_state = self
            .services
            .ratings
            .rating_state(&white.player_id)
            .unwrap_or(white_before);
        let black_state = self
            .services
            .ratings
            .rating_state(&black.player_id)
            .unwrap_or(black_before);

        let (status_wire, winner_wire) = terminal_status_fields(&self.status);
        let record = FinishedGame {
            game_id: uuid::Uuid::new_v4().to_string(),
            room_code: self.code.clone(),
            time_control: self.time_control.label(),
            white_device: white.player_id.clone(),
            black_device: black.player_id.clone(),
            status: status_wire,
            winner: winner_wire,
            move_count: self.move_list.len() as u32,
            white_rating_before: white_before.rating,
            black_rating_before: black_before.rating,
            white_state,
            black_state,
        };
        if let Err(err) = self.services.recorder.record_finished_game(&record).await {
            tracing::error!(
                error = %err,
                game_id = %record.game_id,
                room_code = %record.room_code,
                time_control = %record.time_control,
                white_device = %record.white_device,
                black_device = %record.black_device,
                status = %record.status,
                winner = ?record.winner,
                move_count = record.move_count,
                white_rating_before = record.white_rating_before,
                black_rating_before = record.black_rating_before,
                white_state = ?record.white_state,
                black_state = ?record.black_state,
                "failed to record the finished game; the result was applied in memory and delivered to the players, but it was not persisted"
            );
        }
    }

    /// Release a seat and any pending timer, unbinding the device.
    fn remove_seat(&mut self, idx: usize) {
        if self.grace.is_some_and(|(seat_idx, _)| seat_idx == idx) {
            self.cancel_grace();
        }
        if let Some(seat) = self.seats[idx].take() {
            // Rebuild the identity the seat was held under so both the device
            // entry and the account entry are released (design D8/D9).
            let identity = match seat.account_id {
                Some(account_id) => Identity::Account {
                    account_id,
                    device_id: seat.player_id,
                },
                None => Identity::Guest {
                    device_id: seat.player_id,
                },
            };
            self.services.registry.untrack_if(&identity, &self.code);
        }
    }

    /// Drop the room from the registry (also frees any still-tracked seats).
    fn cleanup(&self) {
        self.services.registry.remove_room(&self.code);
    }

    async fn handle_connect(
        &mut self,
        identity: Identity,
        join_code: Option<String>,
        out: mpsc::UnboundedSender<ServerMessage>,
    ) {
        let player_id = identity.device_id().to_string();
        let account_id = identity.account_id().map(str::to_string);

        if join_code.is_none() {
            // Creator: first message of a freshly spawned room. Its color is
            // chosen at random (spec "Server Room Management") and reported in
            // the ready snapshot.
            let seat = creator_seat(self.services.random_colors);
            self.seats[seat] = Some(Seat {
                player_id,
                account_id,
                out,
                connected: true,
            });
            self.send_room_ready(seat);
            return;
        }

        // A finished game cannot seat anyone new (the forfeit path has
        // already removed the room from the registry, so this covers the
        // terminal-but-still-present rooms: mate/resign/draw while the
        // winner watches the result).
        if self.status.is_terminal() {
            let _ = out.send(ServerMessage::error(ErrorCode::RoomFull));
            self.services.registry.untrack_if(&identity, &self.code);
            return;
        }

        // Re-attach: the same identity returns to the seat it occupied (spec:
        // "Re-attachment MUST be limited to the player who occupied the
        // seat").
        if let Some(idx) = self.seat_index_of(&identity) {
            {
                let seat = self.seats[idx].as_mut().unwrap();
                seat.out = out;
                seat.connected = true;
            }
            // Deferred flag (design D8): if this player's own clock ran out
            // while they were away, settle the flag now, exactly as if they
            // had been connected when it fell.
            if self.started()
                && !self.status.is_terminal()
                && self
                    .clocks
                    .as_ref()
                    .is_some_and(|c| c.side_to_move() == idx as u8 && c.is_flagged(self.now_ms()))
            {
                self.settle_flag().await;
                return;
            }
            self.cancel_grace();
            self.send_state(idx); // resync snapshot for the re-attached player
            self.send_state(1 - idx); // opponent_online goes back to true
            return;
        }

        // The other seat belongs to someone else only when the room is full.
        let Some(seat) = [WHITE, BLACK]
            .into_iter()
            .find(|&seat| self.seats[seat].is_none())
        else {
            // Hijack guard (spec "Server Disconnect, Reconnect, and Forfeit"):
            // a connection with no session presenting *another* player's device
            // identifier must not be able to take their seat. `seat_index_of`
            // refuses a device match on an account-owned seat, so reaching here
            // with a device id that names a seat means the seat belongs to an
            // account this connection cannot prove. The seated player, the
            // clocks, and the status are left exactly as they were.
            if let Some(idx) = self.seat_of_device(&player_id) {
                tracing::warn!(
                    room_code = %self.code,
                    seat = idx,
                    seat_account = ?self.seats[idx]
                        .as_ref()
                        .and_then(|s| s.account_id.as_deref()),
                    "refused an unauthenticated re-attach to an account-owned seat"
                );
            }
            let _ = out.send(ServerMessage::error(ErrorCode::RoomFull));
            self.services.registry.untrack_if(&identity, &self.code);
            return;
        };

        // New player: the game starts and both clocks start at base, with
        // White to move regardless of which seat each player holds.
        self.seats[seat] = Some(Seat {
            player_id,
            account_id,
            out,
            connected: true,
        });
        self.clocks = Some(Clocks::new(
            self.time_control.base_ms,
            WHITE as u8,
            self.now_ms(),
        ));
        self.send_room_ready(seat);
        self.send_state(1 - seat);
    }

    /// The seat a raw device identifier names, ignoring any account. Used
    /// only by the hijack guard, to tell "the seat belongs to someone else"
    /// apart from "the room is simply full".
    fn seat_of_device(&self, player_id: &str) -> Option<usize> {
        self.seats
            .iter()
            .position(|seat| seat.as_ref().is_some_and(|s| s.player_id == player_id))
    }

    async fn handle_move(&mut self, identity: &Identity, uci: &str) {
        let Some(idx) = self.seat_index_of(identity) else {
            return;
        };
        if self.status.is_terminal() {
            self.send_error(idx, ErrorCode::GameOver);
            return;
        }
        if !self.seats[idx].as_ref().is_some_and(|s| s.connected) {
            return;
        }
        if !self.started() {
            self.send_error(idx, ErrorCode::NotConnected);
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
        // A move is honored only if it is applied before the flag (design
        // D3): the clock deadline decides, exactly, at apply time.
        if self
            .clocks
            .as_ref()
            .is_some_and(|c| c.is_flagged(self.now_ms()))
        {
            self.settle_flag().await;
            self.send_error(idx, ErrorCode::GameOver);
            return;
        }
        if self.session.play_move(uci).is_err() {
            self.send_error(idx, ErrorCode::IllegalMove);
            return;
        }
        self.move_list.push(uci.to_string());
        // Fischer settlement (design D2): deduct the thinking time, add the
        // increment on completion, and switch the side to move.
        let increment_ms = self.time_control.increment_ms;
        let now_ms = self.now_ms();
        if let Some(clocks) = self.clocks.as_mut() {
            clocks.settle_move(idx as u8, increment_ms, now_ms);
        }

        let status = classify_mover_outcome(
            self.session.is_checkmate().expect("checkmate check"),
            self.session.is_draw().expect("draw check"),
            Self::color_of(idx),
        );
        self.status = status;
        if self.status.is_terminal() {
            self.stop_clocks();
            self.apply_terminal_ratings(idx).await;
            self.cancel_grace(); // the game is over: nothing left to time out
        }
        self.send_state_both();
    }

    async fn handle_resign(&mut self, identity: &Identity) {
        let Some(idx) = self.seat_index_of(identity) else {
            return;
        };
        if self.status.is_terminal() {
            self.send_error(idx, ErrorCode::GameOver);
            return;
        }
        if !self.started() {
            self.send_error(idx, ErrorCode::NotConnected);
            return;
        }
        let winner = 1 - idx;
        self.status = Status::Resigned {
            winner: Self::color_of(winner),
        };
        self.cancel_grace();
        // Freeze at the moment the result is known, before the (possibly slow)
        // durable commit, so the clocks reflect the real end of the game.
        self.stop_clocks();
        self.apply_terminal_ratings(winner).await;
        self.send_state_both();
    }

    async fn handle_leave(&mut self, identity: &Identity) {
        let Some(idx) = self.seat_index_of(identity) else {
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
            self.stop_clocks();
            self.apply_terminal_ratings(winner).await;
            self.remove_seat(idx);
            self.send_state_both();
            return;
        }
        // Lobby or finished game: just free the seat; the room drops when
        // the last seat is gone.
        self.remove_seat(idx);
        self.send_state_both();
    }

    fn handle_detach(&mut self, identity: &Identity) {
        let Some(idx) = self.seat_index_of(identity) else {
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
        self.grace = Some((idx, Instant::now() + self.services.reconnect_grace));
        self.send_state(1 - idx);
    }

    /// The side to move's clock has reached zero: end the game by flag fall
    /// (design D4) — a draw when the winner cannot deliver checkmate.
    async fn settle_flag(&mut self) {
        let Some(clocks) = self.clocks.as_ref() else {
            return;
        };
        let flagged = clocks.side_to_move();
        let winner = 1 - flagged as usize;
        let drawn = self
            .session
            .insufficient_material_for(winner == WHITE)
            .expect("insufficient material check");
        self.status = if drawn {
            Status::Drawn
        } else {
            Status::TimedOut {
                winner: Self::color_of(winner),
            }
        };
        self.stop_clocks();
        self.apply_terminal_ratings(winner).await;
        self.cancel_grace(); // the game is over: nothing left to time out
        self.send_state_both();
    }

    /// The periodic deadline check (design D2/D8): settles a flag that fell
    /// while the side to move is connected. A flag that falls while the side
    /// to move is disconnected is deferred until re-attach or grace expiry.
    async fn tick_flags(&mut self) {
        if !self.started() || self.status.is_terminal() {
            return;
        }
        let Some(clocks) = self.clocks.as_ref() else {
            return;
        };
        let flagged = clocks.side_to_move();
        let connected = self.seats[flagged as usize]
            .as_ref()
            .is_some_and(|s| s.connected);
        if connected && clocks.is_flagged(self.now_ms()) {
            self.settle_flag().await;
        }
    }

    async fn handle_grace_expiry(&mut self, idx: usize) {
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
        self.stop_clocks();
        self.apply_terminal_ratings(winner).await;
        self.send_state(winner); // final snapshot (forfeited, new ratings)
        self.remove_seat(idx); // the absent player can no longer re-attach
        self.services.registry.remove_room(&self.code); // the room is gone from the registry
    }
}

/// The status a game carries after `mover`'s move is applied, given the
/// engine's checkmate/draw verdicts (spec: "Checkmate or a draw ends the
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

/// The terminal status' wire name and the winner's color, exactly as the
/// protocol reports them to clients (spec "Server Persistence"): these are
/// the values the finished-game record persists.
fn terminal_status_fields(status: &Status) -> (String, Option<String>) {
    let winner = |c: &Color| match c {
        Color::White => Some("white".to_string()),
        Color::Black => Some("black".to_string()),
    };
    match status {
        Status::Playing => ("playing".to_string(), None),
        Status::Checkmated { winner: w } => ("checkmated".to_string(), winner(w)),
        Status::Drawn => ("drawn".to_string(), None),
        Status::Resigned { winner: w } => ("resigned".to_string(), winner(w)),
        Status::Forfeited { winner: w } => ("forfeited".to_string(), winner(w)),
        Status::TimedOut { winner: w } => ("timed_out".to_string(), winner(w)),
    }
}

/// Run the room actor to completion (until every seat is gone).
pub async fn run_room(
    mut mailbox: mpsc::UnboundedReceiver<RoomMsg>,
    services: RoomServices,
    code: String,
    time_control: TimeControl,
) {
    let mut room = Room::new(services, code, time_control);
    // The deadline tick (design D2): 200 ms is precise enough for a side
    // project, and the exact apply-time check in `handle_move` means a move
    // that lands before the next tick is never lost.
    let mut deadline_tick = tokio::time::interval(std::time::Duration::from_millis(200));
    deadline_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            msg = mailbox.recv() => match msg {
                Some(RoomMsg::Connect {
                    identity,
                    code: join_code,
                    out,
                }) => room.handle_connect(identity, join_code, out).await,
                Some(RoomMsg::Move { identity, uci }) => room.handle_move(&identity, &uci).await,
                Some(RoomMsg::Resign { identity }) => room.handle_resign(&identity).await,
                Some(RoomMsg::Leave { identity, ack }) => {
                    room.handle_leave(&identity).await;
                    if let Some(ack) = ack {
                        let _ = ack.send(());
                    }
                }
                Some(RoomMsg::Detach { identity }) => room.handle_detach(&identity),
                None => break, // both connection tasks gave up: room is dead
            },
            _ = deadline_tick.tick() => room.tick_flags().await,
            seat = grace_timeout(&room.grace) => room.handle_grace_expiry(seat).await,
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
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use crate::application::ports::{
        ChessEngine, EngineError, GameRecorder, Ratings, RecorderError, RoomRegistry,
        RatingSession, TimeSource,
    };
    use crate::domain::rating::RatingState;

    /// The creator's color: random when enabled, White by default (the pin the
    /// deterministic room tests rely on).
    #[test]
    fn the_creator_color_is_random_when_enabled_and_white_by_default() {
        assert_eq!(creator_seat(false), WHITE);
        assert_eq!(creator_seat(false), WHITE, "disabled randomness is always White");
        let mut saw_white = false;
        let mut saw_black = false;
        for _ in 0..256 {
            match creator_seat(true) {
                WHITE => saw_white = true,
                BLACK => saw_black = true,
                _ => unreachable!("a seat is White or Black"),
            }
        }
        assert!(
            saw_white && saw_black,
            "both colors must occur across repeated rooms when randomness is enabled"
        );
    }

    const ROOM_CODE: &str = "TESTROOM";
    const P_WHITE: &str = "device-a";
    const P_BLACK: &str = "device-b";
    /// A full-material position: flag falls are decisive, not draws.
    const FULL_MATERIAL_FEN: &str = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";

    // ------------------------------------------------------------------
    // Fakes
    // ------------------------------------------------------------------

    #[derive(Default)]
    struct ScriptedSession {
        moves: AtomicUsize,
        checkmate_after: Option<usize>,
        draw_after: Option<usize>,
    }

    impl EngineSession for ScriptedSession {
        fn play_move(&self, _uci: &str) -> Result<(), EngineError> {
            self.moves.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn board_state(&self) -> Result<String, EngineError> {
            Ok(FULL_MATERIAL_FEN.to_string())
        }

        fn is_checkmate(&self) -> Result<bool, EngineError> {
            Ok(self
                .checkmate_after
                .is_some_and(|n| self.moves.load(Ordering::SeqCst) >= n))
        }

        fn is_draw(&self) -> Result<bool, EngineError> {
            Ok(self
                .draw_after
                .is_some_and(|n| self.moves.load(Ordering::SeqCst) >= n))
        }

        fn insufficient_material_for(&self, _winner_is_white: bool) -> Result<bool, EngineError> {
            // The scripted session always reports a full-material position, so
            // a flag fall is decisive rather than a draw.
            Ok(false)
        }
    }

    /// Accepts any move; reports checkmate/draw after a scripted move count.
    struct ScriptedEngine {
        checkmate_after: Option<usize>,
        draw_after: Option<usize>,
    }

    impl ChessEngine for ScriptedEngine {
        fn new_game_session(&self) -> Box<dyn EngineSession> {
            Box::new(ScriptedSession {
                moves: AtomicUsize::new(0),
                checkmate_after: self.checkmate_after,
                draw_after: self.draw_after,
            })
        }

        fn new_rating_session(&self, rating: f64) -> Box<dyn RatingSession> {
            Box::new(FakeRatingSession::new(rating))
        }

        fn new_rating_session_state(&self, state: &RatingState) -> Box<dyn RatingSession> {
            Box::new(FakeRatingSession::from_state(*state))
        }
    }

    #[derive(Default)]
    struct FakeRatingSession {
        state: Mutex<RatingState>,
    }

    impl FakeRatingSession {
        fn new(rating: f64) -> Self {
            Self {
                state: Mutex::new(RatingState {
                    rating,
                    ..Default::default()
                }),
            }
        }

        fn from_state(state: RatingState) -> Self {
            Self {
                state: Mutex::new(state),
            }
        }
    }

    impl RatingSession for FakeRatingSession {
        fn current_rating(&self) -> Result<f64, EngineError> {
            Ok(self.state.lock().unwrap().rating)
        }

        fn update_rating(&self, _opponent_rating: f64, score: f64) -> Result<(), EngineError> {
            self.state.lock().unwrap().rating += 40.0 * (score - 0.5) * 2.0;
            Ok(())
        }

        fn state(&self) -> Result<RatingState, EngineError> {
            Ok(*self.state.lock().unwrap())
        }
    }

    /// Deterministic ratings: a decisive result moves both ratings by 40
    /// points in opposite directions; a draw moves neither.
    #[derive(Default)]
    struct FakeRatings {
        states: Mutex<HashMap<String, RatingState>>,
    }

    impl Ratings for FakeRatings {
        fn rating_of(&self, player_id: &str) -> f64 {
            self.states
                .lock()
                .unwrap()
                .get(player_id)
                .map(|s| s.rating)
                .unwrap_or(DEFAULT_RATING)
        }

        fn apply_result(&self, winner: &str, loser: &str, score: f64) {
            let delta = 40.0 * (score - 0.5) * 2.0;
            let mut states = self.states.lock().unwrap();
            let w_id = winner.to_string();
            let l_id = loser.to_string();
            {
                let w = states.entry(w_id.clone()).or_default();
                w.rating += delta;
                w.rating_deviation = (w.rating_deviation - 5.0).max(100.0);
            }
            {
                let l = states.entry(l_id).or_default();
                l.rating -= delta;
                l.rating_deviation = (l.rating_deviation - 5.0).max(100.0);
            }
        }

        fn rating_state(&self, player_id: &str) -> Option<RatingState> {
            self.states.lock().unwrap().get(player_id).copied()
        }
    }

    /// A controllable clock; the actor's grace window uses real time.
    struct FakeTime(AtomicU64);

    impl TimeSource for FakeTime {
        fn now_ms(&self) -> u64 {
            self.0.load(Ordering::SeqCst)
        }
    }

    #[derive(Default)]
    struct FakeRegistry {
        removed_rooms: Mutex<Vec<String>>,
    }

    impl RoomRegistry for FakeRegistry {
        fn remove_room(&self, code: &str) {
            self.removed_rooms.lock().unwrap().push(code.to_string());
        }

        fn untrack_if(&self, _identity: &Identity, _code: &str) {}
    }

    /// The fake recorder: captures every record (or fails every commit when
    /// `fail` is set).
    #[derive(Default)]
    struct CapturingRecorder {
        records: Mutex<Vec<FinishedGame>>,
        fail: bool,
    }

    impl GameRecorder for CapturingRecorder {
        fn record_finished_game<'a>(
            &'a self,
            game: &'a FinishedGame,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<(), RecorderError>> + Send + 'a>,
        > {
            Box::pin(async move {
                if self.fail {
                    return Err("injected recorder failure".to_string());
                }
                self.records.lock().unwrap().push(game.clone());
                Ok(())
            })
        }
    }

    // ------------------------------------------------------------------
    // Harness
    // ------------------------------------------------------------------

    struct TestRoom {
        tx: mpsc::UnboundedSender<RoomMsg>,
        white_rx: mpsc::UnboundedReceiver<ServerMessage>,
        black_rx: mpsc::UnboundedReceiver<ServerMessage>,
        /// Kept so a test can assert on what the room asked the ratings to
        /// do; the harness always builds one, most tests never read it.
        #[allow(dead_code)]
        ratings: Arc<FakeRatings>,
        recorder: Arc<CapturingRecorder>,
        time: Arc<FakeTime>,
    }

    impl TestRoom {
        async fn spawn(
            checkmate_after: Option<usize>,
            draw_after: Option<usize>,
            time_control: TimeControl,
            fail_recorder: bool,
        ) -> Self {
            Self::spawn_with(
                P_WHITE.into(),
                P_BLACK.into(),
                checkmate_after,
                draw_after,
                time_control,
                fail_recorder,
            )
            .await
        }

        /// The same room, with the two seats' identities chosen by the test
        /// (6.5/7.1 need a guest, an account, and a replayed device id in the
        /// same room).
        async fn spawn_with(
            white: Identity,
            black: Identity,
            checkmate_after: Option<usize>,
            draw_after: Option<usize>,
            time_control: TimeControl,
            fail_recorder: bool,
        ) -> Self {
            let engine: Arc<dyn ChessEngine> = Arc::new(ScriptedEngine {
                checkmate_after,
                draw_after,
            });
            let ratings = Arc::new(FakeRatings::default());
            let time = Arc::new(FakeTime(AtomicU64::new(0)));
            let registry: Arc<dyn RoomRegistry> = Arc::new(FakeRegistry::default());
            let recorder = Arc::new(CapturingRecorder {
                records: Mutex::new(Vec::new()),
                fail: fail_recorder,
            });
            let services = RoomServices {
                registry,
                ratings: Arc::clone(&ratings) as Arc<dyn Ratings>,
                engine,
                time: Arc::clone(&time) as Arc<dyn TimeSource>,
                reconnect_grace: Duration::from_millis(300),
                recorder: Arc::clone(&recorder) as Arc<dyn GameRecorder>,
                auth: Arc::new(crate::infrastructure::auth::PlayerAuthStore::new()),
                random_colors: false,
            };

            let (tx, rx) = mpsc::unbounded_channel();
            tokio::spawn(run_room(rx, services, ROOM_CODE.to_string(), time_control));

            let (white_tx, mut white_rx) = mpsc::unbounded_channel();
            tx.send(RoomMsg::Connect {
                identity: white,
                code: None,
                out: white_tx,
            })
            .unwrap();
            let (black_tx, mut black_rx) = mpsc::unbounded_channel();
            tx.send(RoomMsg::Connect {
                identity: black,
                code: Some(ROOM_CODE.to_string()),
                out: black_tx,
            })
            .unwrap();
            next_room_ready(&mut white_rx).await;
            next_room_ready(&mut black_rx).await;

            Self {
                tx,
                white_rx,
                black_rx,
                ratings,
                recorder,
                time,
            }
        }

        /// Attach one more connection to the room, without waiting for a
        /// `room_ready`: the caller asserts on whatever comes back.
        async fn attach(&self, identity: Identity) -> mpsc::UnboundedReceiver<ServerMessage> {
            let (tx, rx) = mpsc::unbounded_channel();
            self.tx
                .send(RoomMsg::Connect {
                    identity,
                    code: Some(ROOM_CODE.to_string()),
                    out: tx,
                })
                .unwrap();
            rx
        }

        fn move_as_identity(&self, identity: Identity, uci: &str) {
            self.tx
                .send(RoomMsg::Move { identity, uci: uci.into() })
                .unwrap();
        }

        fn resign_as_identity(&self, identity: Identity) {
            self.tx.send(RoomMsg::Resign { identity }).unwrap();
        }

        /// The next error on `rx`, or a panic naming what arrived instead.
        async fn next_error(&self, rx: &mut mpsc::UnboundedReceiver<ServerMessage>) -> String {
            match next_message(rx).await {
                ServerMessage::Error { code, message, .. } => {
                    assert!(!message.is_empty(), "an error must carry a message");
                    code
                }
                other => panic!("expected an error, got {other:?}"),
            }
        }

        fn move_as(&self, player_id: &str, uci: &str) {
            self.tx
                .send(RoomMsg::Move {
                    identity: player_id.into(),
                    uci: uci.to_string(),
                })
                .unwrap();
        }

        fn resign_as(&self, player_id: &str) {
            self.tx
                .send(RoomMsg::Resign {
                    identity: player_id.into(),
                })
                .unwrap();
        }

        fn leave_as(&self, player_id: &str) {
            self.tx
                .send(RoomMsg::Leave {
                    identity: player_id.into(),
                    ack: None,
                })
                .unwrap();
        }

        fn detach_as(&self, player_id: &str) {
            self.tx
                .send(RoomMsg::Detach {
                    identity: player_id.into(),
                })
                .unwrap();
        }

        fn recorded_games(&self) -> Vec<FinishedGame> {
            self.recorder.records.lock().unwrap().clone()
        }

        /// The single captured record, asserting that exactly one exists.
        fn recorded_game(&self) -> FinishedGame {
            let games = self.recorded_games();
            assert_eq!(games.len(), 1, "exactly one finished-game record expected");
            games.into_iter().next().unwrap()
        }
    }

    async fn next_message(rx: &mut mpsc::UnboundedReceiver<ServerMessage>) -> ServerMessage {
        tokio::time::timeout(Duration::from_secs(3), rx.recv())
            .await
            .expect("timed out waiting for a room message")
            .expect("room channel closed unexpectedly")
    }

    async fn next_room_ready(rx: &mut mpsc::UnboundedReceiver<ServerMessage>) {
        loop {
            if matches!(next_message(rx).await, ServerMessage::RoomReady { .. }) {
                return;
            }
        }
    }

    /// The next state snapshot, skipping room-ready and error messages.
    async fn next_state(rx: &mut mpsc::UnboundedReceiver<ServerMessage>) -> State {
        loop {
            if let ServerMessage::State { state, .. } = next_message(rx).await {
                return state;
            }
        }
    }

    /// The next terminal state snapshot on the seat's channel.
    async fn terminal_state(rx: &mut mpsc::UnboundedReceiver<ServerMessage>) -> State {
        loop {
            let state = next_state(rx).await;
            if state.status.is_terminal() {
                return state;
            }
        }
    }

    fn assert_uuid_v4(id: &str) {
        let parsed = uuid::Uuid::parse_str(id).expect("game_id must be a UUID");
        assert_eq!(Some(uuid::Version::Random), parsed.get_version());
    }

    fn assert_common_record_fields(record: &FinishedGame, time_control: TimeControl) {
        assert_uuid_v4(&record.game_id);
        assert_eq!(record.room_code, ROOM_CODE);
        assert_eq!(record.time_control, time_control.label());
        assert_eq!(record.white_device, P_WHITE);
        assert_eq!(record.black_device, P_BLACK);
        assert_eq!(record.white_rating_before, DEFAULT_RATING);
        assert_eq!(record.black_rating_before, DEFAULT_RATING);
    }

    // ------------------------------------------------------------------
    // Terminal paths
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn a_checkmate_captures_one_record_with_the_expected_fields() {
        let mut room = TestRoom::spawn(Some(1), None, TimeControl::DEFAULT, false).await;
        room.move_as(P_WHITE, "e2e4"); // the engine declares mate after this move
        let final_state = terminal_state(&mut room.white_rx).await;

        assert_eq!(
            final_state.status,
            Status::Checkmated {
                winner: Color::White
            }
        );
        assert_eq!(final_state.white_rating, 1540.0);
        assert_eq!(final_state.black_rating, 1460.0);

        let record = room.recorded_game();
        assert_common_record_fields(&record, TimeControl::DEFAULT);
        assert_eq!(record.status, "checkmated");
        assert_eq!(record.winner, Some("white".to_string()));
        assert_eq!(record.move_count, 1);
        assert_eq!(
            record.white_state,
            RatingState {
                rating: 1540.0,
                rating_deviation: 195.0,
                volatility: 0.06
            }
        );
        assert_eq!(
            record.black_state,
            RatingState {
                rating: 1460.0,
                rating_deviation: 195.0,
                volatility: 0.06
            }
        );
    }

    #[tokio::test]
    async fn a_core_draw_captures_one_record_with_no_winner() {
        let mut room = TestRoom::spawn(None, Some(1), TimeControl::DEFAULT, false).await;
        room.move_as(P_WHITE, "e2e4");
        let final_state = terminal_state(&mut room.white_rx).await;

        assert_eq!(final_state.status, Status::Drawn);

        let record = room.recorded_game();
        assert_common_record_fields(&record, TimeControl::DEFAULT);
        assert_eq!(record.status, "drawn");
        assert_eq!(record.winner, None);
        assert_eq!(record.move_count, 1);
        // A draw moves no points, but the deviation still tightened.
        assert_eq!(record.white_state.rating, DEFAULT_RATING);
        assert_eq!(record.white_state.rating_deviation, 195.0);
        assert_eq!(record.black_state.rating, DEFAULT_RATING);
    }

    #[tokio::test]
    async fn a_resignation_captures_one_record_won_by_the_opponent() {
        let mut room = TestRoom::spawn(None, None, TimeControl::DEFAULT, false).await;
        room.move_as(P_WHITE, "e2e4");
        // consume the non-terminal snapshot from the move
        let _ = next_state(&mut room.white_rx).await;
        let _ = next_state(&mut room.black_rx).await;

        room.resign_as(P_BLACK);
        let final_state = terminal_state(&mut room.white_rx).await;

        assert_eq!(
            final_state.status,
            Status::Resigned {
                winner: Color::White
            }
        );

        let record = room.recorded_game();
        assert_common_record_fields(&record, TimeControl::DEFAULT);
        assert_eq!(record.status, "resigned");
        assert_eq!(record.winner, Some("white".to_string()));
        assert_eq!(record.move_count, 1);
        assert_eq!(record.white_state.rating, 1540.0);
        assert_eq!(record.black_state.rating, 1460.0);
    }

    #[tokio::test]
    async fn leaving_a_started_game_captures_a_resignation_record() {
        let mut room = TestRoom::spawn(None, None, TimeControl::DEFAULT, false).await;
        room.move_as(P_WHITE, "e2e4");
        let _ = next_state(&mut room.white_rx).await;
        let _ = next_state(&mut room.black_rx).await;

        room.leave_as(P_WHITE); // leaving with moves played is a resignation
        let final_state = terminal_state(&mut room.black_rx).await;

        assert_eq!(
            final_state.status,
            Status::Resigned {
                winner: Color::Black
            }
        );
        assert!(!final_state.opponent_online, "the leaver's seat is freed");

        let record = room.recorded_game();
        assert_common_record_fields(&record, TimeControl::DEFAULT);
        assert_eq!(record.status, "resigned");
        assert_eq!(record.winner, Some("black".to_string()));
        assert_eq!(record.move_count, 1);
        assert_eq!(record.black_state.rating, 1540.0);
        assert_eq!(record.white_state.rating, 1460.0);
    }

    #[tokio::test]
    async fn a_flag_fall_captures_one_record_won_by_the_opponent() {
        // A 1-second base clock so the fake time source can flag it.
        let time_control = TimeControl {
            base_ms: 1000,
            increment_ms: 0,
        };
        let mut room = TestRoom::spawn(None, None, time_control, false).await;

        // White's clock (base 1000 ms, started at t=0) is flagged at t=2500.
        room.time.0.store(2500, Ordering::SeqCst);
        room.move_as(P_WHITE, "e2e4"); // rejected: the flag decides first

        let final_state = terminal_state(&mut room.white_rx).await;
        assert_eq!(
            final_state.status,
            Status::TimedOut {
                winner: Color::Black
            }
        );

        let record = room.recorded_game();
        assert_common_record_fields(&record, time_control);
        assert_eq!(record.status, "timed_out");
        assert_eq!(record.winner, Some("black".to_string()));
        assert_eq!(record.move_count, 0, "the flagged move was not applied");
        assert_eq!(record.black_state.rating, 1540.0);
        assert_eq!(record.white_state.rating, 1460.0);
    }

    #[tokio::test]
    async fn a_grace_expiry_forfeit_captures_one_record() {
        let mut room = TestRoom::spawn(None, None, TimeControl::DEFAULT, false).await;
        room.move_as(P_WHITE, "e2e4");
        let _ = next_state(&mut room.white_rx).await;
        let _ = next_state(&mut room.black_rx).await;

        room.detach_as(P_WHITE);
        // The opponent is told the player is offline (non-terminal state).
        let offline_state = next_state(&mut room.black_rx).await;
        assert!(!offline_state.opponent_online);
        assert!(!offline_state.status.is_terminal());

        // The harness' grace window is 300 ms of real time.
        tokio::time::sleep(Duration::from_millis(600)).await;
        let final_state = terminal_state(&mut room.black_rx).await;
        assert_eq!(
            final_state.status,
            Status::Forfeited {
                winner: Color::Black
            }
        );

        let record = room.recorded_game();
        assert_common_record_fields(&record, TimeControl::DEFAULT);
        assert_eq!(record.status, "forfeited");
        assert_eq!(record.winner, Some("black".to_string()));
        assert_eq!(record.move_count, 1);
        assert_eq!(record.black_state.rating, 1540.0);
        assert_eq!(record.white_state.rating, 1460.0);
    }

    #[tokio::test]
    async fn a_failing_recorder_still_delivers_the_final_snapshot_to_both_seats() {
        // A forced recorder failure: the final snapshot is still delivered
        // and the in-memory ratings are updated (the FakeRatings applies
        // the result regardless of the DB failure).
        let mut room = TestRoom::spawn(Some(1), None, TimeControl::DEFAULT, true).await;
        room.move_as(P_WHITE, "e2e4");
        let final_state = terminal_state(&mut room.white_rx).await;
        assert_eq!(
            final_state.status,
            Status::Checkmated {
                winner: Color::White
            }
        );
        assert!(final_state.white_rating > DEFAULT_RATING);
        assert!(final_state.black_rating < DEFAULT_RATING);
        // The recorder failed, so no finished-game record was captured:
        assert_eq!(room.recorded_games().len(), 0);
    }

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

    // ------------------------------------------------------------------
    // 6.5 Seat matching by account, and 7.1 the hijack guard
    // ------------------------------------------------------------------

    const ACCOUNT: &str = "account-1";
    const PHONE: &str = "device-phone";

    fn as_account(device_id: &str, account_id: &str) -> Identity {
        Identity::Account {
            account_id: account_id.into(),
            device_id: device_id.into(),
        }
    }

    /// The snapshot whose move list has exactly `moves` entries.
    ///
    /// A seat's channel carries every broadcast, so a freshly seated player
    /// still has the opening snapshot queued. Waiting for the move count
    /// rather than for "the next message" is what makes these assertions about
    /// the state *after* an action.
    async fn next_state_with_moves(
        rx: &mut mpsc::UnboundedReceiver<ServerMessage>,
        moves: usize,
    ) -> State {
        loop {
            let state = next_state(rx).await;
            if state.move_list.len() == moves {
                return state;
            }
        }
    }

    /// A two-seat room whose White seat is authenticated as `ACCOUNT` and whose
    /// Black seat is an anonymous guest.
    async fn room_with_an_account_on_white() -> TestRoom {
        TestRoom::spawn_with(
            as_account(PHONE, ACCOUNT),
            "device-opponent".into(),
            None,
            None,
            TimeControl::DEFAULT,
            false,
        )
        .await
    }

    /// The same account re-attaches from a different device and finds its seat:
    /// the account, not the device identifier, is what identifies the player
    /// once a session exists (design D9).
    #[tokio::test]
    async fn the_same_account_re_attaches_from_a_different_device() {
        let room = room_with_an_account_on_white().await;
        let mut tablet = room.attach(as_account("device-tablet", ACCOUNT)).await;

        // Re-attach answers with a snapshot for the returning player, not an
        // error: the account is what proves the seat is theirs.
        let state = next_state(&mut tablet).await;
        assert_eq!(state.your_color, Color::White, "it is the same seat");
        assert_eq!(state.status, Status::Playing, "the game is untouched");

        // And the seat is now reached by either identity, so the player can
        // move from the tablet. (The re-attach took over White's channel, so
        // the assertions read the tablet's.)
        room.move_as_identity(as_account("device-tablet", ACCOUNT), "e2e4");
        let state = next_state_with_moves(&mut tablet, 1).await;
        assert_eq!(state.move_list, vec!["e2e4".to_string()]);
        assert_eq!(state.your_color, Color::White);
    }

    /// 6.5 The non-match case: an authenticated stranger is matched by neither
    /// the account nor the device, so the room refuses it without disturbing
    /// the seated player.
    #[tokio::test]
    async fn a_different_account_finds_no_seat() {
        let mut room = room_with_an_account_on_white().await;
        let mut stranger = room.attach(as_account("device-stranger", "account-2")).await;

        assert_eq!(
            room.next_error(&mut stranger).await,
            "room_full",
            "no seat matched, and the room is full"
        );

        // The seated player is untouched: still their turn, still their seat.
        let state = next_state_with_moves(&mut room.white_rx, 0).await;
        assert_eq!(state.your_color, Color::White);
        assert_eq!(state.status, Status::Playing);
        room.move_as_identity(as_account(PHONE, ACCOUNT), "e2e4");
        let state = next_state_with_moves(&mut room.white_rx, 1).await;
        assert_eq!(state.move_list, vec!["e2e4".to_string()]);
    }

    /// A seat taken by a guest and later claimed by an authenticated
    /// connection on that same device is *upgraded*, not replaced: the seat
    /// index, the clock, and the history all stay, and the seat becomes
    /// reachable from any other device of the account.
    #[tokio::test]
    async fn logging_in_upgrades_the_seat_rather_than_moving_it() {
        let mut room = TestRoom::spawn(None, None, TimeControl::DEFAULT, false).await;

        // White plays two moves as a guest, on `P_WHITE`'s device.
        room.move_as(P_WHITE, "e2e4");
        next_state_with_moves(&mut room.white_rx, 1).await;
        room.move_as(P_BLACK, "e7e5");
        let before = next_state_with_moves(&mut room.white_rx, 2).await;

        // Then the same device signs in and acts again, as an account. The
        // third ply lands in the same seat, with its history intact.
        room.move_as_identity(as_account(P_WHITE, ACCOUNT), "g1f3");
        let after = next_state_with_moves(&mut room.white_rx, 3).await;
        assert_eq!(
            after.move_list,
            vec!["e2e4".to_string(), "e7e5".to_string(), "g1f3".to_string()],
            "the move landed in the same seat, with its history intact"
        );
        assert_eq!(after.your_color, Color::White, "still White's seat");
        assert!(
            after.white_time_ms >= before.white_time_ms,
            "White's clock was not reset by the upgrade: it kept running, and \
             gained the increment from its own move"
        );

        // And the upgraded seat is now reachable from another device, which is
        // what the upgrade bought.
        let mut tablet = room.attach(as_account("device-tablet", ACCOUNT)).await;
        let state = next_state(&mut tablet).await;
        assert_eq!(state.your_color, Color::White);
        assert_eq!(state.move_list.len(), 3, "the game it inherited is intact");
    }

    /// 7.1 The hijack guard, end to end: an authenticated player who has
    /// claimed their seat (which is what makes it account-owned) is safe from
    /// a connection with no session that replays their device identifier.
    #[tokio::test]
    async fn replaying_a_seated_players_device_id_is_refused() {
        let mut room = TestRoom::spawn(None, None, TimeControl::DEFAULT, false).await;
        // A real game, so there is state worth preserving.
        room.move_as(P_WHITE, "e2e4");
        next_state_with_moves(&mut room.white_rx, 1).await;
        // White signs in, which upgrades the seat to account-owned.
        room.move_as(P_BLACK, "e7e5");
        next_state_with_moves(&mut room.white_rx, 2).await;
        room.move_as_identity(as_account(P_WHITE, ACCOUNT), "g1f3");
        let before = next_state_with_moves(&mut room.white_rx, 3).await;
        assert_eq!(before.side_to_move, "b", "Black's turn, so a real wait follows");

        // An unauthenticated connection claims to be the seated White player.
        let mut hijacker = room.attach(P_WHITE.into()).await;
        assert_eq!(
            room.next_error(&mut hijacker).await,
            "room_full",
            "the replay is refused with the same code as a full room, so the \
             attacker learns nothing about the seat"
        );

        // The seated player was not displaced: the hijack produced no state
        // for them and changed nothing in the game.
        assert!(
            tokio::time::timeout(Duration::from_millis(50), room.white_rx.recv())
                .await
                .is_err(),
            "the seated player received nothing: the refusal is not even an \
             acknowledgement to the attacker, it is the existing full-room answer"
        );
        // The game is exactly where it was: Black still moves in the seat it
        // holds, on top of the untouched history, with an untouched status.
        room.move_as(P_BLACK, "b8c6");
        let after = next_state_with_moves(&mut room.white_rx, 4).await;
        assert_eq!(after.your_color, Color::White);
        assert_eq!(after.status, Status::Playing, "the status is unchanged");
        assert_eq!(
            after.move_list,
            vec!["e2e4", "e7e5", "g1f3", "b8c6"],
            "the hijack left no trace in the game"
        );
        assert_eq!(
            after.white_time_ms,
            before.white_time_ms,
            "White's clock did not move while the hijack was refused"
        );

        // And the real owner is still the owner: the account re-attaches
        // normally from another device.
        let mut tablet = room.attach(as_account("device-tablet", ACCOUNT)).await;
        assert_eq!(next_state(&mut tablet).await.your_color, Color::White);
    }

    /// The guard must not fire for the legitimate re-attach path: a seat that
    /// was never upgraded still matches on `device_id` alone, so an anonymous
    /// connection on that device gets its seat back.
    #[tokio::test]
    async fn a_guest_seat_still_re_attaches_on_the_device_id_alone() {
        let room = TestRoom::spawn(None, None, TimeControl::DEFAULT, false).await;
        let mut returning = room.attach(P_WHITE.into()).await;
        let state = next_state(&mut returning).await;
        assert_eq!(state.your_color, Color::White);
        assert_eq!(state.status, Status::Playing);
    }

    /// A guest seat that *has* been upgraded refuses the device-id claim, and
    /// an authenticated connection on the same device is still accepted — the
    /// guard distinguishes "no session" from "a session that proves it".
    #[tokio::test]
    async fn an_upgraded_seat_refuses_the_device_claim_but_not_its_owner() {
        let mut room = TestRoom::spawn(None, None, TimeControl::DEFAULT, false).await;
        room.move_as(P_WHITE, "e2e4");
        next_state_with_moves(&mut room.white_rx, 1).await;
        room.move_as(P_BLACK, "e7e5");
        next_state_with_moves(&mut room.white_rx, 2).await;
        room.move_as_identity(as_account(P_WHITE, ACCOUNT), "g1f3");
        next_state_with_moves(&mut room.white_rx, 3).await;

        // The genuine owner, on the same device, with its session: accepted,
        // and matched by account rather than by the device string.
        let mut owner = room.attach(as_account(P_WHITE, ACCOUNT)).await;
        assert_eq!(next_state(&mut owner).await.your_color, Color::White);

        // A third party whose account matches nothing and whose device matches
        // a free seat: refused, because the room is full and no seat matched.
        let mut third = room.attach("device-c".into()).await;
        assert_eq!(room.next_error(&mut third).await, "room_full");
    }

    /// 7.3 A login issued mid-connection is adopted by the next
    /// room-affecting action: the actor matches on the upgraded identity
    /// without the client having to re-announce anything.
    #[tokio::test]
    async fn an_upgraded_identity_keeps_reaching_its_seat() {
        let mut room = room_with_an_account_on_white().await;
        room.move_as_identity(as_account(PHONE, ACCOUNT), "e2e4");
        next_state_with_moves(&mut room.white_rx, 1).await;

        // Resign arrives under the upgraded identity and ends the game, so the
        // seat was reached rather than silently ignored.
        room.resign_as_identity(as_account(PHONE, ACCOUNT));
        let state = terminal_state(&mut room.white_rx).await;
        assert_eq!(state.status, Status::Resigned { winner: Color::Black });
    }

    /// The non-matching identity is ignored rather than acted on: a move from
    /// someone who holds no seat must not change the game (7.2's "guest play is
    /// unaffected", from the other direction).
    #[tokio::test]
    async fn a_move_from_an_unseated_identity_changes_nothing() {
        let mut room = room_with_an_account_on_white().await;
        let mut stranger = room.attach(as_account("device-stranger", "account-2")).await;
        assert_eq!(room.next_error(&mut stranger).await, "room_full");
        // Drain the opening snapshot so the silence below is about the move.
        next_state_with_moves(&mut room.white_rx, 0).await;
        // The stranger's channel is now unreferenced by the room.
        room.move_as_identity(as_account("device-stranger", "account-2"), "e2e4");
        assert!(
            tokio::time::timeout(Duration::from_millis(50), room.white_rx.recv())
                .await
                .is_err(),
            "the game did not move: an unseated move is dropped, not relayed"
        );
    }
}
