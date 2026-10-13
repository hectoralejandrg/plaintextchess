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

use crate::application::room_actor::{RoomMsg, room_mailbox_max_depth};
use crate::domain::account::Identity;
use crate::domain::time_control::TimeControl;
use crate::infrastructure::broker::{
    Broker, InboundEnvelope, InboundPayload, OutboundEnvelope, ENVELOPE_VERSION,
    fabric_metrics,
};
use crate::infrastructure::password::now_ms;
use crate::infrastructure::server::App;
use crate::infrastructure::transport::{shard_index, Outbound, Outbox};
use crate::infrastructure::transport_handler::TransportState;
use crate::interface::protocol::{ClientMessage, ErrorCode, ServerMessage, VERSION, decode_incoming};

/// Backoff bounds and failure budget for broker reconnection (design D10).
/// QA finding: the old budget (10 failures, ~17s total) gave up and
/// self-unregistered (`unregister_logic_node`) well before a realistic
/// Redis restart/outage completed, so a node would delete its own
/// membership key and crash-exit mid-outage, relying entirely on an
/// external supervisor to come back as a brand-new node id.
///
/// A first pass raised this to 150 (~5 minutes), but a QA re-test that
/// literally followed this file's own "wait at least 5 minutes" guidance
/// found the node dying at exactly that boundary — a budget that *just*
/// meets a target survives *less than* it, not the target itself. 600
/// failures at this backoff shape budgets roughly 20 minutes (4 ramp-up
/// retries + ~596 steady 2s retries), comfortably past both the 30s lease
/// TTL (`membership::TTL_SECS`/`liveness::TTL_SECS`) and the real incident
/// that motivated this work (~11 minutes), with actual margin this time.
const RECONNECT_MIN: Duration = Duration::from_millis(200);
const RECONNECT_MAX: Duration = Duration::from_secs(2);
const MAX_SUBSCRIBE_FAILURES: u32 = 600;

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

/// High-water mark of a connection's outbound `mpsc::unbounded_channel` depth,
/// sampled on every receive in `start_outbound_forwarder`. The channel is
/// unbounded by design (a room actor must never block broadcasting), so this
/// stands in for backpressure visibility: if it climbs under load, the
/// forwarder/batcher is falling behind the room actor's output.
static OUT_CHANNEL_MAX_DEPTH: AtomicU64 = AtomicU64::new(0);

/// Maximum number of items in a single outbound batch (design D3). The batch is
/// bounded by item count; a byte cap was removed because it could not be
/// enforced without re-serializing each item (which design D4 avoids), and the
/// item cap already bounds the batch's size.
const MAX_BATCH_ITEMS: usize = 32;
/// Maximum time to wait for more messages before flushing a batch (design D3).
const BATCH_WINDOW: Duration = Duration::from_millis(1);
/// Maximum number of ready batches sent in one pipelined publish round trip
/// (design D2). Bounds in-flight work while amortizing the Redis round trip.
const PIPELINE_DEPTH: usize = 32;

/// A batch item submitted by a forwarder: the envelope plus the connection
/// ID it belongs to (needed for stale-route retry).
struct BatchItem {
    envelope: OutboundEnvelope,
    connection_id: String,
}

/// Representative shard count for the shard-distribution unit test below. The
/// real `NodeBatcher` sizes itself from `Broker::n_shards()` (one `BatchShard`
/// per Redis shard, design: shard the fabric), not this constant; it only
/// remains to exercise `shard_index`'s distribution at a realistic width.
#[cfg(test)]
const BATCH_SHARDS_PER_NODE: usize = 16;

/// One outbound publish shard: an independent task with its own bounded channel.
struct BatchShard {
    tx: mpsc::Sender<BatchItem>,
}

/// Per-node outbound batchers, sharded so publishes run in parallel (design D3).
struct NodeBatcher {
    node_id: String,
    shards: Vec<BatchShard>,
}

/// Decode a client frame exactly once and normalize it to the typed `Client`
/// form so the rest of the logic tier (lane worker + `handle_inbound`) never
/// decodes it again (design D4, "the logic decodes once"). An undecodable frame
/// is left as `ClientFrame` and routed to the control lane; `handle_inbound`
/// then drops it, matching the transport's own validation.
fn normalize_inbound_payload(payload: &mut InboundPayload) {
    let decoded = match payload {
        InboundPayload::ClientFrame { data } => decode_incoming(data).ok(),
        _ => None,
    };
    if let Some(message) = decoded {
        *payload = InboundPayload::Client(message);
    }
}

/// Whether a payload is a client move, used to record move-dispatch latency.
/// A `ClientFrame` is normalized to `Client` before reaching the worker, so the
/// `Client` arm is the one that matches on the transport hot path.
fn is_move_payload(payload: &InboundPayload) -> bool {
    matches!(
        payload,
        InboundPayload::Client(ClientMessage::Move { .. })
            | InboundPayload::LegacyClient {
                client: ClientMessage::Move { .. }
            }
    )
}

