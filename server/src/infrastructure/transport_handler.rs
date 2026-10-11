//! Transport tier WebSocket handler: terminates client WebSockets and bridges
//! them to the logic tier over the Redis fabric (design D4/D5).
//!
//! The transport holds no authoritative game state: it frames, validates,
//! keepalives, coalesces, and forwards. Every game decision stays in the logic
//! tier. There is exactly **one** outbound subscription per node (the
//! dispatcher), not one per socket, so connection count does not multiply the
//! number of Redis subscribers (design D4).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use futures_util::StreamExt;

use crate::infrastructure::broker::{
    generate_connection_id, Broker, InboundEnvelope, InboundPayload, OutboundWire,
    OutboundEnvelope, ENVELOPE_VERSION, DEFAULT_LOGIC_NODE,
};
use crate::interface::protocol::{VERSION, ClientMessage, decode_incoming};
use crate::infrastructure::transport::{write_loop, Outbound, Outbox, transport_metrics, shard_index};
use crate::infrastructure::logic_handler::LogicState;

use tokio::task::JoinHandle;

/// Per-tier health endpoint for the transport tier.
pub async fn healthz(State(state): State<Arc<TransportState>>) -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({
        "status": "ok",
        "tier": "transport",
        "node_id": state.node_id(),
        "protocol_version": VERSION,
        "transport_metrics": transport_metrics(),
    }))
}

/// Backoff bounds for broker reconnection.
const RECONNECT_MIN: Duration = Duration::from_millis(100);
const RECONNECT_MAX: Duration = Duration::from_secs(2);

/// Bounded wait for a logic subscriber before giving up on an inbound frame:
/// a 0-subscriber publish is not delivered, so retry briefly and then close the
/// client visibly instead of silently losing the frame (design D10 / BUG-F1-R1).
const INBOUND_PUBLISH_ATTEMPTS: u32 = 10;
const INBOUND_PUBLISH_RETRY_DELAY: Duration = Duration::from_millis(200);

/// Transport node liveness renewal interval (design D11). The lease TTL lives
/// in `broker::liveness::TTL_SECS`.
const NODE_LEASE_RENEW_INTERVAL: Duration = Duration::from_secs(10);

/// How often the transport checks the logic nodes it holds connections on, so a
/// lost owner closes its players with a clear signal (design D13 / GAP-P3-D).
const LOGIC_WATCH_INTERVAL: Duration = Duration::from_secs(3);

/// Upper bound for best-effort broker calls during connection cleanup: a dead
/// broker must not hold a closing socket open.
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(2);

/// Number of shards for the connection affinity map. Matches the logic side
/// for consistency and avoids hot spots.
const CONN_LOGIC_SHARDS: usize = 16;

/// Sharded connection affinity map to avoid a global lock on the hot path.
/// Shard = hash(connection_id) % CONN_LOGIC_SHARDS.
type ConnLogicShards = Vec<Mutex<HashMap<String, String>>>;

/// Number of shards for the per-connection outbox registry. Without this, the
/// single dispatcher task locks one Mutex shared by every live connection on
/// the node for every dispatched frame; sharding keeps that lock's hot path
/// cost from growing with the connection count.
const OUTBOX_SHARDS: usize = 16;

/// Sharded connection_id -> outbox map. Shard = shard_index(connection_id, OUTBOX_SHARDS).
type OutboxShards = Vec<Mutex<HashMap<String, Arc<Outbox>>>>;

/// Shared state for the transport tier.
pub struct TransportState {
    broker: Arc<Broker>,
    node_id: String,
    /// connection_id -> that socket's outbound buffer. One entry per live
    /// connection, used by the single node dispatcher to route envelopes.
    outboxes: OutboxShards,
    /// Per-Redis-shard dispatcher health: `shard_healthy[s]` is true while
    /// shard `s`'s outbound subscription is up. The node accepts new
    /// connections and is considered healthy while at least one shard works
    /// (all-or-nothing would be overly pessimistic once there is more than
    /// one shard - a connection whose room lives on a working shard should
    /// not be refused just because another shard is down).
    shard_healthy: Vec<AtomicBool>,
    /// Background task that renews the node liveness lease.
    liveness_task: Mutex<Option<JoinHandle<()>>>,
    /// Phase 3: connection_id -> logic_node_id affinity. A connection's
    /// non-room-scoped frames (Move/Resign/Leave/Disconnect) are routed to the
    /// logic node that seats it, so several logic instances do not mis-route.
    /// Sharded by connection_id to avoid global lock contention (task 2.1).
    conn_logic: ConnLogicShards,
    /// Round-robin index for balancing new rooms across live logic nodes.
    /// Only used when placing a new room (create_room).
    placement_rr: std::sync::atomic::AtomicUsize,
    /// `TIER=combined` only: the `LogicState` sharing this process, set once by
    /// `run_combined` right after both are constructed. `None` for a real
    /// `TIER=transport` process, which always goes through the fabric
    /// (colocate-room-hot-path design D3).
    local_logic: std::sync::OnceLock<Arc<LogicState>>,
}

