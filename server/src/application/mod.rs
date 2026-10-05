//! Application layer (design D1): the room-actor use cases and the ports
//! they depend on. The room actor is the single place where a room's state
//! mutates, so seat, turn, and timer logic is free of races. The actor's
//! event loop uses tokio for scheduling; the rules it applies live in the
//! domain layer and are pure.

pub mod ports;
pub mod room_actor;
