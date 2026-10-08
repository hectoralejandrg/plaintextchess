//! Domain layer (design D1): the pure rules of online play. Nothing in this
//! module imports `tokio`, `axum`, `serde`, or `chess_core`, so the rules
//! are unit-testable in isolation.

pub mod account;
pub mod clock;
pub mod game_record;
pub mod rating;
pub mod room;
pub mod time_control;
