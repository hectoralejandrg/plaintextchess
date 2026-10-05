//! Infrastructure layer (design D1): concrete implementations of the
//! application's ports and the axum/tokio wiring.

pub mod config;
pub mod engine;
pub mod ratings;
pub mod server;
pub mod ws;
