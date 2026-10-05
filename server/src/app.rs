//! Application state shared by the HTTP/WebSocket handlers: the room
//! registry (joinable rooms by code), the one-room-per-device index, and the
//! in-memory rating store (design D3/D5).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;

use crate::config::Config;
use crate::protocol::{ErrorCode, ServerMessage};
use crate::rating::RatingStore;
use crate::room::{run as run_room, RoomMsg};

/// 32-character unambiguous alphabet: A-Z without I and O, digits 2-9
/// (no 0/1, no I/O) so codes stay readable when typed by hand.
pub const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
pub const CODE_LEN: usize = 6;

/// A live connection bound to a room: the mailbox to send the room's
/// decisions from, and the room's outbound messages to the client.
pub struct Conn {
    pub player_id: String,
    pub code: String,
    pub mailbox: mpsc::UnboundedSender<RoomMsg>,
    pub out_rx: mpsc::UnboundedReceiver<ServerMessage>,
}

pub struct App {
    config: Config,
    /// Joinable rooms, by code. Terminal rooms stay listed until their
    /// players disconnect; forfeited rooms are removed immediately.
    rooms: Mutex<HashMap<String, mpsc::UnboundedSender<RoomMsg>>>,
    /// One room per device: player_id -> room code.
    players: Mutex<HashMap<String, String>>,
    ratings: RatingStore,
}

impl App {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            rooms: Mutex::new(HashMap::new()),
            players: Mutex::new(HashMap::new()),
            ratings: RatingStore::new(),
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

    pub fn remove_room(&self, code: &str) {
        self.rooms.lock().unwrap().remove(code);
    }

    fn track_player(&self, player_id: &str, code: &str) {
        self.players
            .lock()
            .unwrap()
            .insert(player_id.to_string(), code.to_string());
    }

    /// Forget a device, but only if it is still bound to this room (so a
    /// late rejection never unbinds the device from a newer room).
    pub fn untrack_player_if(&self, player_id: &str, code: &str) {
        let mut players = self.players.lock().unwrap();
        if players.get(player_id).is_some_and(|c| c == code) {
            players.remove(player_id);
        }
    }

    pub fn is_valid_code(code: &str) -> bool {
        code.len() == CODE_LEN && code.bytes().all(|b| CODE_ALPHABET.contains(&b))
    }

    /// Create a room: unique code, the creator seated White, and the room
    /// actor spawned.
    pub fn create_room(self: &Arc<Self>, player_id: &str) -> Result<Conn, ServerMessage> {
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
        tokio::spawn(async move {
            run_room(mailbox_rx, app, actor_code).await;
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
