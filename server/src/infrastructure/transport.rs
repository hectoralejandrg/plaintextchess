//! Shared transport primitives: framing, outbound coalescing, keepalive,
//! protocol-close, and reconnect bookkeeping. Used by the transport tier
//! (and the monolith in TIER=all mode).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::extract::ws::{CloseCode, CloseFrame, Message, WebSocket};
use futures_util::stream::SplitSink;
use futures_util::SinkExt;
use tokio::sync::Notify;

use crate::interface::protocol::{ErrorCode, ServerMessage, PROTOCOL_CLOSE_CODE};

/// Global transport metrics (task 1.1: count socket syscalls/WS frames and where
/// the egress time goes: serialization vs the socket write).
static WS_FRAMES_WRITTEN: AtomicU64 = AtomicU64::new(0);
static WS_BYTES_WRITTEN: AtomicU64 = AtomicU64::new(0);
static WS_SOCKET_WRITES: AtomicU64 = AtomicU64::new(0);
static WS_SERIALIZE_NS: AtomicU64 = AtomicU64::new(0);
static WS_WRITE_NS: AtomicU64 = AtomicU64::new(0);

/// Reset transport metrics.
pub fn reset_transport_metrics() {
    WS_FRAMES_WRITTEN.store(0, Ordering::Relaxed);
    WS_BYTES_WRITTEN.store(0, Ordering::Relaxed);
    WS_SOCKET_WRITES.store(0, Ordering::Relaxed);
    WS_SERIALIZE_NS.store(0, Ordering::Relaxed);
    WS_WRITE_NS.store(0, Ordering::Relaxed);
}

/// Snapshot transport metrics: frames and bytes written, the number of socket
/// writes (one per text frame), and the time spent serializing vs writing.
pub fn transport_metrics() -> serde_json::Value {
    serde_json::json!({
        "ws_frames_written": WS_FRAMES_WRITTEN.load(Ordering::Relaxed),
        "ws_bytes_written": WS_BYTES_WRITTEN.load(Ordering::Relaxed),
        "ws_socket_writes": WS_SOCKET_WRITES.load(Ordering::Relaxed),
        "ws_serialize_ns": WS_SERIALIZE_NS.load(Ordering::Relaxed),
        "ws_write_ns": WS_WRITE_NS.load(Ordering::Relaxed),
    })
}

/// Protocol-level close frame: the server must close only for malformed
/// JSON, unknown message types, or unsupported versions.
pub fn protocol_close() -> Message {
    Message::Close(Some(CloseFrame {
        code: CloseCode::from(PROTOCOL_CLOSE_CODE),
        reason: "protocol error".into(),
    }))
}

/// How long between server keepalive pings: well under the reconnect grace, so
/// a dead peer is noticed before the window elapses.
pub const KEEPALIVE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(20);

/// One item queued for the connection's writer.
#[derive(Debug)]
pub enum Outbound {
    /// A server message (JSON text).
    Text(ServerMessage),
    /// A keepalive ping.
    Ping,
    /// A response to a client ping.
    Pong(Vec<u8>),
    /// A protocol close frame (malformed/unknown frame, 4000).
    Close,
    /// The broker fabric was lost: close the client with an internal-error
    /// frame so the loss is visible rather than a silent zombie socket.
    BrokerUnavailable,
    /// This node lost its liveness lease while still running (a pause longer
    /// than the TTL, or lost lease keys). The authoritative tier may have
    /// presumed the node dead; close the client so it reconnects and re-attaches
    /// instead of being silently orphaned (design D11).
    LeaseLost,
    /// The logic node that owned this connection's room was lost. Close with a
    /// clear terminal signal instead of leaving the socket mute (design D13 /
    /// GAP-P3-D): the room's state lives in that node and is gone.
    RoomEnded,
}

/// Close frame for a lost broker: `1011` internal error, not a client protocol
/// error (design D10).
pub const BROKER_UNAVAILABLE_CLOSE_CODE: u16 = 1011;

