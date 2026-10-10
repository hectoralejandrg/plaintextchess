//! Logic tier handler: subscribes to inbound envelopes from the Redis fabric,
//! dispatches them into the existing `App`/room actors, and publishes outbound
//! envelopes back to the owning transport connection (design D5/D6).
//!
//! Inbound envelopes are routed by a single reader to bounded lanes: room-scoped
//! messages by `hash(room_code) % K` (per-room order preserved) and control-plane
//! messages (create room, auth) by `hash(connection_id) % C`. Independent lanes
//! run concurrently; the bounded lanes make the reader the backpressure point.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, Mutex as AsyncMutex, oneshot};
use futures_util::StreamExt;
use tracing::{debug, error, info, warn};

use crate::application::room_actor::RoomMsg;
use crate::domain::account::Identity;
use crate::domain::time_control::TimeControl;
use crate::infrastructure::broker::{
    Broker, InboundEnvelope, InboundPayload, OutboundEnvelope, ENVELOPE_VERSION,
};
use crate::infrastructure::password::now_ms;
use crate::infrastructure::server::App;
use crate::interface::protocol::{ClientMessage, ErrorCode, ServerMessage, VERSION};

/// Backoff bounds and failure budget for broker reconnection (design D10).
const RECONNECT_MIN: Duration = Duration::from_millis(200);
const RECONNECT_MAX: Duration = Duration::from_secs(2);
const MAX_SUBSCRIBE_FAILURES: u32 = 10;

/// Transport node liveness watcher constants (design D11).
pub const NODE_WATCH_INTERVAL: Duration = Duration::from_secs(5);
pub const NODE_MISS_THRESHOLD: u32 = 2;

/// How long after a node's lease lapses the logic declares it dead (design D11).
/// Combined with the transport lease TTL, it must stay inside the reconnect
/// grace window so seats enter grace in time to be reclaimed.
pub fn node_detection_window() -> Duration {
    NODE_WATCH_INTERVAL * NODE_MISS_THRESHOLD
}

/// How often a logic node refreshes (and, if needed, re-claims) its room
/// ownership leases (Phase 3 / BUG-P3-1). Well under `ownership::TTL_SECS`.
const ROOM_LEASE_RENEW_INTERVAL: Duration = Duration::from_secs(10);

/// Maximum expected duration for a single renewal sweep (one round trip).
/// Used for the startup invariant: TTL must exceed sweep_interval + sweep_duration.
const ROOM_LEASE_RENEW_SWEEP_MAX: Duration = Duration::from_secs(5);

/// Exponential backoff, capped, from a 1-based failure count.
fn reconnect_backoff(failures: u32) -> Duration {
    (RECONNECT_MIN * (1u32 << failures.min(4))).min(RECONNECT_MAX)
}

/// Capacity of each bounded inbound lane (design D3). The reader pauses when a
/// lane is full, providing end-to-end backpressure.
const LANE_CAPACITY: usize = 1024;

/// One item on a lane: the envelope plus when the reader routed it, so a worker
/// can record queue-wait + processing latency (task 6.1).
struct LaneItem {
    envelope: InboundEnvelope,
    routed_at: Instant,
}

/// A bounded inbound lane.
type LaneSender = mpsc::Sender<LaneItem>;
type LaneReceiver = mpsc::Receiver<LaneItem>;

/// Per-lane counters (task 6.1). Queue depth is derived from the bounded sender
/// (`max_capacity - capacity`); the timing fields are recorded per processed item.
#[derive(Default)]
struct LaneMetrics {
    processed: AtomicU64,
    last_us: AtomicU64,
    max_us: AtomicU64,
    sum_us: AtomicU64,
}

impl LaneMetrics {
    fn record(&self, micros: u64) {
        self.processed.fetch_add(1, Ordering::Relaxed);
        self.last_us.store(micros, Ordering::Relaxed);
        self.max_us.fetch_max(micros, Ordering::Relaxed);
        self.sum_us.fetch_add(micros, Ordering::Relaxed);
    }
}

/// Which lane pool a client message belongs to (design D2). Non-room-scoped
/// control work (create room, auth) goes to the control pool; room-scoped work
/// stays on the room pool so it keeps the room's order.
#[derive(Debug, PartialEq, Eq)]
enum Route {
    Room,
    Control,
}

/// Classify a client message into its lane pool. `seated_room` is the room the
/// connection currently holds, if any.
fn route_kind(message: &ClientMessage, seated_room: Option<&str>) -> Route {
    match message {
        ClientMessage::CreateRoom { .. }
        | ClientMessage::Register { .. }
        | ClientMessage::Login { .. }
        | ClientMessage::Logout { .. }
        | ClientMessage::SetProfile { .. } => Route::Control,
        ClientMessage::JoinRoom { .. } => Route::Room,
        ClientMessage::Move { .. } | ClientMessage::Resign { .. } | ClientMessage::Leave { .. } => {
            if seated_room.is_some() {
                Route::Room
            } else {
                Route::Control
            }
        }
    }
}

/// JSON for one lane's metrics (task 6.1). Queue depth is derived from the
/// bounded sender; the rest is read from the lane's counters.
fn lane_metrics_json(sender: &LaneSender, m: &LaneMetrics) -> serde_json::Value {
    let depth = sender.max_capacity().saturating_sub(sender.capacity());
    let processed = m.processed.load(Ordering::Relaxed);
    let sum = m.sum_us.load(Ordering::Relaxed);
    serde_json::json!({
        "depth": depth,
        "processed": processed,
        "last_us": m.last_us.load(Ordering::Relaxed),
        "max_us": m.max_us.load(Ordering::Relaxed),
        "avg_us": if processed > 0 { sum / processed } else { 0 },
    })
}

/// State for a connection being handled by the logic tier.
struct LogicConnection {
    /// The connection ID from the transport tier.
    connection_id: String,
    /// The transport node ID that owns this connection.
    transport_node_id: String,
    /// The outbound sender to the room actor (if seated in a room).
    room_mailbox: Option<mpsc::UnboundedSender<RoomMsg>>,
    /// Receiver for outbound messages from the room actor, drained by a
    /// forwarder task once the connection is seated.
    out_rx: Option<mpsc::UnboundedReceiver<ServerMessage>>,
    /// The identity pinned to this connection (design D8).
    identity: Identity,
    /// Room code if seated.
    room_code: Option<String>,
}