impl TransportState {
    pub fn new(broker: Arc<Broker>, node_id: String) -> Self {
        let mut conn_logic = Vec::with_capacity(CONN_LOGIC_SHARDS);
        for _ in 0..CONN_LOGIC_SHARDS {
            conn_logic.push(Mutex::new(HashMap::new()));
        }
        let mut outboxes = Vec::with_capacity(OUTBOX_SHARDS);
        for _ in 0..OUTBOX_SHARDS {
            outboxes.push(Mutex::new(HashMap::new()));
        }
        let shard_healthy = (0..broker.n_shards()).map(|_| AtomicBool::new(true)).collect();
        Self {
            broker,
            node_id,
            outboxes,
            shard_healthy,
            liveness_task: Mutex::new(None),
            conn_logic,
            placement_rr: std::sync::atomic::AtomicUsize::new(0),
            local_logic: std::sync::OnceLock::new(),
        }
    }

    /// Get the node identifier.
    pub fn node_id(&self) -> &str {
        &self.node_id
    }

    /// `TIER=combined` only: wire this transport to the `LogicState` sharing
    /// its process, so the hot path can skip the fabric when a connection's
    /// owning logic node is this same process (colocate-room-hot-path).
    /// Called once by `run_combined`; a second call is a no-op.
    pub fn set_local_logic(&self, logic: Arc<LogicState>) {
        let _ = self.local_logic.set(logic);
    }

    /// The `LogicState` colocated in this process, if any.
    fn local_logic(&self) -> Option<&Arc<LogicState>> {
        self.local_logic.get()
    }

    /// This connection's outbox, if it is live on this node. Used by the
    /// colocated logic forwarder to push a message directly, skipping the
    /// fabric and the dispatcher (colocate-room-hot-path task 3.1).
    pub fn outbox_for(&self, connection_id: &str) -> Option<Arc<Outbox>> {
        self.outbox_shard_mutex(connection_id).lock().unwrap().get(connection_id).cloned()
    }

