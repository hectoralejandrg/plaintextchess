//! Application state shared by the HTTP/WebSocket handlers (design D1,
//! infrastructure layer): the room registry (joinable rooms by code), the
//! one-room-per-device index, and the in-memory rating store.

#![allow(clippy::result_large_err)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use tokio::sync::mpsc;

use crate::application::ports::{ChessEngine, Ratings, RoomRegistry, RoomServices, TimeSource};
use crate::application::room_actor::{run_room, RoomMsg};
use crate::domain::room::{CODE_ALPHABET, CODE_LEN, is_valid_code};
use crate::domain::time_control::TimeControl;
use crate::infrastructure::config::Config;
use crate::infrastructure::engine::CoreEngine;
use crate::infrastructure::ratings::RatingStore;
use crate::interface::protocol::{ErrorCode, ServerMessage};

/// A live connection bound to a room: the mailbox to send the room's
/// decisions from, and the room's outbound messages to the client.
pub struct Conn {
    pub player_id: String,
    pub code: String,
    pub mailbox: mpsc::UnboundedSender<RoomMsg>,
    pub out_rx: mpsc::UnboundedReceiver<ServerMessage>,
}

/// Monotonic wall time in milliseconds (design D2/D9): the production
/// `TimeSource`. Only differences between marks are meaningful.
pub struct SystemTimeSource {
    origin: Instant,
}

impl SystemTimeSource {
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl TimeSource for SystemTimeSource {
    fn now_ms(&self) -> u64 {
        Instant::now().duration_since(self.origin).as_millis() as u64
    }
}

impl Default for SystemTimeSource {
    fn default() -> Self {
        Self::new()
    }
}

pub struct App {
    config: Config,
    /// Joinable rooms, by code. Terminal rooms stay listed until their
    /// players disconnect; forfeited rooms are removed immediately.
    rooms: Mutex<HashMap<String, mpsc::UnboundedSender<RoomMsg>>>,
    /// One room per device: player_id -> room code.
    players: Mutex<HashMap<String, String>>,
    ratings: Arc<RatingStore>,
    engine: Arc<dyn ChessEngine>,
    time: Arc<dyn TimeSource>,
}

impl RoomRegistry for App {
    fn remove_room(&self, code: &str) {
        self.rooms.lock().unwrap().remove(code);
    }