/// Shard count for partitioned connections map (matches room lanes for simplicity).
const CONNECTION_SHARDS: usize = 16;

/// Sharded connections map to avoid a single global lock.
/// Each shard is an independent AsyncMutex<HashMap>.
type ConnectionsShards = Vec<AsyncMutex<HashMap<String, LogicConnection>>>;

/// Shared state for the logic tier.
pub struct LogicState {
    app: Arc<App>,
    broker: Arc<Broker>,
    node_id: String,
    /// Sharded connection maps to avoid a single global lock (task 3.1).
    /// Shard = hash(connection_id) % CONNECTION_SHARDS.
    connections: ConnectionsShards,
    /// Bounded lane senders for room-scoped messages (design D1).
    /// Index = lane_id = hash(room_code) % K.
    room_lane_senders: Vec<LaneSender>,
    /// Bounded lane receivers for room-scoped messages, taken once by `run()`.
    /// Wrapped so `run()` does not need unique ownership of the `Arc` (a second
    /// `Arc` is held by the caller for graceful shutdown).
    room_lane_receivers: std::sync::Mutex<Option<Vec<LaneReceiver>>>,
    /// Bounded lane senders for control-plane messages (design D2).
    /// Index = connection_id hash % C.
    control_lane_senders: Vec<LaneSender>,
    /// Bounded lane receivers for control-plane messages, taken once by `run()`.
    control_lane_receivers: std::sync::Mutex<Option<Vec<LaneReceiver>>>,
    /// Number of room lanes (LOGIC_LANES).
    room_lane_count: usize,
    /// Number of control lanes (LOGIC_CONTROL_LANES).
    control_lane_count: usize,
    /// Per-lane counters for observability (task 6.1).
    room_lane_metrics: Vec<Arc<LaneMetrics>>,
    control_lane_metrics: Vec<Arc<LaneMetrics>>,
    /// Aggregate inbound move-dispatch latency (task 6.1).
    move_metrics: Arc<LaneMetrics>,
}

impl LogicState {
    pub fn new(app: Arc<App>, broker: Arc<Broker>, node_id: String, room_lane_count: usize, control_lane_count: usize) -> Self {
        // Create bounded room lanes (design D3: bounded capacity for backpressure).
        let mut room_lane_senders = Vec::with_capacity(room_lane_count);
        let mut room_lane_receivers = Vec::with_capacity(room_lane_count);
        let mut room_lane_metrics = Vec::with_capacity(room_lane_count);
        for _ in 0..room_lane_count {
            let (tx, rx) = mpsc::channel(LANE_CAPACITY);
            room_lane_senders.push(tx);
            room_lane_receivers.push(rx);
            room_lane_metrics.push(Arc::new(LaneMetrics::default()));
        }

        // Create bounded control lanes (design D2).
        let mut control_lane_senders = Vec::with_capacity(control_lane_count);
        let mut control_lane_receivers = Vec::with_capacity(control_lane_count);
        let mut control_lane_metrics = Vec::with_capacity(control_lane_count);
        for _ in 0..control_lane_count {
            let (tx, rx) = mpsc::channel(LANE_CAPACITY);
            control_lane_senders.push(tx);
            control_lane_receivers.push(rx);
            control_lane_metrics.push(Arc::new(LaneMetrics::default()));
        }

        Self {
            app,
            broker,
            node_id,
            connections: (0..CONNECTION_SHARDS)
                .map(|_| AsyncMutex::new(HashMap::new()))
                .collect(),
            room_lane_senders,
            room_lane_receivers: std::sync::Mutex::new(Some(room_lane_receivers)),
            control_lane_senders,
            control_lane_receivers: std::sync::Mutex::new(Some(control_lane_receivers)),
            room_lane_count,
            control_lane_count,
            room_lane_metrics,
            control_lane_metrics,
            move_metrics: Arc::new(LaneMetrics::default()),
        }
    }

    /// Get the node ID.
    pub fn get_node_id(&self) -> &str {
        &self.node_id
    }

    /// Get the connections shard for a connection_id.
    fn connections_shard(&self, connection_id: &str) -> &AsyncMutex<HashMap<String, LogicConnection>> {
        let shard_idx = Self::hash_connection_id(connection_id) % CONNECTION_SHARDS;
        &self.connections[shard_idx]
    }

    /// Snapshot of inbound lane metrics for `/healthz` (task 6.1). Queue depth is
    /// derived from each bounded sender; timings are recorded per processed item.
    pub fn metrics_snapshot(&self) -> serde_json::Value {
        let rooms: Vec<serde_json::Value> = self
            .room_lane_senders
            .iter()
            .zip(&self.room_lane_metrics)
            .map(|(s, m)| lane_metrics_json(s, m))
            .collect();
        let control: Vec<serde_json::Value> = self
            .control_lane_senders
            .iter()
            .zip(&self.control_lane_metrics)
            .map(|(s, m)| lane_metrics_json(s, m))
            .collect();
        let mm = &self.move_metrics;
        let mp = mm.processed.load(Ordering::Relaxed);
        let msum = mm.sum_us.load(Ordering::Relaxed);
        serde_json::json!({
            "room_lanes": self.room_lane_count,
            "control_lanes": self.control_lane_count,
            "room_lane_metrics": rooms,
            "control_lane_metrics": control,
            "move_dispatch": {
                "processed": mp,
                "last_us": mm.last_us.load(Ordering::Relaxed),
                "max_us": mm.max_us.load(Ordering::Relaxed),
                "avg_us": if mp > 0 { msum / mp } else { 0 },
            },
        })
    }