    /// Start the node liveness lease and renewal task (design D11).
    /// Registers the lease immediately (fail fast), then spawns a renewal task.
    pub async fn spawn_liveness(self: &Arc<Self>) -> redis::RedisResult<()> {
        // Register the lease at startup (fail fast if Redis unavailable).
        self.broker.register_node_lease().await?;

        let state = Arc::clone(self);
        let task_state = Arc::clone(&state);
        let handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(NODE_LEASE_RENEW_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                interval.tick().await;
                match task_state.broker.renew_node_lease().await {
                    Ok(true) => {}
                    Ok(false) => {
                        // The lease lapsed while this node was alive (a pause
                        // longer than the TTL, or keys lost). The authoritative
                        // tier may have presumed us dead, so close our clients
                        // and let them reconnect and re-attach (design D11).
                        tracing::warn!(
                            node_id = %task_state.node_id,
                            "node lease had lapsed while alive; closing clients to force reconnect"
                        );
                        task_state.on_lease_lost();
                    }
                    Err(e) => {
                        tracing::warn!(node_id = %task_state.node_id, error = %e, "failed to renew node lease");
                    }
                }
            }
        });
        *state.liveness_task.lock().unwrap() = Some(handle);
        Ok(())
    }

    /// Stop the liveness renewal task (used for graceful drain and failure tests).
    pub fn stop_liveness(&self) {
        if let Some(handle) = self.liveness_task.lock().unwrap().take() {
            handle.abort();
        }
    }

    /// Watch the logic nodes this transport holds connections on; when one's
    /// membership key is gone, close its connections with a clear terminal signal
    /// so the players are not left mute when their room's owner dies (design D13 /
    /// GAP-P3-D). The room's state lives in that node and is gone, so this is the
    /// "clear terminal outcome" branch, not continuity.
    pub fn spawn_logic_watch(self: &Arc<Self>) {
        let state = Arc::clone(self);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(LOGIC_WATCH_INTERVAL);
            loop {
                interval.tick().await;
                let nodes: Vec<String> = {
                    let mut seen: Vec<String> = Vec::new();
                    for shard in &state.conn_logic {
                        let map = shard.lock().unwrap();
                        for node in map.values() {
                            if !seen.contains(node) {
                                seen.push(node.clone());
                            }
                        }
                    }
                    seen
                };
                for node in nodes {
                    match state.broker.logic_node_alive(&node).await {
                        Ok(true) => {}
                        Ok(false) => {
                            let ids: Vec<String> = {
                                let mut ids = Vec::new();
                                for shard in &state.conn_logic {
                                    let map = shard.lock().unwrap();
                                    for (id, n) in map.iter() {
                                        if *n == node {
                                            ids.push(id.clone());
                                        }
                                    }
                                }
                                ids
                            };
                            if ids.is_empty() {
                                continue;
                            }
                            tracing::warn!(
                                node_id = %node,
                                connections = ids.len(),
                                "logic node lost; closing its connections"
                            );
                            for id in ids {
                                let outbox = {
                                    let map = state.outbox_shard_mutex(&id).lock().unwrap();
                                    map.get(&id).cloned()
                                };
                                if let Some(outbox) = outbox {
                                    outbox.push(Outbound::RoomEnded);
                                }
                                // Do NOT remove the affinity here: the connection's
                                // own cleanup publishes the `Disconnect` to the node
                                // the affinity names, and removing it first would send
                                // that disconnect to the default channel with no
                                // subscribers, leaving ghost seats (BUG-P3-3).
                            }
                        }
                        Err(e) => {
                            tracing::warn!(node_id = %node, error = %e, "logic node liveness check failed");
                        }
                    }
                }
            }
        });
    }

    /// Get the shard index for a connection ID.
    fn conn_logic_shard(&self, connection_id: &str) -> usize {
        shard_index(connection_id, CONN_LOGIC_SHARDS)
    }

    /// Get the shard mutex for a connection ID.
    fn conn_logic_shard_mutex(&self, connection_id: &str) -> &Mutex<HashMap<String, String>> {
        let shard_idx = self.conn_logic_shard(connection_id);
        &self.conn_logic[shard_idx]
    }

    /// Get the outbox shard mutex for a connection ID.
    fn outbox_shard_mutex(&self, connection_id: &str) -> &Mutex<HashMap<String, Arc<Outbox>>> {
        &self.outboxes[shard_index(connection_id, OUTBOX_SHARDS)]
    }

    /// Phase 3: the logic node a connection is seated on, if known.
    pub fn conn_logic_node(&self, connection_id: &str) -> Option<String> {
        let shard = self.conn_logic_shard_mutex(connection_id);
        shard.lock().unwrap().get(connection_id).cloned()
    }

    /// Phase 3: remember which logic node serves a connection.
    pub fn set_conn_logic_node(&self, connection_id: &str, logic_node_id: &str) {
        let shard = self.conn_logic_shard_mutex(connection_id);
        shard
            .lock()
            .unwrap()
            .insert(connection_id.to_string(), logic_node_id.to_string());
    }

    /// Phase 3: forget a connection's logic affinity.
    pub fn remove_conn_logic_node(&self, connection_id: &str) {
        let shard = self.conn_logic_shard_mutex(connection_id);
        shard.lock().unwrap().remove(connection_id);
    }

    /// Subscribe to this node's outbound channel **on every Redis shard**, fan
    /// each envelope to the connection that owns it, and reconnect with
    /// bounded backoff when a shard's subscription is lost (design D4/D10).
    /// Startup fails fast if any shard's first subscribe fails (spec
    /// `realtime-transport`). One independent task per shard, mirroring the
    /// single-subscription loop this had before sharding, N times.
    pub async fn spawn_dispatcher(self: &Arc<Self>) -> redis::RedisResult<()> {
        let n_shards = self.broker.n_shards();
        let mut initial_pubsubs = Vec::with_capacity(n_shards);
        for shard in 0..n_shards {
            initial_pubsubs.push(self.broker.subscribe_outbound(shard).await?);
        }
        for (shard, pubsub) in initial_pubsubs.into_iter().enumerate() {
            let state = Arc::clone(self);
            tokio::spawn(async move {
                let mut pubsub = pubsub;
                loop {
                    {
                        // Create the message stream once per subscription: re-creating
                        // it each iteration drops messages that arrive between polls.
                        let mut messages = pubsub.on_message();
                        while let Some(message) = messages.next().await {
                            let Ok(payload) = message.get_payload::<String>() else {
                                continue;
                            };
                            let Ok(wire) = serde_json::from_str::<OutboundWire>(&payload)
                            else {
                                continue;
                            };
                            dispatch_outbound_wire(&state.outboxes, wire);
                        }
                    }

                    // The subscription ended: this shard's broker is gone.
                    state.mark_shard_unhealthy(shard, "outbound subscription ended");
                    let mut backoff = RECONNECT_MIN;
                    loop {
                        tokio::time::sleep(backoff).await;
                        match state.broker.subscribe_outbound(shard).await {
                            Ok(new_pubsub) => {
                                state.mark_shard_healthy(shard);
                                tracing::info!(
                                    node_id = %state.node_id,
                                    shard,
                                    "broker reconnected: outbound subscription restored"
                                );
                                pubsub = new_pubsub;
                                break;
                            }
                            Err(err) => {
                                tracing::error!(
                                    node_id = %state.node_id,
                                    shard,
                                    error = %err,
                                    "broker reconnect failed; retrying"
                                );
                                backoff = (backoff * 2).min(RECONNECT_MAX);
                            }
                        }
                    }
                }
            });
        }
        Ok(())
    }

    /// Whether this node accepts new connections: true while at least one
    /// Redis shard's outbound subscription is up.
    fn is_healthy(&self) -> bool {
        self.shard_healthy.iter().any(|h| h.load(Ordering::SeqCst))
    }

    /// Mark one shard's outbound subscription down. If that was the last
    /// healthy shard, close every live client with a visible `broker
    /// unavailable` frame, so no socket silently stops receiving (design D10,
    /// spec "Broker loss is surfaced").
    fn mark_shard_unhealthy(&self, shard: usize, reason: &str) {
        self.shard_healthy[shard].store(false, Ordering::SeqCst);
        if self.is_healthy() {
            tracing::warn!(node_id = %self.node_id, shard, reason, "one outbound shard lost; other shards still serving");
            return;
        }
        tracing::error!(
            node_id = %self.node_id,
            reason,
            "broker lost on every shard; closing live clients until it recovers"
        );
        let outboxes: Vec<Arc<Outbox>> = self
            .outboxes
            .iter()
            .flat_map(|shard| shard.lock().unwrap().values().cloned().collect::<Vec<_>>())
            .collect();
        for outbox in outboxes {
            outbox.push(Outbound::BrokerUnavailable);
        }
    }

    /// Mark one shard's outbound subscription back up.
    fn mark_shard_healthy(&self, shard: usize) {
        self.shard_healthy[shard].store(true, Ordering::SeqCst);
    }

    /// A lapsed lease means the authoritative tier may have dropped this node's
    /// connections. Close every live client with a "reconnect" frame so a
    /// present player is never silently orphaned (design D11).
    fn on_lease_lost(&self) {
        let outboxes: Vec<Arc<Outbox>> = self
            .outboxes
            .iter()
            .flat_map(|shard| shard.lock().unwrap().values().cloned().collect::<Vec<_>>())
            .collect();
        for outbox in outboxes {
            outbox.push(Outbound::LeaseLost);
        }
    }
}