    /// Forget a device, but only if it is still bound to this room (so a
    /// late rejection never unbinds the device from a newer room).
    fn untrack_player_if(&self, player_id: &str, code: &str) {
        let mut players = self.players.lock().unwrap();
        if players.get(player_id).is_some_and(|c| c == code) {
            players.remove(player_id);
        }
    }
}

impl App {
    pub fn new(config: Config) -> Self {
        let engine: Arc<dyn ChessEngine> = Arc::new(CoreEngine);
        let ratings = Arc::new(RatingStore::new(Arc::clone(&engine)));
        let time: Arc<dyn TimeSource> = Arc::new(SystemTimeSource::new());
        Self {
            config,
            rooms: Mutex::new(HashMap::new()),
            players: Mutex::new(HashMap::new()),
            ratings,
            engine,
            time,
        }
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn ratings(&self) -> &RatingStore {
        &self.ratings
    }

    pub fn room_codes(&self) -> Vec<String> {
        self.rooms.lock().unwrap().keys().cloned().collect()
    }

    pub fn contains_room(&self, code: &str) -> bool {
        self.rooms.lock().unwrap().contains_key(code)
    }

    fn track_player(&self, player_id: &str, code: &str) {
        self.players
            .lock()
            .unwrap()
            .insert(player_id.to_string(), code.to_string());
    }

    pub fn is_valid_code(code: &str) -> bool {
        is_valid_code(code)
    }

    /// The ports the room actor needs (design D1): this app owns the
    /// registry, and it shares the ratings, the engine, and the time source
    /// with the room it spawns.
    fn services(&self, this: &Arc<Self>) -> RoomServices {
        RoomServices {
            registry: Arc::clone(this) as Arc<dyn RoomRegistry>,
            ratings: Arc::clone(&self.ratings) as Arc<dyn Ratings>,
            engine: Arc::clone(&self.engine),
            time: Arc::clone(&self.time),
            reconnect_grace: self.config.reconnect_grace,
        }
    }

    /// Create a room with the default time control (spec "Server Room
    /// Management"): unique code, the creator seated White, the room actor
    /// spawned.
    pub fn create_room(self: &Arc<Self>, player_id: &str) -> Result<Conn, ServerMessage> {
        self.create_room_with_time_control(player_id, TimeControl::DEFAULT)
    }

    /// Create a room with an explicit time control (spec "Creating a room
    /// chooses its time control"), used by the wire path and by tests that
    /// need short clocks.
    pub fn create_room_with_time_control(
        self: &Arc<Self>,
        player_id: &str,
        time_control: TimeControl,
    ) -> Result<Conn, ServerMessage> {
        {
            let players = self.players.lock().unwrap();
            if players.contains_key(player_id) {
                return Err(ServerMessage::error(ErrorCode::AlreadyInRoom));
            }
        }
        let code = self.unique_code();
        let (mailbox_tx, mailbox_rx) = mpsc::unbounded_channel();
        let (out_tx, out_rx) = mpsc::unbounded_channel();

        let app = Arc::clone(self);
        let actor_code = code.clone();
        let services = self.services(&app);
        tokio::spawn(async move {
            run_room(mailbox_rx, services, actor_code, time_control).await;
        });

        self.rooms
            .lock()
            .unwrap()
            .insert(code.clone(), mailbox_tx.clone());
        self.track_player(player_id, &code);
        let _ = mailbox_tx.send(RoomMsg::Connect {
            player_id: player_id.to_string(),
            code: None,
            out: out_tx,
        });

        Ok(Conn {
            player_id: player_id.to_string(),
            code,
            mailbox: mailbox_tx,
            out_rx,
        })
    }

    /// Join (or re-attach to) a room by code. Format errors, duplicate
    /// devices, and unknown rooms are rejected before the room actor is
    /// involved; the actor rejects full or finished rooms.
    pub fn join_room(
        self: &Arc<Self>,
        player_id: &str,
        raw_code: &str,
    ) -> Result<Conn, ServerMessage> {
        let code = raw_code.to_uppercase();
        if !Self::is_valid_code(&code) {
            return Err(ServerMessage::error(ErrorCode::InvalidRoomCode));
        }
        {
            let players = self.players.lock().unwrap();
            match players.get(player_id) {
                Some(current) if current != &code => {
                    return Err(ServerMessage::error(ErrorCode::AlreadyInRoom));
                }
                _ => {}
                // A device bound to `code` re-joining it is a reconnect.
            }
        }
        let mailbox = {
            let rooms = self.rooms.lock().unwrap();
            rooms
                .get(&code)
                .cloned()
                .ok_or_else(|| ServerMessage::error(ErrorCode::RoomNotFound))?
        };
        let (out_tx, out_rx) = mpsc::unbounded_channel();
        self.track_player(player_id, &code);
        if mailbox
            .send(RoomMsg::Connect {
                player_id: player_id.to_string(),
                code: Some(code.clone()),
                out: out_tx,
            })
            .is_err()
        {
            // The room actor is gone: it has already dropped the room.
            self.untrack_player_if(player_id, &code);
            return Err(ServerMessage::error(ErrorCode::RoomNotFound));
        }
        Ok(Conn {
            player_id: player_id.to_string(),
            code,
            mailbox,
            out_rx,
        })
    }

    /// Draw a room code that no current room uses.
    fn unique_code(&self) -> String {
        loop {
            let mut code = String::new();
            for _ in 0..CODE_LEN {
                let index = rand::random::<usize>() % CODE_ALPHABET.len();
                code.push(CODE_ALPHABET[index] as char);
            }
            let rooms = self.rooms.lock().unwrap();
            if !rooms.contains_key(&code) {
                return code;
            }
        }
    }
}
