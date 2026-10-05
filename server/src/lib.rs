//! PlainTextChess online server: authoritative two-player rooms over a
//! versioned JSON WebSocket protocol, with in-memory reconnect grace and
//! per-device Glicko2 ratings (see `openspec/changes/add-online-multiplayer`
//! and `openspec/changes/add-online-time-controls`).
//!
//! Clean-architecture layout (design D1 of add-online-time-controls):
//! `domain` (pure rules) ← `application` (room-actor use cases + ports) ←
//! `infrastructure` (axum/tokio wiring, engine, ratings, config) and
//! `interface` (the wire protocol). Dependencies point inward only; nothing
//! in `domain/` imports `tokio`, `axum`, `serde`, or `chess_core`.

pub mod application;
pub mod domain;
pub mod infrastructure;
pub mod interface;