/// Collect the next batch of items (design D3): block for the first item, flush
/// a lone item immediately (no window), otherwise drain up to `BATCH_WINDOW` or
/// `MAX_BATCH_ITEMS`. Returns `None` when every sender has dropped, which ends
/// the batcher task. Extracted so the batching decision is unit-testable.
async fn next_batch(rx: &mut mpsc::Receiver<BatchItem>) -> Option<Vec<BatchItem>> {
    let first = rx.recv().await?;
    let mut items = vec![first];
    // Lone item: flush right away so it pays no batching window (design D3).
    match rx.try_recv() {
        Ok(second) => items.push(second),
        Err(_) => return Some(items), // channel empty (or disconnected): flush now
    }
    // Two or more: drain immediately-available items until the window closes or
    // the item cap is hit.
    let deadline = Instant::now() + BATCH_WINDOW;
    while items.len() < MAX_BATCH_ITEMS {
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        let wait = deadline - now;
        tokio::select! {
            item = rx.recv() => match item {
                Some(item) => items.push(item),
                None => break,
            },
            _ = tokio::time::sleep(wait) => break,
        }
    }
    Some(items)
}

/// Serialize one ready batch: a lone item as the unchanged single envelope, or
/// several items as one `OutboundBatch` for the node.
fn serialize_unit(node_id: &str, items: &[BatchItem]) -> String {
    if items.len() == 1 {
        serde_json::to_string(&items[0].envelope).unwrap_or_default()
    } else {
        let batch = crate::infrastructure::broker::OutboundBatch {
            v: ENVELOPE_VERSION,
            node_id: node_id.to_string(),
            items: items.iter().map(|i| i.envelope.clone()).collect(),
        };
        serde_json::to_string(&batch).unwrap_or_default()
    }
}

/// Re-resolve each message's owner through the registry and retry it once
/// against the node it names now (design D4: the registry is the stale-entry
/// fallback). Used when a publish reports zero subscribers.
async fn retry_stale(broker: &Broker, redis_shard: usize, items: Vec<BatchItem>) {
    for item in items {
        let mut envelope = item.envelope;
        let connection_id = item.connection_id;
        match broker.lookup_connection_node(&connection_id).await {
            Ok(Some(owner)) if owner != envelope.node_id => {
                envelope.node_id = owner;
                let _ = broker.publish_outbound(&envelope, redis_shard).await;
            }
            _ => {
                debug!(
                    connection_id = %connection_id,
                    "registry names no owner or same node; dropping stale message"
                );
            }
        }
    }
}

/// Publish one collected batch: a lone item as the unchanged single envelope, or
/// several items as one `OutboundBatch` for the node, with the stale-route retry.
async fn publish_batch(broker: &Broker, redis_shard: usize, items: Vec<BatchItem>) {
    if items.len() == 1 {
        let item = items.into_iter().next().expect("one item");
        let _ = deliver_outbound_envelope(broker, redis_shard, item).await;
        return;
    }
    let t_process = Instant::now();
    let node_id = items[0].envelope.node_id.clone();
    let batch = crate::infrastructure::broker::OutboundBatch {
        v: ENVELOPE_VERSION,
        node_id,
        items: items.iter().map(|i| i.envelope.clone()).collect(),
    };
    crate::infrastructure::broker::record_publish_process_ns(t_process.elapsed().as_nanos() as u64);

    let t_wait = Instant::now();
    let result = broker.publish_outbound_batch(&batch, redis_shard).await;
    crate::infrastructure::broker::record_publish_wait_ns(t_wait.elapsed().as_nanos() as u64);

    match result {
        Ok(0) => retry_stale(broker, redis_shard, items).await,
        Ok(_) => {}
        Err(e) => {
            error!(error = %e, "failed to publish outbound batch");
        }
    }
}

/// Publish a group of ready batches in one pipelined round trip (design D2), in
/// order, on `redis_shard`. Falls back to the per-batch path (with full retry
/// semantics) if the pipeline fails, and runs the stale-route retry for any
/// batch that reported zero subscribers.
async fn publish_units(broker: &Broker, node_id: &str, redis_shard: usize, units: Vec<Vec<BatchItem>>) {
    if units.len() == 1 {
        publish_batch(broker, redis_shard, units.into_iter().next().expect("one unit")).await;
        return;
    }
    let t_process = Instant::now();
    let payloads: Vec<String> = units.iter().map(|items| serialize_unit(node_id, items)).collect();
    crate::infrastructure::broker::record_publish_process_ns(t_process.elapsed().as_nanos() as u64);

    let t_wait = Instant::now();
    let result = broker.publish_outbound_pipelined(node_id, redis_shard, &payloads).await;
    crate::infrastructure::broker::record_publish_wait_ns(t_wait.elapsed().as_nanos() as u64);

    match result {
        Ok(counts) if counts.len() == units.len() => {
            for (items, count) in units.into_iter().zip(counts) {
                if count == 0 {
                    retry_stale(broker, redis_shard, items).await;
                }
            }
        }
        _ => {
            warn!(
                node_id = %node_id,
                "pipelined outbound publish failed; falling back to per-batch publish"
            );
            for items in units {
                publish_batch(broker, redis_shard, items).await;
            }
        }
    }
}

