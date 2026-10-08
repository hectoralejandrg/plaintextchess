//! Infrastructure layer (design D1): concrete implementations of the
//! application's ports and the axum/tokio wiring.

pub mod auth;
pub mod auth_recorder;
pub mod config;
pub mod engine;
pub mod password;
pub mod ratings;
pub mod recorder;
pub mod server;
pub mod ws;
