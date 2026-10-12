//! Tournament actor (add-tournaments phase 1): one task per Arena tournament,
//! mirroring `room_actor.rs`'s one-task-per-room shape. Owns the tournament's
//! live state (waiting players, standings) in memory; the recorder is a
//! durability side effect, not the source of truth while the actor runs
//! (same split as `Room`/`GameRecorder`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;
use tokio::time::Duration;

use crate::application::ports::TournamentRecorder;
use crate::domain::account::Identity;
use crate::domain::time_control::TimeControl;
use crate::domain::tournament::{pair_waiting_players, Standing, Tournament, TournamentStatus};
use crate::infrastructure::server::{App, Conn};

/// How often the actor retries pairing / checks the end time, as a catch-all
/// alongside the event-driven attempts on join/game-finish.
const TICK_INTERVAL: Duration = Duration::from_secs(2);

pub enum TournamentMsg {
    Join {
        identity: Identity,
        paired_tx: mpsc::UnboundedSender<Conn>,
    },
    Leave {
        player_id: String,
    },
    /// A tournament game ended; `*_points` are already doubled (win=2,
    /// draw=1, loss=0 — `domain::tournament::points_for`).
    GameFinished {
        white: String,
        black: String,
        white_points: u32,
        black_points: u32,
    },
}

/// What a tournament game's `RoomServices` carries so its terminal result
/// reaches the tournament instead of the Glicko-2 rating path.
#[derive(Clone)]
pub struct TournamentHooks {
    pub tournament_id: String,
    pub mailbox: mpsc::UnboundedSender<TournamentMsg>,
    pub recorder: Arc<dyn TournamentRecorder>,
}

/// What `App` keeps per active tournament: the mailbox to route messages to,
/// a shared status flag so a lookup (e.g. `JoinTournament`) doesn't need a
/// round trip through the actor, and the static info a listing shows.
pub struct TournamentHandle {
    pub mailbox: mpsc::UnboundedSender<TournamentMsg>,
    pub status: Arc<Mutex<TournamentStatus>>,
    pub time_control: TimeControl,
    pub starts_at_ms: i64,
    pub duration_ms: i64,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// One task per tournament. Uses real wall-clock time (matches
/// `starts_at_ms`/`duration_ms`, a calendar window), not the room actor's
/// monotonic `TimeSource` (a per-room relative clock) — the two are
/// unrelated concepts and deliberately not shared.
pub async fn run_tournament(
    self_tx: mpsc::UnboundedSender<TournamentMsg>,
    mut mailbox: mpsc::UnboundedReceiver<TournamentMsg>,
    app: Arc<App>,
    mut tournament: Tournament,
    status: Arc<Mutex<TournamentStatus>>,
    recorder: Arc<dyn TournamentRecorder>,
) {
    let mut standings: HashMap<String, Standing> = HashMap::new();
    let mut identities: HashMap<String, Identity> = HashMap::new();
    let mut waiting: HashMap<String, mpsc::UnboundedSender<Conn>> = HashMap::new();
    let mut in_game: HashMap<String, mpsc::UnboundedSender<Conn>> = HashMap::new();
    let mut last_opponent: HashMap<String, String> = HashMap::new();

    // Persisted before accepting any message: `tournament_standings`/
    // `tournament_games` reference this row by id, so it must exist before
    // the first `GameFinished` can be recorded.
    if let Err(err) = recorder
        .record_tournament_created(
            &tournament.id,
            &tournament.time_control.label(),
            tournament.starts_at_ms,
            tournament.duration_ms,
        )
        .await
    {
        tracing::error!(error = %err, tournament_id = %tournament.id, "failed to persist the new tournament");
    }

    tournament.status = TournamentStatus::Running;
    *status.lock().unwrap() = TournamentStatus::Running;

    let mut interval = tokio::time::interval(TICK_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            msg = mailbox.recv() => match msg {
                Some(TournamentMsg::Join { identity, paired_tx }) => {
                    if tournament.status != TournamentStatus::Running {
                        continue;
                    }
                    let player_id = identity.device_id().to_string();
                    identities.insert(player_id.clone(), identity);
                    standings.entry(player_id.clone()).or_insert_with(|| Standing {
                        player_id: player_id.clone(),
                        points: 0,
                        games_played: 0,
                    });
                    waiting.insert(player_id, paired_tx);
                    try_pair(&app, &tournament, &self_tx, &recorder, &standings, &mut waiting, &mut in_game, &last_opponent, &identities).await;
                }
                Some(TournamentMsg::Leave { player_id }) => {
                    waiting.remove(&player_id);
                }
                Some(TournamentMsg::GameFinished { white, black, white_points, black_points }) => {
                    for (player_id, points) in [(&white, white_points), (&black, black_points)] {
                        let standing = standings.entry(player_id.clone()).or_insert_with(|| Standing {
                            player_id: player_id.clone(),
                            points: 0,
                            games_played: 0,
                        });
                        standing.points += points;
                        standing.games_played += 1;
                        if let Err(err) = recorder.record_standing_update(&tournament.id, player_id, points).await {
                            tracing::error!(error = %err, tournament_id = %tournament.id, %player_id, "failed to persist tournament standing");
                        }
                    }
                    last_opponent.insert(white.clone(), black.clone());
                    last_opponent.insert(black.clone(), white.clone());
                    // Back into the waiting pool for the next pairing, unless
                    // they left in the meantime or the tournament already ended.
                    if tournament.status == TournamentStatus::Running {
                        if let Some(tx) = in_game.remove(&white) {
                            waiting.insert(white, tx);
                        }
                        if let Some(tx) = in_game.remove(&black) {
                            waiting.insert(black, tx);
                        }
                    }
                    try_pair(&app, &tournament, &self_tx, &recorder, &standings, &mut waiting, &mut in_game, &last_opponent, &identities).await;
                }
                None => break, // every handle dropped: nothing left to manage
            },
            _ = interval.tick() => {
                if tournament.is_over(now_ms()) {
                    tournament.status = TournamentStatus::Finished;
                    *status.lock().unwrap() = TournamentStatus::Finished;
                    break;
                }
                try_pair(&app, &tournament, &self_tx, &recorder, &standings, &mut waiting, &mut in_game, &last_opponent, &identities).await;
            }
        }
    }
}