/// Collect up to `PIPELINE_DEPTH` ready batches: block for the first (design D3
/// window), then drain immediately-available items into further batches without
/// waiting. Returns `None` when every sender has dropped.
async fn collect_units(rx: &mut mpsc::Receiver<BatchItem>) -> Option<Vec<Vec<BatchItem>>> {
    let first = next_batch(rx).await?;
    let mut units = vec![first];
    while units.len() < PIPELINE_DEPTH {
        match rx.try_recv() {
            Ok(item) => {
                let mut batch = vec![item];
                while batch.len() < MAX_BATCH_ITEMS {
                    match rx.try_recv() {
                        Ok(next) => batch.push(next),
                        Err(_) => break,
                    }
                }
                units.push(batch);
            }
            Err(_) => break,
        }
    }
    Some(units)
}

impl BatchShard {
    /// Spawn one publish shard for a node, permanently bound to `redis_shard`
    /// (the Redis instance its key range maps to - design: shard the fabric).
    fn spawn(node_id: String, broker: Arc<Broker>, redis_shard: usize) -> Self {
        let (tx, mut rx) = mpsc::channel(1024);
        tokio::spawn(async move {
            // The task lives as long as any sender holds this shard; when the
            // last drops, `collect_units` returns `None` and the task ends.
            while let Some(units) = collect_units(&mut rx).await {
                publish_units(&broker, &node_id, redis_shard, units).await;
            }
        });
        Self { tx }
    }

    /// Submit a message to this shard. Awaits when the channel is full, so a
    /// saturated shard applies backpressure instead of dropping the message
    /// (design D3: no loss under load).
    async fn submit(&self, connection_id: String, envelope: OutboundEnvelope) {
        if self
            .tx
            .send(BatchItem {
                envelope,
                connection_id,
            })
            .await
            .is_err()
        {
            crate::infrastructure::broker::record_outbound_drop();
            warn!("outbound batcher shard is gone; dropping message");
        }
    }
}

impl NodeBatcher {
    /// Spawn the sharded batcher for `node_id`: one `BatchShard` per Redis
    /// shard, each permanently bound to that shard's connection.
    fn spawn(node_id: String, broker: Arc<Broker>) -> Arc<Self> {
        let n_shards = broker.n_shards();
        let shards = (0..n_shards)
            .map(|redis_shard| BatchShard::spawn(node_id.clone(), Arc::clone(&broker), redis_shard))
            .collect();
        Arc::new(Self { node_id, shards })
    }

    /// The shard for a key: a connection's own id when it is not room-scoped, or
    /// its room code so that both seats of a game share a shard.
    fn shard_for(&self, key: &str) -> &BatchShard {
        &self.shards[shard_index(key, self.shards.len())]
    }

    /// Submit an outbound message for `shard_key`, backpressured (never dropped
    /// while the shard is alive).
    async fn submit(&self, shard_key: &str, connection_id: String, envelope: OutboundEnvelope) {
        self.shard_for(shard_key).submit(connection_id, envelope).await;
    }
}