/// Close frame for a lost node lease: `1012` service restart, so the client
/// reconnects and re-attaches (design D11).
pub const LEASE_LOST_CLOSE_CODE: u16 = 1012;

/// Close frame for a lost broker.
pub fn broker_unavailable_close() -> Message {
    Message::Close(Some(CloseFrame {
        code: BROKER_UNAVAILABLE_CLOSE_CODE,
        reason: "broker unavailable".into(),
    }))
}

/// Close frame for a lost node lease: tell the client to reconnect.
pub fn lease_lost_close() -> Message {
    Message::Close(Some(CloseFrame {
        code: LEASE_LOST_CLOSE_CODE,
        reason: "node lease lost; reconnect".into(),
    }))
}

/// Close code for a room whose logic owner was lost: `4001` (application range),
/// distinct from the protocol error (`4000`) and the broker/lease codes.
pub const ROOM_ENDED_CLOSE_CODE: u16 = 4001;

/// Close frame for a room whose owning logic node was lost: a clear terminal
/// signal, so the players are not left mute (GAP-P3-D).
pub fn room_ended_close() -> Message {
    Message::Close(Some(CloseFrame {
        code: ROOM_ENDED_CLOSE_CODE,
        reason: "room ended: its logic node was lost".into(),
    }))
}

/// Stable shard index for a key (FNV-1a hash). Shared by the transport and
/// logic tiers so a sharded map (connection affinity, outbound batcher,
/// per-connection outbox registry, ...) never serializes on one lock as the
/// connection count grows.
pub fn shard_index(key: &str, shard_count: usize) -> usize {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in key.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    (hash as usize) % shard_count
}

/// Whether a frame is a state-class snapshot/update: these coalesce so a slow
/// client always converges to the newest authoritative state, while one-shot
/// control frames are never dropped.
pub fn is_state_class(message: &ServerMessage) -> bool {
    matches!(
        message,
        ServerMessage::RoomReady { .. } | ServerMessage::State { .. } | ServerMessage::Update { .. }
    )
}

/// A per-connection outbound buffer with backpressure: at most one coalesced
/// state-class frame plus an ordered queue of control frames, so a client that
/// stops reading cannot grow the server's memory without bound.
#[derive(Default)]
pub struct Outbox {
    inner: Mutex<VecDeque<Outbound>>,
    closed: AtomicBool,
    notify: Notify,
}

impl Outbox {
    pub fn push(&self, item: Outbound) {
        let mut queue = self.inner.lock().unwrap();
        if let Outbound::Text(message) = &item {
            if is_state_class(message) {
                queue.retain(|queued| !matches!(queued, Outbound::Text(m) if is_state_class(m)));
            }
        }
        queue.push_back(item);
        drop(queue);
        self.notify.notify_one();
    }

    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.notify.notify_one();
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    pub fn take_batch(&self) -> Vec<Outbound> {
        let mut queue = self.inner.lock().unwrap();
        queue.drain(..).collect()
    }
}