/// Attempt to pair every currently-waiting player, creating a real room per
/// pair via the same `App::create_tournament_room`/`join_room` paths, and
/// pushing each side's `Conn` down the channel it registered at `Join`
/// (`handle_socket`'s pairing-push arm installs it).
#[allow(clippy::too_many_arguments)]
async fn try_pair(
    app: &Arc<App>,
    tournament: &Tournament,
    self_tx: &mpsc::UnboundedSender<TournamentMsg>,
    recorder: &Arc<dyn TournamentRecorder>,
    standings: &HashMap<String, Standing>,
    waiting: &mut HashMap<String, mpsc::UnboundedSender<Conn>>,
    in_game: &mut HashMap<String, mpsc::UnboundedSender<Conn>>,
    last_opponent: &HashMap<String, String>,
    identities: &HashMap<String, Identity>,
) {
    let standings_vec: Vec<Standing> = standings.values().cloned().collect();
    let waiting_ids: Vec<String> = waiting.keys().cloned().collect();
    let pairs = pair_waiting_players(&standings_vec, &waiting_ids, |p| last_opponent.get(p).cloned());

    for (a, b) in pairs {
        let (Some(identity_a), Some(identity_b)) = (identities.get(&a), identities.get(&b)) else {
            continue;
        };
        let hooks = TournamentHooks {
            tournament_id: tournament.id.clone(),
            mailbox: self_tx.clone(),
            recorder: Arc::clone(recorder),
        };
        let Ok(conn_a) = app.create_tournament_room(identity_a.clone(), tournament.time_control, hooks) else {
            continue;
        };
        let Ok(conn_b) = app.join_room(identity_b.clone(), &conn_a.code) else {
            continue;
        };
        if let Some(tx) = waiting.remove(&a) {
            let _ = tx.send(conn_a);
            in_game.insert(a, tx);
        }
        if let Some(tx) = waiting.remove(&b) {
            let _ = tx.send(conn_b);
            in_game.insert(b, tx);
        }
    }
}