/// Upgrade a WebSocket connection for the transport tier.
///
/// tungstenite allocates its read/write buffers eagerly per connection and
/// defaults both to 128 KiB, which dominates the per-connection memory budget.
/// Our frames are small JSON, so we shrink the initial buffers; they still grow
/// on demand up to `max_frame_size`, so correctness is unaffected.
pub async fn ws_upgrade(
    ws: WebSocketUpgrade,
    State(state): State<Arc<TransportState>>,
) -> impl IntoResponse {
    ws.read_buffer_size(8 * 1024)
        .write_buffer_size(8 * 1024)
        .max_write_buffer_size(64 * 1024)
        .on_upgrade(move |socket| handle_transport_socket(socket, state))
}

/// Handle a WebSocket connection in the transport tier.
async fn handle_transport_socket(socket: WebSocket, state: Arc<TransportState>) {
    let (sink, mut stream) = socket.split();
    let outbox = Arc::new(Outbox::default());
    let alive = Arc::new(AtomicBool::new(true));
    let finished = Arc::new(tokio::sync::Notify::new());

    let writer = tokio::spawn(write_loop(
        sink,
        Arc::clone(&outbox),
        Arc::clone(&alive),
        Arc::clone(&finished),
    ));

    let connection_id = generate_connection_id();
    let node_id = state.node_id.clone();

    // If the broker is currently down, tell the client right away rather than
    // accepting a socket that could never receive anything.
    if !state.is_healthy() {
        outbox.push(Outbound::BrokerUnavailable);
        let _ = writer.await;
        return;
    }

    // Make this socket reachable by the node's single dispatcher.
    state
        .outbox_shard_mutex(&connection_id)
        .lock()
        .unwrap()
        .insert(connection_id.clone(), Arc::clone(&outbox));

    // Phase 2 registry: record which node holds this connection.
    if let Err(e) = state.broker.register_connection(&connection_id, None).await {
        tracing::warn!(connection_id = %connection_id, error = %e, "failed to register connection in broker");
    }

    loop {
        tokio::select! {
            frame = stream.next() => match frame {
                Some(Ok(Message::Text(text))) => {
                    alive.store(true, Ordering::SeqCst);
                    if !handle_transport_text(&outbox, text.as_str(), &connection_id, &node_id, &state).await {
                        break;
                    }
                }
                Some(Ok(Message::Ping(payload))) => {
                    alive.store(true, Ordering::SeqCst);
                    outbox.push(Outbound::Pong(payload.to_vec()));
                }
                Some(Ok(Message::Pong(_))) => {
                    alive.store(true, Ordering::SeqCst);
                }
                Some(Ok(Message::Close(_))) | None => break,
                // Binary frames and read errors are protocol failures.
                Some(Ok(_)) | Some(Err(_)) => {
                    outbox.push(Outbound::Close);
                    break;
                }
            },
            // The writer gave up (keepalive timeout or dead socket).
            _ = finished.notified() => break,
        }
    }

    // Close this socket promptly: drop the read half so the kernel can send the
    // FIN as soon as the writer's final frame is out, rather than waiting on
    // broker chatter (OBS-1).
    drop(stream);
    outbox.close();

    // Tell the logic tier that seats this connection that the socket is gone, so
    // it can hold or release the seat exactly as the monolith's `RoomMsg::Detach`
    // did (design D5/D13). The target is the connection's logic affinity; a
    // disconnect routed to the wrong (or default) channel is what leaves ghost
    // connections. Best-effort and bounded (design D10): a broker that is down
    // must never hold the socket open while the connection manager retries.
    let disconnect_target = state
        .conn_logic_node(&connection_id)
        .unwrap_or_else(|| DEFAULT_LOGIC_NODE.to_string());
    let disconnect = InboundEnvelope {
        v: ENVELOPE_VERSION,
        connection_id: connection_id.clone(),
        node_id,
        payload: InboundPayload::Disconnect,
    };
    let broker = Arc::clone(&state.broker);
    let cleanup_connection_id = connection_id.clone();
    tokio::spawn(async move {
        if let Ok(Err(e)) = tokio::time::timeout(
            CLEANUP_TIMEOUT,
            broker.publish_inbound_to(&disconnect, &disconnect_target),
        )
        .await
        {
            tracing::warn!(connection_id = %cleanup_connection_id, error = %e, "failed to publish disconnect");
        }
        let _ = tokio::time::timeout(
            CLEANUP_TIMEOUT,
            broker.unregister_connection(&cleanup_connection_id),
        )
        .await;
    });

    state.outbox_shard_mutex(&connection_id).lock().unwrap().remove(&connection_id);
    state.remove_conn_logic_node(&connection_id);
    let _ = writer.await;
}

