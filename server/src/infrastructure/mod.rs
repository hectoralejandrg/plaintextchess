//! Infrastructure layer (design D1): concrete implementations of the
//! application's ports and the axum/tokio wiring.

pub mod auth;
pub mod auth_recorder;
pub mod breach;
pub mod broker;
pub mod config;
pub mod email;
pub mod engine;
pub mod net;
pub mod rate_limit;
pub mod logic_handler;
pub mod password;
pub mod ratings;
pub mod recorder;
pub mod server;
pub mod transport;
pub mod transport_handler;
pub mod ws;
