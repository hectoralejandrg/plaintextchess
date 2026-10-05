//! PlainTextChess online server: authoritative two-player rooms over a
//! versioned JSON WebSocket protocol, with in-memory reconnect grace and
//! per-device Glicko2 ratings (see `openspec/changes/add-online-multiplayer`).

pub mod app;
pub mod config;
pub mod protocol;
pub mod rating;
pub mod room;
pub mod ws;