/// The writer half: drains the outbox and sends keepalive pings, treating a
/// peer that stops responding as gone. Notifies `finished` on the way out so
/// the reader ends the session too.
pub async fn write_loop(
    mut sink: SplitSink<WebSocket, Message>,
    outbox: Arc<Outbox>,
    alive: Arc<AtomicBool>,
    finished: Arc<Notify>,
) {
    let mut ping = tokio::time::interval_at(
        tokio::time::Instant::now() + KEEPALIVE_INTERVAL,
        KEEPALIVE_INTERVAL,
    );
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    async {
        loop {
            let batch = outbox.take_batch();
            if !batch.is_empty() {
                let mut fed = false;
                for item in batch {
                    match item {
                        Outbound::Text(message) => {
                            let Some(text) = encode_text(&message) else {
                                return;
                            };
                            if sink.feed(Message::Text(text.into())).await.is_err() {
                                return;
                            }
                            WS_FRAMES_WRITTEN.fetch_add(1, Ordering::Relaxed);
                            fed = true;
                        }
                        Outbound::Ping => {
                            if sink.send(Message::Ping(Default::default())).await.is_err() {
                                return;
                            }
                            WS_FRAMES_WRITTEN.fetch_add(1, Ordering::Relaxed);
                        }
                        Outbound::Pong(payload) => {
                            if sink.send(Message::Pong(payload.into())).await.is_err() {
                                return;
                            }
                            WS_FRAMES_WRITTEN.fetch_add(1, Ordering::Relaxed);
                        }
                        Outbound::Close => {
                            let _ = sink.send(protocol_close()).await;
                            return;
                        }
                        Outbound::BrokerUnavailable => {
                            let _ = sink.send(broker_unavailable_close()).await;
                            return;
                        }
                        Outbound::LeaseLost => {
                            let _ = sink.send(lease_lost_close()).await;
                            return;
                        }
                        Outbound::RoomEnded => {
                            let _ = sink.send(room_ended_close()).await;
                            return;
                        }
                    }
                }
                // One flush for the whole drained batch (design D2): fewer flush
                // syscalls than one per frame when several frames are ready.
                if fed {
                    let t_write = Instant::now();
                    let flushed = sink.flush().await.is_ok();
                    WS_WRITE_NS.fetch_add(t_write.elapsed().as_nanos() as u64, Ordering::Relaxed);
                    if !flushed {
                        return;
                    }
                    WS_SOCKET_WRITES.fetch_add(1, Ordering::Relaxed);
                }
                continue;
            }
            if outbox.is_closed() {
                return;
            }
            tokio::select! {
                _ = outbox.notify.notified() => {}
                _ = ping.tick() => {
                    // No inbound frame since the last ping: the peer is gone.
                    if !alive.swap(false, Ordering::SeqCst) {
                        return;
                    }
                    let _ = sink.send(Message::Ping(Default::default())).await;
                }
            }
        }
    }
    .await;
    finished.notify_one();
}

/// Serialize and send one server message over the writer half. `false` means
/// the socket is gone.
/// Serialize one server message, counting its bytes and the time spent
/// serializing (design D1). `None` means the message could not be encoded.
fn encode_text(message: &ServerMessage) -> Option<String> {
    let t = Instant::now();
    match serde_json::to_string(message) {
        Ok(text) => {
            WS_SERIALIZE_NS.fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
            WS_BYTES_WRITTEN.fetch_add(text.len() as u64, Ordering::Relaxed);
            Some(text)
        }
        Err(_) => None,
    }
}

/// Serialize and send one server message, flushing immediately. `false` means
/// the socket is gone.
pub async fn send_json(sink: &mut SplitSink<WebSocket, Message>, message: &ServerMessage) -> bool {
    let Some(text) = encode_text(message) else {
        return false;
    };
    let t_write = Instant::now();
    let result = sink.send(Message::Text(text.into())).await.is_ok();
    WS_WRITE_NS.fetch_add(t_write.elapsed().as_nanos() as u64, Ordering::Relaxed);
    if result {
        WS_SOCKET_WRITES.fetch_add(1, Ordering::Relaxed);
    }
    result
}