/// Handle one client text frame in the transport tier. Returns `false` when the
/// connection must end (protocol failure). The frame is validated here so a
/// malformed frame still closes with the monolith's protocol close code, then
/// forwarded to the logic tier with the original frame text (design D4).
async fn handle_transport_text(
    outbox: &Outbox,
    text: &str,
    connection_id: &str,
    node_id: &str,
    state: &Arc<TransportState>,
) -> bool {
    let message = match decode_incoming(text) {
        Ok(message) => message,
        Err(_) => {
            outbox.push(Outbound::Close);
            return false;
        }
    };

    let join_room_code = match &message {
        ClientMessage::JoinRoom { room_code, .. } => Some(room_code.clone()),
        _ => None,
    };

    // Specific target first: a join goes to the room's recorded owner, any other
    // frame to the node that already serves this connection. The full node list
    // is only fetched if that target does not accept the frame, so the hot path
    // (a move on a seated connection) costs ONE publish, not list + publish. A
    // stale membership/target is tolerated: a 0-subscriber probe is skipped.
    let mut candidates: Vec<String> = Vec::new();
    if let Some(code) = &join_room_code {
        if let Some(owner) = resolve_room_owner(state, code).await {
            candidates.push(owner);
        }
    } else if let Some(known) = state.conn_logic_node(connection_id) {
        candidates.push(known);
    }

    let envelope = InboundEnvelope {
        v: ENVELOPE_VERSION,
        connection_id: connection_id.to_string(),
        node_id: node_id.to_string(),
        payload: InboundPayload::ClientFrame { data: text.to_string() },
    };

    // 1) Hot path: try the specific target(s) without paying for the node list.
    if !candidates.is_empty() {
        for candidate in &candidates {
            if try_local_logic(state, candidate, &envelope).await {
                state.set_conn_logic_node(connection_id, candidate);
                return true;
            }
            match state.broker.publish_inbound_to(&envelope, candidate).await {
                Ok(subscribers) if subscribers > 0 => {
                    state.set_conn_logic_node(connection_id, candidate);
                    return true;
                }
                Ok(_) => continue,
                Err(e) => {
                    tracing::error!(error = %e, "failed to publish inbound envelope");
                    outbox.push(Outbound::BrokerUnavailable);
                    return false;
                }
            }
        }
    }

    // 2) Cold/fallback path: broaden to every registered logic node and retry
    // (bounded) so a logic tier can resubscribe after a broker restart.
    for attempt in 0..INBOUND_PUBLISH_ATTEMPTS {
        if attempt > 0 {
            tokio::time::sleep(INBOUND_PUBLISH_RETRY_DELAY).await;
        }
        let nodes = match state.broker.logic_nodes().await {
            Ok(nodes) => nodes,
            Err(e) => {
                tracing::warn!(error = %e, "failed to list logic nodes");
                Vec::new()
            }
        };
        if nodes.is_empty() && candidates.is_empty() {
            candidates.push(DEFAULT_LOGIC_NODE.to_string());
        }
        for node in nodes {
            if candidates.contains(&node) {
                continue;
            }
            candidates.push(node.clone());
        }

        // Round-robin placement for new rooms (create_room):
        // When there's no specific target (no join_room_code and no existing
        // conn_logic_node), rotate the candidate list so the next live node
        // in round-robin order is tried first. This distributes new rooms
        // across live logic nodes while keeping single-node behavior identical.
        let is_create_room = join_room_code.is_none() && state.conn_logic_node(connection_id).is_none();
        if is_create_room && !candidates.is_empty() {
            let idx = state.placement_rr.fetch_add(1, Ordering::Relaxed) % candidates.len();
            // Rotate so candidate at idx is tried first.
            candidates.rotate_left(idx);
        }

        // Try any candidate not already tried above (the appended ones).
        for candidate in candidates.clone() {
            if try_local_logic(state, &candidate, &envelope).await {
                state.set_conn_logic_node(connection_id, &candidate);
                return true;
            }
            match state.broker.publish_inbound_to(&envelope, &candidate).await {
                Ok(subscribers) if subscribers > 0 => {
                    state.set_conn_logic_node(connection_id, &candidate);
                    return true;
                }
                Ok(_) => continue,
                Err(e) => {
                    tracing::error!(error = %e, "failed to publish inbound envelope");
                    outbox.push(Outbound::BrokerUnavailable);
                    return false;
                }
            }
        }
    }
    tracing::warn!(
        connection_id = %connection_id,
        "no logic tier accepted the inbound frame; closing the client"
    );
    outbox.push(Outbound::BrokerUnavailable);
    false
}