    /// Start the logic tier: register the node lease, subscribe to inbound,
    /// spawn room/control workers, then run the reader fan-out loop.
    /// The subscription is re-established when the broker drops; if it stays
    /// gone past a bounded number of attempts the tier exits non-zero so a
    /// supervisor notices (spec "Broker loss is surfaced").
    ///
    /// If `ready_tx` is provided, a signal is sent when the logic node has
    /// subscribed to its inbound channel and workers are running.
    /// This is useful for test harnesses that need to wait for the node to be ready.
    pub async fn run(self: Arc<Self>, ready_tx: Option<oneshot::Sender<()>>) -> Result<(), Box<dyn std::error::Error>> {
        info!(node_id = %self.node_id, room_lanes = self.room_lane_count, control_lanes = self.control_lane_count, "logic tier starting");

        // Fail fast at startup: no broker, no service.
        self.broker.register_logic_node(None).await?;

        // Assert that the room ownership TTL safely exceeds the sweep interval
        // plus the worst-case sweep duration, so a live node never spuriously
        // re-claims its own rooms under load (task 5.2).
        let ttl_secs = crate::infrastructure::broker::ownership::TTL_SECS as u64;
        let sweep_interval = ROOM_LEASE_RENEW_INTERVAL.as_secs();
        let sweep_max = ROOM_LEASE_RENEW_SWEEP_MAX.as_secs();
        if ttl_secs <= sweep_interval + sweep_max {
            return Err(format!(
                "ROOM_OWNERSHIP_TTL_SECS ({ttl_secs}) must exceed sweep interval ({sweep_interval}s) + max sweep ({sweep_max}s); \
                adjust TTL or reduce sweep interval/max"
            ).into());
        }
        info!(ttl_secs, sweep_interval, sweep_max, "room ownership TTL invariant satisfied");

        // Subscribe to our inbound channel immediately so we're ready to receive
        // messages before spawning background tasks (helps with test harness timing).
        let mut pubsub = self.broker.subscribe_inbound_self().await?;

        // Take the lane receivers once. They live behind a Mutex so `run()` does
        // not need unique ownership of the Arc: the caller keeps a second Arc to
        // trigger graceful shutdown while the reader loop runs.
        let mut room_lane_receivers = self
            .room_lane_receivers
            .lock()
            .expect("lane receivers mutex poisoned")
            .take()
            .expect("logic tier run() called twice");
        let mut control_lane_receivers = self
            .control_lane_receivers
            .lock()
            .expect("lane receivers mutex poisoned")
            .take()
            .expect("logic tier run() called twice");

        // Spawn room lane workers (design D1): each worker processes one lane.
        // Room-scoped messages (moves, resigns, leaves for seated connections,
        // disconnects for seated connections) are routed to these lanes by
        // hash(room_code) % K, preserving per-room FIFO ordering.
        for lane_id in 0..self.room_lane_count {
            let mut lane_receiver = room_lane_receivers.remove(0);
            let state = Arc::clone(&self);
            let metrics = Arc::clone(&self.room_lane_metrics[lane_id]);
            let move_metrics = Arc::clone(&self.move_metrics);
            tokio::spawn(async move {
                info!(node_id = %state.node_id, lane_id, "room lane worker started");
                while let Some(item) = lane_receiver.recv().await {
                    let is_move = matches!(
                        &item.envelope.payload,
                        InboundPayload::Client(ClientMessage::Move { .. })
                    );
                    state.handle_inbound(item.envelope).await;
                    let micros = item.routed_at.elapsed().as_micros() as u64;
                    metrics.record(micros);
                    if is_move {
                        move_metrics.record(micros);
                    }
                }
                info!(node_id = %state.node_id, lane_id, "room lane worker stopped");
            });
        }

        // Spawn control lane workers (design D2): each worker processes one
        // control lane. Control-plane messages (create/join/leave/resign/auth)
        // are routed by connection_id hash to isolate argon2 auth and Redis
        // round trips from the move path.
        for lane_id in 0..self.control_lane_count {
            let mut lane_receiver = control_lane_receivers.remove(0);
            let state = Arc::clone(&self);
            let metrics = Arc::clone(&self.control_lane_metrics[lane_id]);
            tokio::spawn(async move {
                info!(node_id = %state.node_id, lane_id, "control lane worker started");
                while let Some(item) = lane_receiver.recv().await {
                    state.handle_inbound(item.envelope).await;
                    metrics.record(item.routed_at.elapsed().as_micros() as u64);
                }
                info!(node_id = %state.node_id, lane_id, "control lane worker stopped");
            });
        }

        // Signal that we're ready to receive messages (workers are running).
        if let Some(tx) = ready_tx {
            let _ = tx.send(());
        }

        // Keep the node lease renewed (Phase 3 owns the re-resolve behaviour;
        // the lease is harmless in Phase 1).
        let lease_state = Arc::clone(&self);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(10));
            loop {
                interval.tick().await;
                if let Err(e) = lease_state.broker.renew_logic_node_lease().await {
                    warn!(error = %e, "failed to renew logic node lease");
                }
            }
        });

        // Keep this node's room ownership alive: refresh the lease, and re-claim
        // any room whose key was lost (DEL, expiry, or a broker restart wiped the
        // keyspace). A lost key must never leave an active room ownerless
        // (BUG-P3-1).
        let rooms_state = Arc::clone(&self);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(ROOM_LEASE_RENEW_INTERVAL);
            loop {
                interval.tick().await;
                rooms_state.keep_room_ownership().await;
            }
        });

        // Transport node liveness watcher (design D11): periodically checks the
        // `ptc.node.<id>` leases of transport nodes that have connections.
        // If a node's lease expires (it crashed without publishing disconnects),
        // synthesize Disconnect for all its connections so their seats enter grace.
        let watch_state = Arc::clone(&self);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(NODE_WATCH_INTERVAL);
            let mut misses: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
            loop {
                interval.tick().await;

                // Collect distinct transport node ids currently holding connections
                // across all shards.
                let nodes: Vec<String> = {
                    let mut seen = Vec::new();
                    for shard in &watch_state.connections {
                        let conns = shard.lock().await;
                        for conn in conns.values() {
                            if !conn.transport_node_id.is_empty() && !seen.contains(&conn.transport_node_id) {
                                seen.push(conn.transport_node_id.clone());
                            }
                        }
                    }
                    seen
                };

                for node in nodes {
                    match watch_state.broker.node_lease_alive(&node).await {
                        Ok(true) => {
                            // Node is alive, reset miss counter.
                            misses.remove(&node);
                        }
                        Ok(false) => {
                            // Node lease missing: increment miss counter.
                            let m = misses.entry(node.clone()).or_insert(0);
                            *m += 1;
                            if *m >= NODE_MISS_THRESHOLD {
                                // Two consecutive misses -> declare node dead.
                                misses.remove(&node);
                                // Collect connection ids on this node across all shards.
                                let ids: Vec<String> = {
                                    let mut ids = Vec::new();
                                    for shard in &watch_state.connections {
                                        let conns = shard.lock().await;
                                        for (id, c) in conns.iter() {
                                            if c.transport_node_id == node {
                                                ids.push(id.clone());
                                            }
                                        }
                                    }
                                    ids
                                };
                                for id in ids {
                                    watch_state.handle_disconnect(&id).await;
                                    // Also drop the stale registry entry for this connection.
                                    let _ = watch_state.broker.unregister_connection(&id).await;
                                }
                                warn!(node_id = %node, "transport node lease expired; synthesized disconnects for its connections");
                            }
                        }
                        Err(e) => {
                            // Redis error: don't count as a miss (could be transient).
                            warn!(node_id = %node, error = %e, "node lease check failed; will retry");
                        }
                    }
                }
            }
        });

        // Reader fan-out loop (design D1/D3): a single reader parses envelopes
        // and routes them to bounded lanes. The reader is the backpressure
        // point: `route_inbound` awaits a lane when it is full, so memory is
        // bounded and no envelope is dropped. Because the reader is single, a
        // saturated lane also backpressures the other lanes; this is a
        // documented limitation (see design D3) whose durable fix is per-shard
        // consumption. Per-room ordering is always preserved.
        let mut failures: u32 = 0;
        'reader: loop {
            // Process messages in a scope that drops the stream before reassigning pubsub.
            {
                let mut messages = pubsub.on_message();
                while let Some(message) = messages.next().await {
                    let Ok(payload) = message.get_payload::<String>() else {
                        continue;
                    };
                    let Ok(envelope) = serde_json::from_str::<InboundEnvelope>(&payload) else {
                        warn!("dropped an unparseable inbound envelope");
                        continue;
                    };
                    // Route to appropriate lane (design D1/D2).
                    self.route_inbound(envelope).await;
                }
            } // messages stream dropped here, releasing borrow on pubsub

            // Subscription ended (broker lost); reconnect within failure budget.
            error!(node_id = %self.node_id, "inbound subscription ended (broker lost); reconnecting");
            loop {
                failures += 1;
                if failures >= MAX_SUBSCRIBE_FAILURES {
                    // Give up: drop the membership key so this node is not
                    // mistaken for a live logic node, then exit non-zero.
                    let _ = self.broker.unregister_logic_node().await;
                    return Err(format!("broker unavailable after {failures} attempts").into());
                }
                tokio::time::sleep(reconnect_backoff(failures)).await;
                match self.broker.subscribe_inbound_self().await {
                    Ok(new_pubsub) => {
                        info!(node_id = %self.node_id, "broker reconnected: inbound subscription restored");
                        failures = 0;
                        pubsub = new_pubsub;
                        continue 'reader;
                    }
                    Err(e) => {
                        warn!(error = %e, failures, "failed to re-subscribe to the inbound channel; retrying");
                    }
                }
            }
        }
    }

    /// Route an inbound envelope to the appropriate lane (design D1/D2).
    /// - Room-scoped messages (join by room code, moves, resigns, leaves, and
    ///   disconnects for seated connections) go to room lane
    ///   `hash(room_code) % K`, so a room's messages are always ordered.
    /// - Control-plane messages (create room, auth) and messages for unseated
    ///   connections go to control lane `hash(connection_id) % C`, keeping
    ///   argon2 auth and create-room Redis round trips off the move path.
    /// Bounded lanes apply backpressure through the reader.
    async fn route_inbound(&self, envelope: InboundEnvelope) {
        let lane_sender = match &envelope.payload {
            InboundPayload::Client(message) => {
                let seated = self.connection_room_code(&envelope.connection_id).await;
                match route_kind(message, seated.as_deref()) {
                    Route::Room => match message {
                        ClientMessage::JoinRoom { room_code, .. } => self.room_lane_for_room(room_code),
                        _ => {
                            let room = seated
                                .as_deref()
                                .expect("a room-scoped route requires the connection to be seated");
                            self.room_lane_for_room(room)
                        }
                    },
                    Route::Control => self.control_lane_for_connection(&envelope.connection_id),
                }
            }
            InboundPayload::Disconnect => {
                match self.connection_room_code(&envelope.connection_id).await {
                    Some(room_code) => self.room_lane_for_room(&room_code),
                    None => self.control_lane_for_connection(&envelope.connection_id),
                }
            }
        };

        // Bounded lanes: awaiting a full lane applies backpressure. A closed
        // lane means its worker is gone (panic/shutdown); surface it instead of
        // dropping silently.
        let connection_id = envelope.connection_id.clone();
        let item = LaneItem {
            envelope,
            routed_at: Instant::now(),
        };
        if lane_sender.send(item).await.is_err() {
            warn!(connection_id = %connection_id, "inbound lane worker is gone; envelope dropped");
        }
    }

    /// Get the room lane sender for a room code (hash(room_code) % K).
    fn room_lane_for_room(&self, room_code: &str) -> &LaneSender {
        let lane_id = Self::hash_room_code(room_code) % self.room_lane_count;
        &self.room_lane_senders[lane_id]
    }

    /// Get the control lane sender for a connection (hash(connection_id) % C).
    fn control_lane_for_connection(&self, connection_id: &str) -> &LaneSender {
        let lane_id = Self::hash_connection_id(connection_id) % self.control_lane_count;
        &self.control_lane_senders[lane_id]
    }

    /// Hash a room code to a lane index.
    fn hash_room_code(room_code: &str) -> usize {
        // Simple FNV-1a hash for deterministic distribution.
        let mut hash: u64 = 0xcbf29ce484222325;
        for byte in room_code.as_bytes() {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash as usize
    }

    /// Hash a connection id to a lane index.
    fn hash_connection_id(connection_id: &str) -> usize {
        let mut hash: u64 = 0xcbf29ce484222325;
        for byte in connection_id.as_bytes() {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash as usize
    }

    /// Get the room code for a connection, if seated.
    async fn connection_room_code(&self, connection_id: &str) -> Option<String> {
        let shard = self.connections_shard(connection_id);
        let connections = shard.lock().await;
        connections.get(connection_id).and_then(|c| c.room_code.clone())
    }

    /// Refresh this node's ownership of every room it still holds, re-claiming a
    /// room whose ownership key was lost (BUG-P3-1). The room list comes from the
    /// authoritative `App`, not from scanning Redis keys (a lost key would make a
    /// scan-based sweep miss exactly the room that needs re-claiming).
    /// Uses a pipelined Redis call to renew all leases in a single round trip (task 5.1).
    async fn keep_room_ownership(&self) {
        let room_codes = self.app.room_codes();
        if room_codes.is_empty() {
            return;
        }

        // Prepare rooms for pipelined renewal: (room_code, logic_node_id)
        let rooms: Vec<(String, String)> = room_codes.iter()
            .map(|c| (c.clone(), self.node_id.clone()))
            .collect();

        // Single pipelined round trip to renew all leases (task 5.1).
        let results = match self.broker.renew_room_leases_pipelined(&rooms).await {
            Ok(results) => results,
            Err(e) => {
                // Fallback to per-room renewal on a pipeline error (e.g. an
                // unexpected broker error). Log the cause once, then renew
                // each room individually so a sweep still makes progress.
                warn!(error = %e, "pipelined lease renewal failed; falling back to per-room renewal");
                for code in &room_codes {
                    match self.broker.renew_room_lease(code).await {
                        Ok(true) => {}
                        Ok(false) => {
                            if let Err(e) = self.broker.claim_room(code).await {
                                warn!(room_code = %code, error = %e, "failed to re-claim room");
                            } else {
                                warn!(room_code = %code, "re-claimed a room whose ownership lease was lost");
                            }
                        }
                        Err(e) => warn!(error = %e, "failed to renew room ownership"),
                    }
                }
                return;
            }
        };

        // For rooms where renewal failed (key lost or owned by another), re-claim.
        for i in 0..room_codes.len() {
            let code = &room_codes[i];
            let renewed = results[i];
            if !renewed {
                match self.broker.claim_room(code).await {
                    Ok(true) => {
                        warn!(room_code = %code, "re-claimed a room whose ownership lease was lost");
                    }
                    Ok(false) => {
                        let owner = self.broker.get_room_owner(code).await.ok().flatten();
                        if owner.as_deref() != Some(self.node_id.as_str()) {
                            warn!(room_code = %code, owner = ?owner, "another node owns a room this node holds");
                        }
                    }
                    Err(e) => {
                        warn!(room_code = %code, error = %e, "failed to re-claim room ownership");
                    }
                }
            }
        }
    }

    /// Graceful shutdown: release ownership of the rooms this node holds and drop
    /// its membership key, so a survivor is not blocked by a stale owner or a
    /// membership entry that looks alive (BUG-P3-2).
    pub async fn shutdown(&self) {
        for code in self.app.room_codes() {
            let _ = self.broker.release_room(&code).await;
        }
        let _ = self.broker.unregister_logic_node().await;
        info!(node_id = %self.node_id, "logic tier released its rooms and membership");
    }

    /// Dispatch one inbound envelope.
    async fn handle_inbound(&self, envelope: InboundEnvelope) {
        match envelope.payload {
            InboundPayload::Disconnect => {
                self.handle_disconnect(&envelope.connection_id).await;
            }
            InboundPayload::Client(message) => {
                self.handle_client(&envelope.connection_id, &envelope.node_id, message)
                    .await;
            }
        }
    }

    /// A transport socket closed: release or grace the seat, exactly as the
    /// monolith did on `RoomMsg::Detach` (design D5).
    ///
    /// If another live connection for the same device already holds the seat
    /// (the client reconnected before this socket's close was processed), the
    /// stale disconnect is ignored so it cannot unseat the fresh connection.
    async fn handle_disconnect(&self, connection_id: &str) {
        let shard = self.connections_shard(connection_id);
        let mut connections = shard.lock().await;
        let Some(conn) = connections.remove(connection_id) else {
            return;
        };
        let superseded = conn.room_code.is_some()
            && connections.values().any(|other| {
                other.room_code == conn.room_code
                    && other.identity.device_id() == conn.identity.device_id()
            });
        drop(connections);

        if superseded {
            return;
        }
        if let Some(mailbox) = conn.room_mailbox {
            let _ = mailbox.send(RoomMsg::Detach {
                identity: conn.identity,
            });
        }
    }

    /// Handle one client message for its connection.
    async fn handle_client(
        &self,
        connection_id: &str,
        transport_node_id: &str,
        message: ClientMessage,
    ) {
        let shard = self.connections_shard(connection_id);
        let mut connections = shard.lock().await;
        let logic_conn = connections
            .entry(connection_id.to_string())
            .or_insert_with(|| LogicConnection {
                connection_id: connection_id.to_string(),
                transport_node_id: transport_node_id.to_string(),
                room_mailbox: None,
                out_rx: None,
                // A device id is supplied by create/join; until then the
                // connection is an unidentified guest.
                identity: Identity::Guest {
                    device_id: connection_id.to_string(),
                },
                room_code: None,
            });
        logic_conn.transport_node_id = transport_node_id.to_string();

        match message {
            ClientMessage::CreateRoom {
                player_id,
                time_control,
                token,
                ..
            } => {
                self.handle_create_room(logic_conn, player_id, time_control, token)
                    .await;
            }
            ClientMessage::JoinRoom {
                player_id,
                room_code,
                token,
                ..
            } => {
                self.handle_join_room(logic_conn, player_id, room_code, token)
                    .await;
            }
            ClientMessage::Move { uci, .. } => self.handle_move(logic_conn, uci).await,
            ClientMessage::Resign { .. } => self.handle_resign(logic_conn).await,
            ClientMessage::Leave { .. } => self.handle_leave(logic_conn).await,
            ClientMessage::Register { .. }
            | ClientMessage::Login { .. }
            | ClientMessage::Logout { .. }
            | ClientMessage::SetProfile { .. } => self.handle_auth(logic_conn, message).await,
        }
    }

    async fn handle_create_room(
        &self,
        logic_conn: &mut LogicConnection,
        player_id: String,
        time_control: Option<String>,
        token: Option<String>,
    ) {
        if logic_conn.room_mailbox.is_some() {
            self.send_error(logic_conn, ErrorCode::AlreadyInRoom).await;
            return;
        }

        let time_control = time_control
            .as_deref()
            .and_then(TimeControl::parse)
            .unwrap_or(TimeControl::DEFAULT);

        let identity = self
            .resolve_identity_shared(&player_id, token.as_deref())
            .await;
        self.app.link_identity(&identity);
        logic_conn.identity = identity;

        match self.app.create_room_with_time_control(
            logic_conn.identity.clone(),
            time_control,
        ) {
            Ok(conn) => {
                // Phase 3: claim room ownership in Redis
                let room_code = conn.code.clone();
                if let Err(e) = self.broker.claim_room(&room_code).await {
                    tracing::warn!(room_code = %room_code, error = %e, "failed to claim room ownership");
                }
                self.adopt_room(logic_conn, conn);
            }
            Err(err) => {
                self.send_to_transport(
                    &logic_conn.connection_id,
                    &logic_conn.transport_node_id,
                    err,
                )
                .await;
            }
        }
    }

    async fn handle_join_room(
        &self,
        logic_conn: &mut LogicConnection,
        player_id: String,
        room_code: String,
        token: Option<String>,
    ) {
        if logic_conn.room_mailbox.is_some() {
            self.send_error(logic_conn, ErrorCode::AlreadyInRoom).await;
            return;
        }

        let identity = self
            .resolve_identity_shared(&player_id, token.as_deref())
            .await;
        self.app.link_identity(&identity);
        logic_conn.identity = identity;

        match self.app.join_room(logic_conn.identity.clone(), &room_code) {
            Ok(conn) => self.adopt_room(logic_conn, conn),
            Err(err) => {
                self.send_to_transport(
                    &logic_conn.connection_id,
                    &logic_conn.transport_node_id,
                    err,
                )
                .await;
            }
        }
    }

    /// Wire a freshly created/joined room connection to the transport: keep its
    /// mailbox and start draining its outbound stream to the broker.
    fn adopt_room(&self, logic_conn: &mut LogicConnection, conn: crate::infrastructure::server::Conn) {
        logic_conn.room_mailbox = Some(conn.mailbox);
        logic_conn.out_rx = Some(conn.out_rx);
        logic_conn.room_code = Some(conn.code);
        self.start_outbound_forwarder(logic_conn);
    }

    async fn handle_move(&self, logic_conn: &mut LogicConnection, uci: String) {
        if let Some(mailbox) = &logic_conn.room_mailbox {
            let _ = mailbox.send(RoomMsg::Move {
                identity: logic_conn.identity.clone(),
                uci,
            });
        } else {
            self.send_error(logic_conn, ErrorCode::NotConnected).await;
        }
    }

    async fn handle_resign(&self, logic_conn: &mut LogicConnection) {
        if let Some(mailbox) = &logic_conn.room_mailbox {
            let _ = mailbox.send(RoomMsg::Resign {
                identity: logic_conn.identity.clone(),
            });
        } else {
            self.send_error(logic_conn, ErrorCode::NotConnected).await;
        }
    }

    async fn handle_leave(&self, logic_conn: &mut LogicConnection) {
        if let Some(mailbox) = &logic_conn.room_mailbox {
            let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
            let _ = mailbox.send(RoomMsg::Leave {
                identity: logic_conn.identity.clone(),
                ack: Some(ack_tx),
            });
            let _ = ack_rx.await;
            logic_conn.room_mailbox = None;
            logic_conn.out_rx = None;
            logic_conn.room_code = None;
        }
    }

    /// Route an auth message to the authoritative `App` and send the reply
    /// back. The transport never decides auth (design D5/D11). A successful
    /// login also upgrades the connection's pinned identity.
    async fn handle_auth(&self, logic_conn: &mut LogicConnection, message: ClientMessage) {
        let reply = match message {
            ClientMessage::Register {
                username,
                password,
                device_id,
                ..
            } => {
                let reply = self.app.register(&username, &password, device_id).await;
                // Phase 3 (task 4.5): mirror the freshly issued session
                // to Redis so another logic node accepts the same token.
                self.mirror_if_session(&reply).await;
                reply
            }
            ClientMessage::Login {
                username,
                password,
                device_id,
                ..
            } => {
                let reply = self.app.login(&username, &password, device_id).await;
                self.mirror_if_session(&reply).await;
                reply
            }
            ClientMessage::Logout { token, .. } => {
                let reply = self.app.logout(&token);
                // Phase 3 (task 4.5): revoke the mirrored session so the
                // token is invalid on every logic node at once. When the
                // local store has no such session it was issued by another
                // logic instance: fall back to the shared mirror so logout
                // succeeds on any instance, not only the one that issued
                // the token.
                match reply {
                    ServerMessage::SessionOk { .. } => {
                        // Local logout succeeded; also revoke the
                        // mirrored session everywhere.
                        if let Err(e) =
                            self.broker.revoke_mirrored_session(&token).await
                        {
                            warn!(error = %e, "failed to revoke mirrored session");
                        }
                        reply
                    }
                    _ => {
                        match self
                            .broker
                            .lookup_mirrored_session(&token)
                            .await
                        {
                            Ok(Some((
                                account_id,
                                username,
                                display_name,
                                _expires_at_ms,
                            ))) => {
                                if let Err(e) = self
                                    .broker
                                    .revoke_mirrored_session(&token)
                                    .await
                                {
                                    warn!(error = %e, "failed to revoke mirrored session");
                                }
                                ServerMessage::SessionOk {
                                    v: VERSION,
                                    account_id,
                                    username,
                                    display_name,
                                    expires_at_ms: now_ms(),
                                }
                            }
                            _ => reply,
                        }
                    }
                }
            }
            ClientMessage::SetProfile {
                display_name, token, ..
            } => self.app.set_profile(&display_name, &token).await,
            _ => return,
        };

        // A login that succeeds mid-connection upgrades the pinned identity so
        // later moves use the account, and records the account->room binding so
        // the one-room-per-account rule applies from this instant (design D8;
        // mirrors ws.rs's `adopt_identity` call).
        if let ServerMessage::Session { account_id, .. } = &reply {
            logic_conn.identity = logic_conn.identity.authenticated_as(account_id);
            if let Some(code) = logic_conn.room_code.clone() {
                self.app.adopt_identity(&logic_conn.identity, &code);
            }
        }

        self.send_to_transport(
            &logic_conn.connection_id,
            &logic_conn.transport_node_id,
            reply,
        )
        .await;
    }

    /// Phase 3 (task 4.5): mirror a freshly issued session to the
    /// shared Redis store so another logic node accepts the same
    /// token. Best-effort: a failed mirror is logged, never fatal
    /// (the issuing node still holds the session in memory).
    async fn mirror_if_session(&self, reply: &ServerMessage) {
        if let ServerMessage::Session {
            token,
            account_id,
            username,
            display_name,
            expires_at_ms,
            ..
        } = reply
        {
            if let Err(e) = self
                .broker
                .mirror_session(
                    token,
                    account_id,
                    username,
                    display_name,
                    *expires_at_ms,
                )
                .await
            {
                warn!(error = %e, "failed to mirror session to Redis");
            }
        }
    }

    /// Resolve a connection's identity, preferring the shared Redis
    /// session mirror (Phase 3, task 4.5) so a session issued on
    /// another logic node is accepted here. Falls back to the
    /// in-memory store, which remains the authority for this
    /// node's own sessions and for the monolith path. The
    /// connection's device id is trusted over the session's
    /// recorded device (design D1/D9): the account is what
    /// follows, so a player may re-attach from a second device.
    async fn resolve_identity_shared(
        &self,
        player_id: &str,
        token: Option<&str>,
    ) -> Identity {
        if let Some(token) = token.filter(|t| !t.is_empty()) {
            match self.broker.lookup_mirrored_session(token).await {
                Ok(Some((account_id, _username, _display_name, _expires_at_ms))) => {
                    return Identity::Account {
                        account_id,
                        device_id: player_id.to_string(),
                    };
                }
                Ok(None) => {
                    // Not mirrored (guest, expired, or revoked
                    // everywhere): fall through to the local store.
                }
                Err(e) => {
                    warn!(error = %e, "Redis session lookup failed; using local store");
                }
            }
        }
        self.app.resolve_identity(player_id, token)
    }

    /// Forward a room actor's outbound stream to the owning transport node.
    fn start_outbound_forwarder(&self, logic_conn: &mut LogicConnection) {
        let Some(mut out_rx) = logic_conn.out_rx.take() else {
            return;
        };
        let connection_id = logic_conn.connection_id.clone();
        let transport_node_id = logic_conn.transport_node_id.clone();
        let broker = Arc::clone(&self.broker);

        tokio::spawn(async move {
            while let Some(message) = out_rx.recv().await {
                if !deliver_outbound(&broker, &connection_id, &transport_node_id, message).await {
                    // The owning node cannot be resolved for this connection
                    // anymore (stale/absent registry entry); the room stream
                    // can only be forwarded through it, so stop early instead
                    // of draining messages that can never be delivered.
                    break;
                }
            }
        });
    }

    async fn send_error(&self, logic_conn: &LogicConnection, code: ErrorCode) {
        let message = ServerMessage::Error {
            v: VERSION,
            code: code.as_str().to_string(),
            message: code.message().to_string(),
        };
        self.send_to_transport(
            &logic_conn.connection_id,
            &logic_conn.transport_node_id,
            message,
        )
        .await;
    }

    async fn send_to_transport(
        &self,
        connection_id: &str,
        transport_node_id: &str,
        message: ServerMessage,
    ) {
        deliver_outbound(&self.broker, connection_id, transport_node_id, message).await;
    }
}

/// Deliver one outbound message to the transport node that owns
/// `connection_id` (design D4 "Owning node resolution").
///
/// Resolution order:
/// 1. The node remembered for the connection (from the inbound envelope, or
///    the registry when that is not already known).
/// 2. If that publish reaches no subscriber (the node went away, or its id
///    was stale), the `ptc.conn.<id>` registry is consulted once more and the
///    message is re-routed to whatever node the registry now names.
/// 3. If no owning node can be resolved, the message is dropped: a stale
///    registry entry must not route messages after the disconnect.
///
/// Returns `false` when the Redis publish itself failed, so a room forwarder
/// can stop instead of spinning on a dead fabric.
async fn deliver_outbound(
    broker: &Broker,
    connection_id: &str,
    remembered_node: &str,
    message: ServerMessage,
) -> bool {
    // Primary route: the remembered owner, falling back to the registry when
    // the owner is not already known.
    let mut node_id = remembered_node.to_string();
    if node_id.is_empty() {
        match broker.lookup_connection_node(connection_id).await {
            Ok(Some(lookup)) => node_id = lookup,
            Ok(None) => {
                debug!(
                    connection_id,
                    "no owning node in the connection registry; dropping outbound message"
                );
                return true;
            }
            Err(e) => {
                warn!(error = %e, connection_id, "registry lookup failed; dropping message");
                return true;
            }
        }
    }

    let mut envelope = OutboundEnvelope {
        v: ENVELOPE_VERSION,
        node_id,
        connection_id: connection_id.to_string(),
        payload: message,
    };

    match broker.publish_outbound(&envelope).await {
        Ok(0) => {
            // No subscriber on that channel: the remembered owner is stale.
            // Re-resolve through the registry and retry once against the node
            // it names now (design D4: the registry is the stale-entry
            // fallback).
            match broker.lookup_connection_node(connection_id).await {
                Ok(Some(owner)) if owner != envelope.node_id => {
                    envelope.node_id = owner;
                    match broker.publish_outbound(&envelope).await {
                        Ok(0) => {
                            debug!(
                                connection_id,
                                "registry names a node with no subscriber; dropping stale message"
                            );
                            true
                        }
                        Ok(_) => true,
                        Err(e) => {
                            error!(error = %e, "failed to publish outbound envelope");
                            false
                        }
                    }
                }
                Ok(Some(_)) => {
                    debug!(
                        connection_id,
                        "owning node has no subscriber; dropping outbound message"
                    );
                    true
                }
                Ok(None) => {
                    debug!(
                        connection_id,
                        "stale registry entry dropped; no owning node routes this message"
                    );
                    true
                }
                Err(e) => {
                    warn!(error = %e, connection_id, "registry re-lookup failed; dropping message");
                    true
                }
            }
        }
        Ok(_) => true,
        Err(e) => {
            error!(error = %e, "failed to publish outbound envelope");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The room hash is deterministic: a room always maps to the same lane,
    /// which is what preserves per-room ordering across K lanes.
    #[test]
    fn room_lane_assignment_is_deterministic() {
        for code in ["ABC123", "XYZ789", "N3959S", "room-0001"] {
            let a = LogicState::hash_room_code(code) % 4;
            let b = LogicState::hash_room_code(code) % 4;
            assert_eq!(a, b, "room {code} must always map to the same lane");
        }
    }

    /// With several lanes, distinct rooms spread across lanes (so independent
    /// rooms are actually processed in parallel) and no single lane receives
    /// every room.
    #[test]
    fn rooms_spread_across_lanes() {
        const LANES: usize = 4;
        let mut counts = [0usize; LANES];
        for i in 0..400 {
            let code = format!("ROOM{i:04}");
            counts[LogicState::hash_room_code(&code) % LANES] += 1;
        }
        assert!(
            counts.iter().filter(|c| **c > 0).count() >= LANES - 1,
            "rooms should occupy most lanes, got {counts:?}"
        );
        assert!(
            counts.iter().all(|c| *c < 400),
            "a single lane must not receive every room: {counts:?}"
        );
    }

    /// Control-plane routing is keyed by connection id and is deterministic, so
    /// create-before-join ordering per connection is preserved.
    #[test]
    fn control_lane_assignment_is_deterministic() {
        for id in ["conn-1", "conn-2", "abc", "zzz"] {
            let a = LogicState::hash_connection_id(id) % 3;
            let b = LogicState::hash_connection_id(id) % 3;
            assert_eq!(a, b, "connection {id} must always map to the same lane");
        }
    }

    /// The TTL invariant assertion rejects a TTL that is too small for the
    /// configured sweep interval and max sweep duration (task 5.2).
    #[test]
    fn lease_ttl_exceeds_renewal_sweep() {
        let ttl = crate::infrastructure::broker::ownership::TTL_SECS as u64;
        let interval = ROOM_LEASE_RENEW_INTERVAL.as_secs();
        let sweep = ROOM_LEASE_RENEW_SWEEP_MAX.as_secs();
        assert!(ttl > interval + sweep,
            "default TTL ({ttl}) must exceed sweep_interval ({interval}) + sweep_max ({sweep})");
    }

    /// Lanes are bounded, so a full lane rejects further sends until the worker
    /// drains it (backpressure) rather than growing without limit (task 2.4).
    #[test]
    fn lane_capacity_is_bounded() {
        let (tx, _rx) = mpsc::channel::<u8>(LANE_CAPACITY);
        assert_eq!(tx.max_capacity(), LANE_CAPACITY);
        for _ in 0..LANE_CAPACITY {
            assert!(tx.try_send(0).is_ok(), "a lane must admit LANE_CAPACITY items");
        }
        assert!(
            tx.try_send(0).is_err(),
            "a full lane must reject further sends (backpressure), not grow unbounded"
        );
    }

    /// The connection registry is sharded, so the reader's per-message lane
    /// lookup and the lane workers do not serialize on one global lock, and
    /// connections spread across shards (task 3.1).
    #[test]
    fn lanes_do_not_serialize_on_connection_map() {
        assert!(CONNECTION_SHARDS > 1, "the connection map must be sharded");
        let total = CONNECTION_SHARDS * 50;
        let mut counts = [0usize; CONNECTION_SHARDS];
        for i in 0..total {
            let id = format!("conn-{i:05}");
            counts[LogicState::hash_connection_id(&id) % CONNECTION_SHARDS] += 1;
        }
        assert!(
            counts.iter().filter(|c| **c > 0).count() >= CONNECTION_SHARDS - 1,
            "connections should spread across shards, got {counts:?}"
        );
        assert!(
            counts.iter().all(|c| *c < total),
            "no single shard may hold every connection: {counts:?}"
        );
    }

    /// Control-plane work (auth, create room) and room-scoped work (moves) are
    /// routed to separate lane pools, so an in-flight auth cannot occupy the
    /// worker that would ack a move (task 4.2).
    #[test]
    fn control_and_move_messages_use_separate_pools() {
        let login = ClientMessage::Login {
            v: VERSION,
            username: "u".into(),
            password: "p".into(),
            device_id: None,
        };
        let create = ClientMessage::CreateRoom {
            v: VERSION,
            player_id: "d".into(),
            time_control: None,
            token: None,
        };
        let mv = ClientMessage::Move {
            v: VERSION,
            uci: "e2e4".into(),
        };
        let join = ClientMessage::JoinRoom {
            v: VERSION,
            player_id: "d".into(),
            room_code: "ROOM".into(),
            token: None,
        };
        assert_eq!(route_kind(&login, None), Route::Control);
        assert_eq!(route_kind(&create, None), Route::Control);
        assert_eq!(route_kind(&mv, Some("ROOM")), Route::Room);
        assert_eq!(route_kind(&join, None), Route::Room);
        // A move from a connection not seated in a room is not room-scoped.
        assert_eq!(route_kind(&mv, None), Route::Control);
    }

    /// Per-lane metrics expose the queue depth derived from the bounded sender
    /// plus the processing timings recorded by the worker (task 6.1).
    #[test]
    fn lane_metrics_are_exposed() {
        let (tx, _rx) = mpsc::channel::<LaneItem>(LANE_CAPACITY);
        let m = LaneMetrics::default();

        let empty = lane_metrics_json(&tx, &m);
        assert_eq!(empty["depth"], 0);
        assert_eq!(empty["processed"], 0);
        assert_eq!(empty["avg_us"], 0);

        let mk = || InboundEnvelope {
            v: ENVELOPE_VERSION,
            connection_id: "c1".into(),
            node_id: "n1".into(),
            payload: InboundPayload::Disconnect,
        };
        for _ in 0..3 {
            tx.try_send(LaneItem {
                envelope: mk(),
                routed_at: Instant::now(),
            })
            .unwrap();
        }
        assert_eq!(lane_metrics_json(&tx, &m)["depth"], 3, "depth is derived from the sender");

        m.record(120);
        m.record(80);
        let recorded = lane_metrics_json(&tx, &m);
        assert_eq!(recorded["processed"], 2);
        assert_eq!(recorded["last_us"], 80);
        assert_eq!(recorded["max_us"], 120);
        assert_eq!(recorded["avg_us"], 100);
    }
}