/// Deliver one outbound envelope to the transport node, with the
/// stale-route retry logic (extracted from the old `deliver_outbound`).
async fn deliver_outbound_envelope(
    broker: &Broker,
    redis_shard: usize,
    item: BatchItem,
) -> bool {
    let mut envelope = item.envelope;
    let connection_id = item.connection_id;

    match broker.publish_outbound(&envelope, redis_shard).await {
        Ok(0) => {
            // No subscriber on that channel: the remembered owner is stale.
            // Re-resolve through the registry and retry once against the node
            // it names now (design D4: the registry is the stale-entry
            // fallback).
            match broker.lookup_connection_node(&connection_id).await {
                Ok(Some(owner)) if owner != envelope.node_id => {
                    envelope.node_id = owner;
                    match broker.publish_outbound(&envelope, redis_shard).await {
                        Ok(0) => {
                            debug!(
                                connection_id = %connection_id,
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
                        connection_id = %connection_id,
                        "owning node has no subscriber; dropping outbound message"
                    );
                    true
                }
                Ok(None) => {
                    debug!(
                        connection_id = %connection_id,
                        "stale registry entry dropped; no owning node routes this message"
                    );
                    true
                }
                Err(e) => {
                    warn!(error = %e, connection_id = %connection_id, "registry re-lookup failed; dropping message");
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
        | ClientMessage::SetProfile { .. }
        | ClientMessage::JoinTournament { .. }
        | ClientMessage::LeaveTournament { .. }
        | ClientMessage::ListTournaments { .. } => Route::Control,
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
    /// Per-node outbound batcher for this connection's transport node (design D3).
    batcher: Option<Arc<NodeBatcher>>,
}

/// Shard count for partitioned connections map (matches room lanes for simplicity).
const CONNECTION_SHARDS: usize = 16;

/// Sharded connections map to avoid a single global lock.
/// Each shard is an independent AsyncMutex<HashMap>.
type ConnectionsShards = Vec<AsyncMutex<HashMap<String, LogicConnection>>>;

/// Sharded batcher map to avoid a single global lock.
/// Shard = hash(node_id) % BATCHER_SHARDS.
/// Uses `Weak` references so a batcher whose task has ended (all its senders
/// dropped) is recreated on the next `get_batcher`.
type BatcherShards = Vec<AsyncMutex<HashMap<String, std::sync::Weak<NodeBatcher>>>>;
const BATCHER_SHARDS: usize = 16;

/// Shared state for the logic tier.
pub struct LogicState {
    app: Arc<App>,
    broker: Arc<Broker>,
    node_id: String,
    /// Sharded connection maps to avoid a single global lock (task 3.1).
    /// Shard = hash(connection_id) % CONNECTION_SHARDS.
    connections: ConnectionsShards,
    /// Sharded per-transport-node batchers (design D3).
    batchers: BatcherShards,
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
    /// `TIER=combined` only: the `TransportState` sharing this process, set
    /// once by `run_combined` right after both are constructed. `None` for a
    /// real `TIER=logic` process, which always publishes to the fabric
    /// (colocate-room-hot-path design D3).
    local_transport: std::sync::OnceLock<Arc<TransportState>>,
    /// `run()`'s long-lived background tasks (lane workers, lease renewal,
    /// room-ownership keep-alive, transport-liveness watcher), tracked so
    /// `abort_background_tasks` can stop all of them. In a real process
    /// these run for the process's lifetime and are never aborted; test
    /// harnesses that construct and tear down many `LogicState`s in one
    /// binary need this (QA finding: untracked `tokio::spawn`s outlived
    /// their test and kept rewriting shared test Redis keys for their TTL
    /// window into the next test).
    background_tasks: std::sync::Mutex<tokio::task::JoinSet<()>>,
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
            batchers: (0..BATCHER_SHARDS)
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
            local_transport: std::sync::OnceLock::new(),
            background_tasks: std::sync::Mutex::new(tokio::task::JoinSet::new()),
        }
    }

    /// Stop every background task `run()` spawned (lane workers, lease
    /// renewal, room-ownership keep-alive, transport-liveness watcher).
    /// Production processes exit as a whole and never call this; test
    /// harnesses that tear down a `LogicState` mid-binary must, so the next
    /// test's shared Redis keys/ports are not touched by a zombie task.
    pub fn abort_background_tasks(&self) {
        self.background_tasks.lock().unwrap().abort_all();
    }

    /// Get the node ID.
    pub fn get_node_id(&self) -> &str {
        &self.node_id
    }

    /// `TIER=combined` only: wire this logic state to the `TransportState`
    /// sharing its process, so the outbound forwarder can skip the fabric for
    /// a connection served by this same process (colocate-room-hot-path).
    /// Called once by `run_combined`; a second call is a no-op.
    pub fn set_local_transport(&self, transport: Arc<TransportState>) {
        let _ = self.local_transport.set(transport);
    }

    /// Handle one inbound frame in-process, skipping the fabric publish and
    /// the inbound lane (colocate-room-hot-path task 3.1). The frame arrives
    /// undecoded (`ClientFrame`) exactly as a fabric envelope would, so it is
    /// normalized the same way `route_inbound` normalizes a fabric envelope
    /// before dispatch.
    pub async fn handle_inbound_local(&self, mut envelope: InboundEnvelope) {
        normalize_inbound_payload(&mut envelope.payload);
        self.handle_inbound(envelope).await;
    }

    /// Get or create a batcher for the given transport node.
    async fn get_batcher(&self, node_id: &str) -> Arc<NodeBatcher> {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        node_id.hash(&mut hasher);
        let shard_idx = (hasher.finish() as usize) % BATCHER_SHARDS;

        let mut map = self.batchers[shard_idx].lock().await;
        // Try to upgrade existing Weak reference; if dead or missing, create new.
        if let Some(weak) = map.get(node_id) {
            if let Some(batcher) = weak.upgrade() {
                return batcher;
            }
            // Dead batcher: remove stale entry.
            map.remove(node_id);
        }
        let batcher = NodeBatcher::spawn(node_id.to_string(), Arc::clone(&self.broker));
        map.insert(node_id.to_string(), Arc::downgrade(&batcher));
        batcher
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
        let (inbound_pubs, outbound_pubs, outbound_batches, inbound_bytes, outbound_bytes) = fabric_metrics();
        let (publish_wait_ns, publish_process_ns) = crate::infrastructure::broker::fabric_timing_metrics();
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
            "fabric_metrics": {
                "inbound_publishes": inbound_pubs,
                "outbound_publishes": outbound_pubs,
                "outbound_batches_published": outbound_batches,
                "inbound_bytes": inbound_bytes,
                "outbound_bytes": outbound_bytes,
                "outbound_drops": crate::infrastructure::broker::outbound_drops(),
                "outbound_publish_wait_ns": publish_wait_ns,
                "outbound_publish_process_ns": publish_process_ns,
            },
            "channel_depth": {
                "room_mailbox_max_depth": room_mailbox_max_depth(),
                "out_channel_max_depth": OUT_CHANNEL_MAX_DEPTH.load(Ordering::Relaxed),
            }
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

        // Subscribe to our inbound channel on every Redis shard immediately so
        // we're ready to receive messages before spawning background tasks
        // (helps with test harness timing).
        let n_shards = self.broker.n_shards();
        let mut initial_pubsubs = Vec::with_capacity(n_shards);
        for shard in 0..n_shards {
            initial_pubsubs.push(self.broker.subscribe_inbound_self(shard).await?);
        }

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
            self.background_tasks.lock().unwrap().spawn(async move {
                info!(node_id = %state.node_id, lane_id, "room lane worker started");
                while let Some(item) = lane_receiver.recv().await {
                    let is_move = is_move_payload(&item.envelope.payload);
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
            self.background_tasks.lock().unwrap().spawn(async move {
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
        // the lease is harmless in Phase 1). QA finding: a fixed 10s
        // interval meant a renewal failing right after Redis went down
        // waited the full 10s for its next chance, even if Redis came back
        // a second later — measured as a 10-18s end-to-end recovery time
        // after a full Redis restart, dominating far more than the publish-
        // health probe (<250ms) or the control-shard timeout. Backing off
        // on failure (reusing `reconnect_backoff`, same shape as the
        // subscribe-retry loop below) bounds that to ~2s instead.
        let lease_state = Arc::clone(&self);
        self.background_tasks.lock().unwrap().spawn(async move {
            let mut failures: u32 = 0;
            loop {
                match lease_state.broker.renew_logic_node_lease().await {
                    Ok(_) => {
                        failures = 0;
                        tokio::time::sleep(Duration::from_secs(10)).await;
                    }
                    Err(e) => {
                        warn!(error = %e, "failed to renew logic node lease");
                        failures += 1;
                        tokio::time::sleep(reconnect_backoff(failures)).await;
                    }
                }
            }
        });

        // Keep this node's room ownership alive: refresh the lease, and re-claim
        // any room whose key was lost (DEL, expiry, or a broker restart wiped the
        // keyspace). A lost key must never leave an active room ownerless
        // (BUG-P3-1).
        let rooms_state = Arc::clone(&self);
        self.background_tasks.lock().unwrap().spawn(async move {
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
        self.background_tasks.lock().unwrap().spawn(async move {
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

        // Reader fan-out (design D1/D3): one reader task per Redis shard (sharded
        // fabric follow-up), each parsing envelopes and routing them to bounded
        // lanes exactly as the single reader did before sharding. The reader is
        // the backpressure point: `route_inbound` awaits a lane when it is full,
        // so memory is bounded and no envelope is dropped. A saturated lane
        // backpressures the readers feeding it (documented limitation, design
        // D3). Per-room ordering is always preserved. `run()` treats any one
        // shard's permanent failure as the whole node's failure (same "exit
        // non-zero so a supervisor notices" behavior as before sharding).
        let mut readers = Vec::with_capacity(n_shards);
        for (shard, pubsub) in initial_pubsubs.into_iter().enumerate() {
            readers.push(tokio::spawn(Arc::clone(&self).run_inbound_reader(shard, pubsub)));
        }
        let (result, _index, _rest) = futures_util::future::select_all(readers).await;
        match result {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(e.into()),
            Err(join_err) => Err(format!("inbound reader task panicked: {join_err}").into()),
        }
    }

    /// One Redis shard's inbound reader: parses envelopes from `pubsub` and
    /// routes them to bounded lanes, reconnecting with bounded backoff when the
    /// subscription is lost. Returns `Err` only after exhausting the reconnect
    /// budget for this shard.
    async fn run_inbound_reader(
        self: Arc<Self>,
        shard: usize,
        mut pubsub: redis::aio::PubSub,
    ) -> Result<(), String> {
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
                    // Route to appropriate lane (design D1/D2). The function handles
                    // lane selection and sending internally.
                    self.route_inbound(envelope).await;
                }
            } // messages stream dropped here, releasing borrow on pubsub

            // Subscription ended (broker lost); reconnect within failure budget.
            error!(node_id = %self.node_id, shard, "inbound subscription ended (broker lost); reconnecting");
            loop {
                failures += 1;
                if failures >= MAX_SUBSCRIBE_FAILURES {
                    // Give up: drop the membership key so this node is not
                    // mistaken for a live logic node, then report failure (the
                    // caller exits non-zero so a supervisor notices).
                    let _ = self.broker.unregister_logic_node().await;
                    return Err(format!("shard {shard} broker unavailable after {failures} attempts"));
                }
                tokio::time::sleep(reconnect_backoff(failures)).await;
                match self.broker.subscribe_inbound_self(shard).await {
                    Ok(new_pubsub) => {
                        info!(node_id = %self.node_id, shard, "broker reconnected: inbound subscription restored");
                        failures = 0;
                        pubsub = new_pubsub;
                        continue 'reader;
                    }
                    Err(e) => {
                        warn!(error = %e, shard, failures, "failed to re-subscribe to the inbound channel; retrying");
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
    async fn route_inbound(&self, mut envelope: InboundEnvelope) {
        // Decode the client frame once and normalize it so downstream stages do
        // not decode again (design D4); this also keeps move-dispatch counting
        // accurate for the transport hot path, which always sends `ClientFrame`.
        normalize_inbound_payload(&mut envelope.payload);
        let lane_sender = match &envelope.payload {
            InboundPayload::Client(message) => {
                let seated = self.connection_room_code(&envelope.connection_id).await;
                Self::route_lane(message, seated.as_deref(), &envelope.connection_id, self)
            }
            InboundPayload::LegacyClient { client } => {
                let seated = self.connection_room_code(&envelope.connection_id).await;
                Self::route_lane(client, seated.as_deref(), &envelope.connection_id, self)
            }
            InboundPayload::ClientFrame { .. } => {
                // Only reachable when the frame was undecodable: route it to
                // control so `handle_inbound` can drop it as before.
                self.control_lane_for_connection(&envelope.connection_id)
            }
            InboundPayload::Disconnect => {
                match self.connection_room_code(&envelope.connection_id).await {
                    Some(room_code) => self.room_lane_for_room(&room_code),
                    None => self.control_lane_for_connection(&envelope.connection_id),
                }
            }
            InboundPayload::LegacyDisconnect(_) => {
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

    /// Compute the lane for a typed client message (extracted to avoid duplication
    /// between `Client` and `ClientFrame` paths).
    fn route_lane<'a>(
        message: &ClientMessage,
        seated_room: Option<&str>,
        connection_id: &str,
        state: &'a LogicState,
    ) -> &'a LaneSender {
        match route_kind(message, seated_room) {
            Route::Room => match message {
                ClientMessage::JoinRoom { room_code, .. } => state.room_lane_for_room(room_code),
                _ => {
                    let room = seated_room
                        .expect("a room-scoped route requires the connection to be seated");
                    state.room_lane_for_room(room)
                }
            },
            Route::Control => state.control_lane_for_connection(connection_id),
        }
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
            InboundPayload::LegacyDisconnect(_) => {
                self.handle_disconnect(&envelope.connection_id).await;
            }
            InboundPayload::Client(message) => {
                self.handle_client(&envelope.connection_id, &envelope.node_id, message)
                    .await;
            }
            InboundPayload::LegacyClient { client } => {
                self.handle_client(&envelope.connection_id, &envelope.node_id, client)
                    .await;
            }
            InboundPayload::ClientFrame { .. } => {
                // `route_inbound` already decoded this frame once; a payload
                // still in `ClientFrame` form is undecodable and is dropped,
                // matching the transport's own validation (no second decode).
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
                batcher: None,
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
            // Arena tournaments (add-tournaments phase 1) are monolith-only
            // (`TIER=all`, see `ws.rs`): the split tier has no tournament
            // registry to route these to. Explicit non-goal of this phase,
            // not an oversight — reject cleanly instead of panicking.
            ClientMessage::JoinTournament { .. }
            | ClientMessage::LeaveTournament { .. }
            | ClientMessage::ListTournaments { .. } => {
                self.send_error(logic_conn, ErrorCode::TournamentNotFound).await;
            }
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
                // colocate-room-hot-path task 2.1: record the creating seat's
                // transport node, so an external LB can route the second seat
                // to it (nothing in this repo reads this back yet).
                if let Err(e) = self
                    .broker
                    .set_room_transport(&room_code, &logic_conn.transport_node_id)
                    .await
                {
                    tracing::warn!(room_code = %room_code, error = %e, "failed to record room transport locality");
                }
                self.adopt_room(logic_conn, conn).await;
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
            Ok(conn) => self.adopt_room(logic_conn, conn).await,
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
    async fn adopt_room(&self, logic_conn: &mut LogicConnection, conn: crate::infrastructure::server::Conn) {
        logic_conn.room_mailbox = Some(conn.mailbox);
        logic_conn.out_rx = Some(conn.out_rx);
        logic_conn.room_code = Some(conn.code);
        // Get or create the batcher for this connection's transport node.
        logic_conn.batcher = Some(self.get_batcher(&logic_conn.transport_node_id).await);
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
    /// Messages are submitted to the node's sharded batcher, keyed by the room
    /// so a game's two seats share a shard and coalesce (design D3).
    fn start_outbound_forwarder(&self, logic_conn: &mut LogicConnection) {
        let Some(mut out_rx) = logic_conn.out_rx.take() else {
            return;
        };
        let connection_id = logic_conn.connection_id.clone();
        // Room-scoped messages shard by room; a connection is always seated for
        // the lifetime of this forwarder, so the key is stable.
        let shard_key = logic_conn
            .room_code
            .clone()
            .unwrap_or_else(|| connection_id.clone());
        let batcher = match logic_conn.batcher.take() {
            Some(b) => b,
            None => return,
        };
        // colocate-room-hot-path task 3.1: a connection's transport node does
        // not change for the forwarder's lifetime (the batcher above already
        // relies on that same invariant), so decide local-vs-fabric once here
        // instead of per message. `None` on a real `TIER=logic` process or a
        // connection served by another node (task 3.3 fallback).
        let local_outbox: Option<Arc<Outbox>> = self
            .local_transport
            .get()
            .filter(|t| t.node_id() == logic_conn.transport_node_id)
            .and_then(|t| t.outbox_for(&connection_id));

        tokio::spawn(async move {
            while let Some(message) = out_rx.recv().await {
                OUT_CHANNEL_MAX_DEPTH.fetch_max(out_rx.len() as u64, Ordering::Relaxed);
                if let Some(outbox) = &local_outbox {
                    outbox.push(Outbound::Text(message));
                    continue;
                }
                let envelope = OutboundEnvelope {
                    v: ENVELOPE_VERSION,
                    node_id: batcher.node_id.clone(),
                    connection_id: connection_id.clone(),
                    payload: message,
                };
                batcher
                    .submit(&shard_key, connection_id.clone(), envelope)
                    .await;
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
        let batcher = self.get_batcher(transport_node_id).await;
        let envelope = OutboundEnvelope {
            v: ENVELOPE_VERSION,
            node_id: transport_node_id.to_string(),
            connection_id: connection_id.to_string(),
            payload: message,
        };
        // Control/error messages are not room-scoped: shard by connection.
        batcher
            .submit(connection_id, connection_id.to_string(), envelope)
            .await;
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

    // -- Batcher and inbound-normalization tests (optimize-fabric-message-path) --

    /// A batch item for `conn`.
    fn batch_item(conn: &str, code: &str) -> BatchItem {
        BatchItem {
            envelope: OutboundEnvelope {
                v: ENVELOPE_VERSION,
                node_id: "transport-abc".to_string(),
                connection_id: conn.to_string(),
                payload: ServerMessage::Error {
                    v: VERSION,
                    code: code.to_string(),
                    message: code.to_string(),
                },
            },
            connection_id: conn.to_string(),
        }
    }

    /// Task 3.1 (real): two seats on one node are collected into ONE batch, so
    /// the logic publishes a single fabric message instead of one per seat.
    #[tokio::test]
    async fn two_seats_one_fabric_message() {
        let (tx, mut rx) = mpsc::channel::<BatchItem>(MAX_BATCH_ITEMS);
        tx.try_send(batch_item("conn-white", "w")).unwrap();
        tx.try_send(batch_item("conn-black", "b")).unwrap();

        let items = next_batch(&mut rx).await.expect("a batch");

        assert_eq!(items.len(), 2, "two seats must be collected into one batch");
        assert_eq!(items[0].envelope.connection_id, "conn-white");
        assert_eq!(items[1].envelope.connection_id, "conn-black");
    }

    /// Design D3: a lone message is flushed immediately (it does not wait for
    /// the batching window).
    #[tokio::test]
    async fn lone_message_flushes_immediately() {
        let (tx, mut rx) = mpsc::channel::<BatchItem>(MAX_BATCH_ITEMS);
        tx.try_send(batch_item("conn-a", "a")).unwrap();

        let started = Instant::now();
        let items = next_batch(&mut rx).await.expect("a batch");
        assert_eq!(items.len(), 1, "a lone message is its own batch of one");
        assert!(
            started.elapsed() < BATCH_WINDOW,
            "a lone message must not wait for the {BATCH_WINDOW:?} window"
        );
    }

    /// The batcher task ends only when every sender has dropped (no idle timer).
    #[tokio::test]
    async fn next_batch_ends_when_all_senders_drop() {
        let (tx, mut rx) = mpsc::channel::<BatchItem>(MAX_BATCH_ITEMS);
        drop(tx);
        assert!(
            next_batch(&mut rx).await.is_none(),
            "no senders left means the batcher task should end"
        );
    }

    /// Sharding by room keeps a game's two seats in the same shard (so they
    /// coalesce) while distinct rooms spread across shards (so publishes run in
    /// parallel).
    #[test]
    fn room_seats_share_a_shard_and_rooms_spread() {
        let k = BATCH_SHARDS_PER_NODE;
        // Both seats of one game carry the same room code -> same shard.
        assert_eq!(shard_index("ROOM1", k), shard_index("ROOM1", k));
        // Distinct rooms spread across shards.
        let mut seen = std::collections::HashSet::new();
        for i in 0..400 {
            seen.insert(shard_index(&format!("ROOM{i:04}"), k));
        }
        assert!(
            seen.len() >= k - 1,
            "rooms should occupy most shards, got {}",
            seen.len()
        );
    }

    /// Pipelining groups ready batches but never reorders their messages.
    #[tokio::test]
    async fn collect_units_preserves_order() {
        let (tx, mut rx) = mpsc::channel::<BatchItem>(64);
        for (conn, code) in [
            ("conn-a", "a1"),
            ("conn-b", "b1"),
            ("conn-a", "a2"),
            ("conn-b", "b2"),
        ] {
            tx.try_send(batch_item(conn, code)).unwrap();
        }
        let units = collect_units(&mut rx).await.expect("units");
        let flat: Vec<String> = units
            .iter()
            .flatten()
            .map(|i| i.envelope.connection_id.clone())
            .collect();
        assert_eq!(flat, vec!["conn-a", "conn-b", "conn-a", "conn-b"]);
        assert!(units.len() <= 4, "at least as compact as one unit per item");
    }

    /// A unit serializes to the same wire forms the transport already decodes:
    /// a lone item as a single envelope, several as one `OutboundBatch`.
    #[test]
    fn serialize_unit_single_and_batch() {
        let single = vec![batch_item("conn-a", "x")];
        let json = serialize_unit("transport-abc", &single);
        let env: OutboundEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(env.connection_id, "conn-a");

        let batch = vec![batch_item("conn-a", "x"), batch_item("conn-b", "y")];
        let json = serialize_unit("transport-abc", &batch);
        match serde_json::from_str::<crate::infrastructure::broker::OutboundWire>(&json).unwrap() {
            crate::infrastructure::broker::OutboundWire::Batch(b) => {
                assert_eq!(b.items.len(), 2);
            }
            _ => panic!("expected a batch"),
        }
    }

    /// H1 regression: the transport hot path sends `ClientFrame`, and the lane
    /// worker must still count it as a move (after normalization) so
    /// `move_dispatch` is accurate.
    #[test]
    fn client_frame_move_is_counted_after_normalization() {
        let mut payload = InboundPayload::ClientFrame {
            data: r#"{"v":1,"type":"move","uci":"e2e4"}"#.to_string(),
        };
        assert!(!is_move_payload(&payload), "a raw frame is not classified before normalization");
        normalize_inbound_payload(&mut payload);
        assert!(matches!(payload, InboundPayload::Client(ClientMessage::Move { .. })));
        assert!(is_move_payload(&payload), "a move sent as ClientFrame must be counted");
    }

    /// A non-move frame is normalized but not counted as a move.
    #[test]
    fn client_frame_non_move_is_not_counted() {
        let mut payload = InboundPayload::ClientFrame {
            data: r#"{"v":1,"type":"create_room","player_id":"p1"}"#.to_string(),
        };
        normalize_inbound_payload(&mut payload);
        assert!(matches!(payload, InboundPayload::Client(ClientMessage::CreateRoom { .. })));
        assert!(!is_move_payload(&payload));
    }

    /// An undecodable frame stays as `ClientFrame` (and is later dropped).
    #[test]
    fn undecodable_client_frame_stays_a_frame() {
        let mut payload = InboundPayload::ClientFrame {
            data: "not json".to_string(),
        };
        normalize_inbound_payload(&mut payload);
        assert!(matches!(payload, InboundPayload::ClientFrame { .. }));
        assert!(!is_move_payload(&payload));
    }
}