/// Map an `ErrorCode` to a protocol `ServerMessage::Error`.
pub fn error_message(code: ErrorCode) -> ServerMessage {
    ServerMessage::Error {
        v: crate::interface::protocol::VERSION,
        code: code.as_str().to_string(),
        message: code.message().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interface::protocol::Status;

    /// The transport metrics expose the egress breakdown and reset cleanly.
    #[test]
    fn egress_metrics_shape_and_reset() {
        reset_transport_metrics();
        let m = transport_metrics();
        for key in [
            "ws_frames_written",
            "ws_bytes_written",
            "ws_socket_writes",
            "ws_serialize_ns",
            "ws_write_ns",
        ] {
            assert_eq!(m.get(key).and_then(|v| v.as_u64()), Some(0), "{key} should start at 0");
        }
        WS_SOCKET_WRITES.fetch_add(3, Ordering::Relaxed);
        WS_SERIALIZE_NS.fetch_add(120, Ordering::Relaxed);
        WS_WRITE_NS.fetch_add(340, Ordering::Relaxed);
        let m = transport_metrics();
        assert_eq!(m["ws_socket_writes"], 3);
        assert_eq!(m["ws_serialize_ns"], 120);
        assert_eq!(m["ws_write_ns"], 340);
        reset_transport_metrics();
        assert_eq!(transport_metrics()["ws_socket_writes"], 0);
    }

    /// `encode_text` counts the serialized bytes and time for a frame.
    #[test]
    fn encode_text_counts_bytes() {
        reset_transport_metrics();
        let msg = update("e2e4");
        let text = encode_text(&msg).expect("encodes");
        assert!(!text.is_empty());
        assert_eq!(transport_metrics()["ws_bytes_written"], text.len() as u64);
    }

    fn update(uci: &str) -> ServerMessage {
        ServerMessage::Update {
            v: crate::interface::protocol::VERSION,
            uci: Some(uci.to_string()),
            side_to_move: "b".into(),
            status: Status::Playing,
            white_rating: 1500.0,
            black_rating: 1500.0,
            opponent_online: true,
            white_time_ms: 1_000,
            black_time_ms: 1_000,
        }
    }

    fn a_state() -> ServerMessage {
        ServerMessage::State {
            v: crate::interface::protocol::VERSION,
            state: crate::interface::protocol::State {
                board_fen: "x".into(),
                move_list: vec![],
                side_to_move: "w".into(),
                status: Status::Playing,
                your_color: crate::interface::protocol::Color::White,
                white_rating: 1500.0,
                black_rating: 1500.0,
                opponent_online: true,
                time_control: "1+0".into(),
                white_time_ms: 60_000,
                black_time_ms: 60_000,
            },
        }
    }

    /// A slow client that never drains cannot grow memory: the pending
    /// state-class frames collapse to the newest one.
    #[test]
    fn state_frames_coalesce_to_the_newest_with_bounded_memory() {
        let outbox = Outbox::default();
        for i in 0..1_000 {
            outbox.push(Outbound::Text(update(&format!("m{i}"))));
        }
        let batch = outbox.take_batch();
        assert_eq!(batch.len(), 1, "state frames coalesce to a single slot");
        match &batch[0] {
            Outbound::Text(ServerMessage::Update { uci, .. }) => {
                assert_eq!(uci.as_deref(), Some("m999"), "the newest update wins");
            }
            other => panic!("expected the newest update, got {other:?}"),
        }
    }

    /// One-shot control frames are never dropped, even when surrounded by
    /// coalesced state frames.
    #[test]
    fn control_frames_are_never_dropped_and_keep_their_order() {
        let outbox = Outbox::default();
        outbox.push(Outbound::Text(error_message(ErrorCode::NotYourTurn)));
        outbox.push(Outbound::Text(a_state()));
        outbox.push(Outbound::Text(update("a")));
        outbox.push(Outbound::Text(error_message(ErrorCode::IllegalMove)));
        outbox.push(Outbound::Text(update("b")));

        let batch = outbox.take_batch();
        // The two control frames survive in order; only the newest state-class
        // frame (the "b" update) remains.
        assert_eq!(batch.len(), 3, "two control frames plus the newest state");
        assert!(matches!(
            &batch[0],
            Outbound::Text(ServerMessage::Error { .. })
        ));
        assert!(matches!(
            &batch[1],
            Outbound::Text(ServerMessage::Error { .. })
        ));
        match &batch[2] {
            Outbound::Text(ServerMessage::Update { uci, .. }) => {
                assert_eq!(uci.as_deref(), Some("b"), "the newest state-class frame is last");
            }
            other => panic!("expected the newest update last, got {other:?}"),
        }
    }
}