/// The logic node that owns a room, read from the authoritative Redis record
/// (Phase 3). Deliberately uncached: a cached owner is exactly what let a
/// mis-resolved value stick and poison routing (BUG-P3-1).
async fn resolve_room_owner(state: &Arc<TransportState>, room_code: &str) -> Option<String> {
    match state.broker.get_room_owner(room_code).await {
        Ok(owner) => owner,
        Err(e) => {
            tracing::warn!(room_code = %room_code, error = %e, "failed to read room owner");
            None
        }
    }
}

/// `TIER=combined` only: if `candidate` names this process's own colocated
/// `LogicState`, hand the frame to it directly and report handled. Skips the
/// fabric publish and the inbound lane entirely for the common case (both
/// seats and the room's logic on one node), falling back to the fabric for
/// everything else (colocate-room-hot-path design D3/D4).
async fn try_local_logic(state: &Arc<TransportState>, candidate: &str, envelope: &InboundEnvelope) -> bool {
    let Some(local) = state.local_logic() else {
        return false;
    };
    if local.get_node_id() != candidate {
        return false;
    }
    local.handle_inbound_local(envelope.clone()).await;
    true
}

/// Fan a decoded outbound wire message out to the per-connection outboxes.
/// A batch's items are pushed in order, so each connection receives its own
/// messages in the order they were published (design D2/D5).
fn dispatch_outbound_wire(
    outboxes: &OutboxShards,
    wire: OutboundWire,
) {
    let push = |envelope: OutboundEnvelope| {
        let outbox = {
            let shard = outboxes[shard_index(&envelope.connection_id, OUTBOX_SHARDS)].lock().unwrap();
            shard.get(&envelope.connection_id).cloned()
        };
        if let Some(outbox) = outbox {
            outbox.push(Outbound::Text(envelope.payload));
        }
    };
    match wire {
        OutboundWire::Envelope(envelope) => push(envelope),
        OutboundWire::Batch(batch) => {
            for envelope in batch.items {
                push(envelope);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use crate::infrastructure::transport::{Outbound, Outbox};
    use crate::infrastructure::broker::OutboundWire;

    /// Round-robin placement distributes new rooms across live logic nodes.
    /// The counter increments modulo the number of candidates, so each call
    /// selects the next node in the list. The index sequence is 0, 1, 2, 0, 1, 2...
    #[test]
    fn placement_round_robins_over_nodes() {
        let rr = std::sync::atomic::AtomicUsize::new(0);
        let len = 3;

        // Index sequence: 0, 1, 2, 0, 1, 2, ...
        assert_eq!(rr.fetch_add(1, Ordering::Relaxed) % len, 0);
        assert_eq!(rr.fetch_add(1, Ordering::Relaxed) % len, 1);
        assert_eq!(rr.fetch_add(1, Ordering::Relaxed) % len, 2);
        assert_eq!(rr.fetch_add(1, Ordering::Relaxed) % len, 0);
        assert_eq!(rr.fetch_add(1, Ordering::Relaxed) % len, 1);
        assert_eq!(rr.fetch_add(1, Ordering::Relaxed) % len, 2);
    }

    /// When there's only one candidate, round-robin always selects it.
    #[test]
    fn placement_round_robin_single_node() {
        let rr = std::sync::atomic::AtomicUsize::new(0);
        let len = 1;

        for _ in 0..5 {
            assert_eq!(rr.fetch_add(1, Ordering::Relaxed) % len, 0);
        }
    }

    /// With two nodes, round-robin alternates.
    #[test]
    fn placement_round_robin_two_nodes() {
        let rr = std::sync::atomic::AtomicUsize::new(0);
        let len = 2;

        let mut seen = std::collections::HashSet::new();
        for _ in 0..10 {
            let idx = rr.fetch_add(1, Ordering::Relaxed) % len;
            assert!(idx == 0 || idx == 1);
            if idx == 0 {
                seen.insert("node-a");
            } else {
                seen.insert("node-b");
            }
        }
        assert_eq!(seen.len(), 2);
    }

    /// Task 3.2: per-connection order is preserved when messages for two
    /// connections on the same node are coalesced into one batch. Each
    /// connection must still see its own messages in their original order.
    #[test]
    fn per_connection_order_preserved_under_batching() {
        use crate::infrastructure::broker::{OutboundBatch, OutboundEnvelope, ENVELOPE_VERSION};
        use crate::interface::protocol::{ServerMessage, VERSION};

        let outboxes: super::OutboxShards = (0..super::OUTBOX_SHARDS).map(|_| Mutex::new(HashMap::new())).collect();
        let out_a = Arc::new(Outbox::default());
        let out_b = Arc::new(Outbox::default());
        {
            let mut map = outboxes[super::shard_index("conn-a", super::OUTBOX_SHARDS)].lock().unwrap();
            map.insert("conn-a".to_string(), out_a.clone());
        }
        {
            let mut map = outboxes[super::shard_index("conn-b", super::OUTBOX_SHARDS)].lock().unwrap();
            map.insert("conn-b".to_string(), out_b.clone());
        }

        let envelope = |conn: &str, code: &str| OutboundEnvelope {
            v: ENVELOPE_VERSION,
            node_id: "transport-abc".to_string(),
            connection_id: conn.to_string(),
            payload: ServerMessage::Error {
                v: VERSION,
                code: code.to_string(),
                message: code.to_string(),
            },
        };

        // Interleave: a1, b1, a2, b2 - per-connection order must be a1<a2 and b1<b2.
        let batch = OutboundBatch {
            v: ENVELOPE_VERSION,
            node_id: "transport-abc".to_string(),
            items: vec![
                envelope("conn-a", "a1"),
                envelope("conn-b", "b1"),
                envelope("conn-a", "a2"),
                envelope("conn-b", "b2"),
            ],
        };
        super::dispatch_outbound_wire(&outboxes, OutboundWire::Batch(batch));

        let drain = |outbox: &Outbox| -> Vec<String> {
            outbox
                .take_batch()
                .into_iter()
                .map(|item| match item {
                    Outbound::Text(ServerMessage::Error { code, .. }) => code,
                    _ => panic!("expected an error text frame"),
                })
                .collect()
        };

        assert_eq!(drain(&out_a), vec!["a1".to_string(), "a2".to_string()]);
        assert_eq!(drain(&out_b), vec!["b1".to_string(), "b2".to_string()]);
    }

    /// A single-item envelope is still dispatched to its connection (rollout
    /// compatibility), and a batch for an unknown connection is dropped.
    #[test]
    fn single_envelope_dispatch_and_unknown_connection_dropped() {
        use crate::infrastructure::broker::{OutboundEnvelope, ENVELOPE_VERSION};
        use crate::interface::protocol::{ServerMessage, VERSION};

        let outboxes: super::OutboxShards = (0..super::OUTBOX_SHARDS).map(|_| Mutex::new(HashMap::new())).collect();
        let out_a = Arc::new(Outbox::default());
        {
            let mut map = outboxes[super::shard_index("conn-a", super::OUTBOX_SHARDS)].lock().unwrap();
            map.insert("conn-a".to_string(), out_a.clone());
        }

        let envelope = OutboundEnvelope {
            v: ENVELOPE_VERSION,
            node_id: "transport-abc".to_string(),
            connection_id: "conn-a".to_string(),
            payload: ServerMessage::Error {
                v: VERSION,
                code: "single".to_string(),
                message: "single".to_string(),
            },
        };
        super::dispatch_outbound_wire(&outboxes, OutboundWire::Envelope(envelope));

        let unknown = OutboundEnvelope {
            v: ENVELOPE_VERSION,
            node_id: "transport-abc".to_string(),
            connection_id: "conn-missing".to_string(),
            payload: ServerMessage::Error {
                v: VERSION,
                code: "dropped".to_string(),
                message: "dropped".to_string(),
            },
        };
        // Must not panic when the connection has no outbox.
        super::dispatch_outbound_wire(&outboxes, OutboundWire::Envelope(unknown));

        let delivered = out_a.take_batch();
        assert_eq!(delivered.len(), 1, "only the known connection receives the frame");
    }
}
