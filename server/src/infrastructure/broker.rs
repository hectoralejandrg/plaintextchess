//! Redis event fabric for the split realtime architecture (design D2-D4).
//!
//! Provides versioned envelopes, channel naming, and connection addressing
//! for communication between the transport tier and the logic tier.

use std::sync::RwLock;

use redis::{
    aio::{ConnectionManager, ConnectionManagerConfig, MultiplexedConnection}, AsyncCommands,
    Client as RedisClient, ExistenceCheck, SetExpiry, SetOptions,
};
use serde::{Deserialize, Serialize};

use crate::application::ports::{RatingMirror, RatingState, RecorderError};
use crate::infrastructure::password::{hash_token, now_ms};
use crate::infrastructure::transport::shard_index;
use crate::interface::protocol::{ClientMessage, ServerMessage};

/// Version of the cross-tier envelope format (design D3). Distinct from the
/// client wire `VERSION`: an envelope may evolve independently of the client
/// protocol.
///
/// v1: original format (externally tagged InboundPayload with Client(ClientMessage) as bare variant)
/// v2: adjacent-tagged InboundPayload with explicit "type" field; supports both v1 and v2
///     during rolling deploy via untagged deserialization.
pub const ENVELOPE_VERSION: u16 = 2;

/// The well-known logic channel target in Phase 1, where exactly one logic
/// instance is authoritative (design D4/D6). Phase 3 replaces this with
/// per-room ownership routing.
pub const DEFAULT_LOGIC_NODE: &str = "logic";

/// How many times a publish is retried across a transient connection failure,
/// and the pause between attempts. A freshly restarted broker leaves the
/// connection manager holding a dead connection; its first command can fail
/// (`broken pipe`) before it reconnects (BUG-R1-F1).
const PUBLISH_ATTEMPTS: u32 = 3;
const PUBLISH_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(100);

/// Internal metrics counters (task 1.1).
static INBOUND_PUBLISHES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static OUTBOUND_PUBLISHES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static OUTBOUND_BATCHES_PUBLISHED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static INBOUND_BYTES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static OUTBOUND_BYTES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// Outbound messages dropped because the per-node batcher channel was full
/// (design D3 drop-on-full policy).
static OUTBOUND_DROPS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Time an outbound batch shard spends awaiting a publish reply from Redis
/// (gate task 1.1 of `pipeline-fabric-publishes`), summed across every shard.
static OUTBOUND_PUBLISH_WAIT_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// Time an outbound batch shard spends building the batch/payloads before
/// publishing (serialization, not I/O), summed across every shard.
static OUTBOUND_PROCESS_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Reset all fabric metrics counters.
pub fn reset_fabric_metrics() {
    INBOUND_PUBLISHES.store(0, std::sync::atomic::Ordering::Relaxed);
    OUTBOUND_PUBLISHES.store(0, std::sync::atomic::Ordering::Relaxed);
    OUTBOUND_BATCHES_PUBLISHED.store(0, std::sync::atomic::Ordering::Relaxed);
    INBOUND_BYTES.store(0, std::sync::atomic::Ordering::Relaxed);
    OUTBOUND_BYTES.store(0, std::sync::atomic::Ordering::Relaxed);
    OUTBOUND_DROPS.store(0, std::sync::atomic::Ordering::Relaxed);
    OUTBOUND_PUBLISH_WAIT_NS.store(0, std::sync::atomic::Ordering::Relaxed);
    OUTBOUND_PROCESS_NS.store(0, std::sync::atomic::Ordering::Relaxed);
}

/// Record time spent awaiting a publish reply (gate task 1.1).
pub fn record_publish_wait_ns(ns: u64) {
    OUTBOUND_PUBLISH_WAIT_NS.fetch_add(ns, std::sync::atomic::Ordering::Relaxed);
}

/// Record time spent building a batch/payload before publishing (gate task 1.1).
pub fn record_publish_process_ns(ns: u64) {
    OUTBOUND_PROCESS_NS.fetch_add(ns, std::sync::atomic::Ordering::Relaxed);
}

/// Snapshot of outbound publish timing: (wait_ns, process_ns), summed across
/// every outbound batch shard since the last reset.
pub fn fabric_timing_metrics() -> (u64, u64) {
    (
        OUTBOUND_PUBLISH_WAIT_NS.load(std::sync::atomic::Ordering::Relaxed),
        OUTBOUND_PROCESS_NS.load(std::sync::atomic::Ordering::Relaxed),
    )
}

/// Record an outbound message dropped because the batcher channel was full.
pub fn record_outbound_drop() {
    OUTBOUND_DROPS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// Number of outbound messages dropped so far (batcher channel full).
pub fn outbound_drops() -> u64 {
    OUTBOUND_DROPS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Snapshot of current fabric metrics.
pub fn fabric_metrics() -> (u64, u64, u64, u64, u64) {
    (
        INBOUND_PUBLISHES.load(std::sync::atomic::Ordering::Relaxed),
        OUTBOUND_PUBLISHES.load(std::sync::atomic::Ordering::Relaxed),
        OUTBOUND_BATCHES_PUBLISHED.load(std::sync::atomic::Ordering::Relaxed),
        INBOUND_BYTES.load(std::sync::atomic::Ordering::Relaxed),
        OUTBOUND_BYTES.load(std::sync::atomic::Ordering::Relaxed),
    )
}

/// What a transport node forwards to the logic tier (design D3/D5).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum InboundPayload {
    /// A typed client message (v2+ wire form): directly the ClientMessage.
    /// ClientMessage is internally tagged with "type" (snake_case).
    Client(ClientMessage),
    /// Legacy v1 wire form: InboundPayload was externally tagged with
    /// rename_all="snake_case", so Client(ClientMessage) serialized as
    /// {"client": <ClientMessage>}. Kept for rollout compatibility.
    LegacyClient { client: ClientMessage },
    /// The original client frame text, already validated by the transport.
    /// The logic tier decodes it once (design D4).
    ClientFrame { data: String },
    /// Legacy v1 disconnect: the externally tagged unit variant serialized as
    /// the bare string "disconnect". Kept for rollout compatibility.
    LegacyDisconnect(String),
    /// The client's socket closed. The logic tier MUST release or grace the
    /// seat exactly as the monolith's `RoomMsg::Detach` did.
    Disconnect,
}

/// Versioned envelope for inbound messages (transport -> logic).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboundEnvelope {
    /// Envelope version (`ENVELOPE_VERSION`).
    pub v: u16,
    /// Unique connection identifier assigned by the transport node at accept time.
    pub connection_id: String,
    /// Transport node identifier that holds this connection.
    pub node_id: String,
    /// The forwarded client message, or a disconnect control event.
    pub payload: InboundPayload,
}

/// Versioned envelope for outbound messages (logic -> transport).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboundEnvelope {
    /// Envelope version (`ENVELOPE_VERSION`).
    pub v: u16,
    /// Target transport node identifier.
    pub node_id: String,
    /// Target connection identifier.
    pub connection_id: String,
    /// The server message payload.
    pub payload: ServerMessage,
}

/// A batch of outbound envelopes for one transport node (design D2).
/// Internal fabric message: the transport fans the items out to their
/// connections' outboxes as if they had arrived individually.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboundBatch {
    /// Envelope version (`ENVELOPE_VERSION`).
    pub v: u16,
    /// The target transport node identifier (all items share it).
    pub node_id: String,
    /// Per-connection messages, in publish order.
    pub items: Vec<OutboundEnvelope>,
}

/// The wire form of an outbound fabric message: either a single envelope
/// or a batch. Untagged preserves the exact existing single-envelope JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OutboundWire {
    /// A single per-connection message (v1 wire form).
    Envelope(OutboundEnvelope),
    /// A batch of per-connection messages for one node (design D2).
    Batch(OutboundBatch),
}

/// Channel name constants for the Redis pub/sub fabric.
pub mod channels {
    /// Inbound messages from transport to a specific logic node.
    /// Format: `ptc.ws.in.{logic_node_id}`
    pub fn inbound_to_logic(logic_node_id: &str) -> String {
        format!("ptc.ws.in.{}", logic_node_id)
    }

    /// Outbound messages from logic to a specific transport node.
    /// Format: `ptc.ws.out.{transport_node_id}`
    pub fn outbound_to_transport(transport_node_id: &str) -> String {
        format!("ptc.ws.out.{}", transport_node_id)
    }

    /// Room-scoped broadcasts (all transport nodes subscribe).
    /// Format: `ptc.room.{room_code}`
    pub fn room(room_code: &str) -> String {
        format!("ptc.room.{}", room_code)
    }

    /// Site-wide broadcasts (all transport nodes subscribe).
    pub const SITE: &str = "ptc.site";

    /// Transport node membership heartbeats.
    /// Format: `ptc.node.{node_id}`
    pub fn node_heartbeat(node_id: &str) -> String {
        format!("ptc.node.{}", node_id)
    }
}

/// Redis-backed connection registry: maps connection_id -> (node_id, session_info).
/// Used in Phase 2+ for cross-node delivery.
pub mod registry {
    use super::*;
    use redis::AsyncCommands;

    /// Prefix for connection registry keys.
    const PREFIX: &str = "ptc.conn.";

    /// TTL for connection registry entries (slightly longer than reconnect grace).
    pub const TTL_SECS: u64 = 180;

    /// Register a connection with its owning transport node.
    pub async fn register(
        conn: &mut ConnectionManager,
        connection_id: &str,
        node_id: &str,
        session_info: Option<&str>,
    ) -> redis::RedisResult<()> {
        let key = format!("{}{}", PREFIX, connection_id);
        let value = serde_json::json!({
            "node_id": node_id,
            "session_info": session_info,
            "registered_at": now_ms(),
        });
        conn.set_ex(key, serde_json::to_string(&value).unwrap(), TTL_SECS).await
    }

    /// Lookup the transport node owning a connection.
    pub async fn lookup_node(conn: &mut ConnectionManager, connection_id: &str) -> redis::RedisResult<Option<String>> {
        let key = format!("{}{}", PREFIX, connection_id);
        let value: Option<String> = conn.get(key).await?;
        if let Some(v) = value {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&v) {
                return Ok(json.get("node_id").and_then(|n| n.as_str().map(|s| s.to_string())));
            }
        }
        Ok(None)
    }

    /// Remove a connection from the registry.
    pub async fn unregister(conn: &mut ConnectionManager, connection_id: &str) -> redis::RedisResult<()> {
        let key = format!("{}{}", PREFIX, connection_id);
        conn.del(key).await
    }
}

/// Room ownership map: maps room_code -> logic_node_id.
/// Used in Phase 3 for room routing.
pub mod ownership {
    use super::*;
    use redis::AsyncCommands;
    use redis::Script;

    /// Prefix for room ownership keys.
    const PREFIX: &str = "ptc.room_owner.";

    /// TTL for room ownership (must be renewed by the logic node).
    pub const TTL_SECS: u64 = 60;

    /// Lua body that renews one ownership lease if (and only if) we still own it.
    const RENEW_SCRIPT: &str = r#"
        if redis.call("get", KEYS[1]) == ARGV[1] then
            return redis.call("expire", KEYS[1], ARGV[2])
        else
            return 0
        end
    "#;

    /// Claim ownership of a room.
    pub async fn claim(
        conn: &mut ConnectionManager,
        room_code: &str,
        logic_node_id: &str,
    ) -> redis::RedisResult<bool> {
        let key = format!("{}{}", PREFIX, room_code);
        let opts = SetOptions::default()
            .conditional_set(ExistenceCheck::NX)
            .with_expiration(SetExpiry::EX(TTL_SECS));
        conn.set_options(key, logic_node_id, opts).await
    }

    /// Release ownership of a room (only if we own it).
    pub async fn release(
        conn: &mut ConnectionManager,
        room_code: &str,
        logic_node_id: &str,
    ) -> redis::RedisResult<bool> {
        let key = format!("{}{}", PREFIX, room_code);
        let script = Script::new(r#"
            if redis.call("get", KEYS[1]) == ARGV[1] then
                return redis.call("del", KEYS[1])
            else
                return 0
            end
        "#);
        let result: i32 = script.key(key).arg(logic_node_id).invoke_async(conn).await?;
        Ok(result == 1)
    }

    /// Get the current owner of a room.
    pub async fn get_owner(conn: &mut ConnectionManager, room_code: &str) -> redis::RedisResult<Option<String>> {
        let key = format!("{}{}", PREFIX, room_code);
        conn.get(key).await
    }

    /// Renew ownership lease (only if we own it).
    pub async fn renew(
        conn: &mut ConnectionManager,
        room_code: &str,
        logic_node_id: &str,
    ) -> redis::RedisResult<bool> {
        let key = format!("{}{}", PREFIX, room_code);
        let script = Script::new(r#"
            if redis.call("get", KEYS[1]) == ARGV[1] then
                return redis.call("expire", KEYS[1], ARGV[2])
            else
                return 0
            end
        "#);
        let result: i32 = script.key(key).arg(logic_node_id).arg(TTL_SECS).invoke_async(conn).await?;
        Ok(result == 1)
    }

    /// Renew multiple ownership leases in a single pipeline (design D4).
    ///
    /// Returns a vec of success (`true` = renewed, `false` = key lost or owned
    /// by another node) in the same order as the input rooms. The Lua body is
    /// sent via `EVALSHA` so a sweep is one round trip; if Redis reports
    /// `NOSCRIPT` (e.g. its script cache was flushed by a restart), the script
    /// is loaded once and the pipeline is retried, so the steady-state path
    /// stays pipelined instead of falling back to one call per room.
    pub async fn renew_pipelined(
        conn: &mut ConnectionManager,
        rooms: &[(String, String)], // (room_code, logic_node_id)
    ) -> redis::RedisResult<Vec<bool>> {
        if rooms.is_empty() {
            return Ok(Vec::new());
        }
        let script = Script::new(RENEW_SCRIPT);
        let results: Vec<i32> = match build_renew_pipeline(&script, rooms).query_async(conn).await {
            Ok(results) => results,
            Err(e) if e.kind() == redis::ErrorKind::NoScriptError => {
                script.prepare_invoke().load_async(conn).await?;
                build_renew_pipeline(&script, rooms).query_async(conn).await?
            }
            Err(e) => return Err(e),
        };
        Ok(results.into_iter().map(|r| r == 1).collect())
    }

    /// Build the pipelined `EVALSHA` command for a batch of rooms.
    fn build_renew_pipeline(script: &Script, rooms: &[(String, String)]) -> redis::Pipeline {
        let mut pipe = redis::pipe();
        for (room_code, logic_node_id) in rooms {
            let key = format!("{}{}", PREFIX, room_code);
            pipe.cmd("EVALSHA")
                .arg(script.get_hash())
                .arg(1)
                .arg(key)
                .arg(logic_node_id)
                .arg(TTL_SECS);
        }
        pipe
    }
}

/// Records which transport node holds a room's first seat (colocate-room-hot-path,
/// task 2.1). Informational only: nothing in this repo reads it back yet, since
/// the in-process colocated hot path decides locality per-connection
/// (`LogicConnection.transport_node_id` vs the local node's own id), not by
/// looking up this key. It exists so an external load balancer can consistently
/// route a room's second seat to the same transport node as the first (design
/// D2's open question), without this server building that LB itself.
pub mod room_transport {
    use super::*;
    use redis::AsyncCommands;

    /// Prefix for room-transport-locality keys.
    const PREFIX: &str = "ptc.room_transport.";

    /// TTL matches room ownership: both describe the same room's lifetime.
    pub const TTL_SECS: u64 = ownership::TTL_SECS;

    /// Record the transport node serving a room's first seat. Write-only
    /// bookkeeping: unconditionally overwrites, since only the creating seat
    /// ever calls this (the second seat only reads, if anything ever does).
    pub async fn set(
        conn: &mut ConnectionManager,
        room_code: &str,
        transport_node_id: &str,
    ) -> redis::RedisResult<()> {
        let key = format!("{}{}", PREFIX, room_code);
        let opts = SetOptions::default().with_expiration(SetExpiry::EX(TTL_SECS));
        conn.set_options(key, transport_node_id, opts).await
    }

    /// Read the transport node recorded for a room, if any.
    pub async fn get(conn: &mut ConnectionManager, room_code: &str) -> redis::RedisResult<Option<String>> {
        let key = format!("{}{}", PREFIX, room_code);
        conn.get(key).await
    }
}

/// Shared session mirror (Phase 3, task 4.5): a logic node that issues
/// a session mirrors it to Redis so another logic node accepts the same
/// token. SQLite remains the durable mirror (written by the issuing
/// node via `App::issue_session`); Redis is the shared, fast lookup.
/// Only the token's SHA-256 hash is stored - the plaintext token never
/// leaves the node that issued it (design D6).
pub mod session {
    use super::*;
    use redis::AsyncCommands;

    /// Prefix for mirrored session keys.
    const PREFIX: &str = "ptc.session.";

    /// The shared form of a session: just enough for another logic node
    /// to resolve the identity (the account it belongs to, the account's
    /// reported names, and when it stops being usable). Revocation is
    /// expressed by deleting the key.
    #[derive(Serialize, Deserialize)]
    struct MirroredSession {
        account_id: String,
        username: String,
        display_name: String,
        expires_at_ms: i64,
    }

    /// Mirror a freshly issued session. The Redis key's TTL matches the
    /// session's remaining lifetime so an expired session disappears on
    /// its own (design D7: expiry evaluated at lookup time, no sweeper).
    pub async fn store(
        conn: &mut ConnectionManager,
        token_hash: &str,
        account_id: &str,
        username: &str,
        display_name: &str,
        expires_at_ms: i64,
    ) -> redis::RedisResult<()> {
        let key = format!("{}{}", PREFIX, token_hash);
        let remaining = (expires_at_ms - now_ms()).max(1) as u64;
        let value = MirroredSession {
            account_id: account_id.to_string(),
            username: username.to_string(),
            display_name: display_name.to_string(),
            expires_at_ms,
        };
        let payload = serde_json::to_string(&value).map_err(serialize_error)?;
        conn.set_ex(key, payload, remaining).await
    }

    /// Load a mirrored session if it is still usable (not expired).
    /// Returns the account id, the account's username, its display
    /// name, and the stored expiry.
    pub async fn load(
        conn: &mut ConnectionManager,
        token_hash: &str,
    ) -> redis::RedisResult<Option<(String, String, String, i64)>> {
        let key = format!("{}{}", PREFIX, token_hash);
        let value: Option<String> = conn.get(key).await?;
        let Some(v) = value else { return Ok(None) };
        let mirrored: MirroredSession =
            serde_json::from_str(&v).map_err(serialize_error)?;
        if now_ms() >= mirrored.expires_at_ms {
            return Ok(None);
        }
        Ok(Some((
            mirrored.account_id,
            mirrored.username,
            mirrored.display_name,
            mirrored.expires_at_ms,
        )))
    }

    /// Remove a mirrored session (logout / revocation), so the token is
    /// revoked on every logic node at once.
    pub async fn remove(
        conn: &mut ConnectionManager,
        token_hash: &str,
    ) -> redis::RedisResult<()> {
        let key = format!("{}{}", PREFIX, token_hash);
        let _: () = conn.del(key).await?;
        Ok(())
    }
}

/// Cross-node rating mirror (split-realtime multi-logic-node follow-up):
/// fixes a correctness gap, not a performance one — each logic node keeps
/// its own SQLite file (design D7 of `split-realtime-architecture`), so
/// without this a player whose games land on different nodes would show a
/// different rating per node. The mirror is a plain Redis hash (not JSON)
/// so the last-write-wins guard below never needs to parse a payload.
/// SQLite remains each node's durable history; Redis is the cross-node
/// source of freshness (same split as `session` above).
pub mod rating {
    use super::*;
    use redis::AsyncCommands;
    use redis::Script;

    /// Prefix for mirrored rating keys.
    const PREFIX: &str = "ptc.rating.";

    /// Overwrite only if the incoming `updated_at_ms` is newer than what's
    /// stored, so a slow node's stale write can never clobber a fresher one
    /// already mirrored by a faster node.
    const STORE_SCRIPT: &str = r#"
        local current = redis.call("HGET", KEYS[1], "updated_at_ms")
        if (not current) or tonumber(ARGV[4]) > tonumber(current) then
            redis.call("HSET", KEYS[1], "rating", ARGV[1], "rating_deviation", ARGV[2], "volatility", ARGV[3], "updated_at_ms", ARGV[4])
            return 1
        else
            return 0
        end
    "#;

    /// Mirror a device's rating, guarded by `updated_at_ms`. No TTL: unlike
    /// a session, a rating never expires.
    pub async fn store(
        conn: &mut ConnectionManager,
        device_id: &str,
        state: RatingState,
        updated_at_ms: i64,
    ) -> redis::RedisResult<()> {
        let key = format!("{}{}", PREFIX, device_id);
        let script = Script::new(STORE_SCRIPT);
        let _: i32 = script
            .key(key)
            .arg(state.rating)
            .arg(state.rating_deviation)
            .arg(state.volatility)
            .arg(updated_at_ms)
            .invoke_async(conn)
            .await?;
        Ok(())
    }

    /// Load the mirrored rating, if any logic node has ever recorded one for
    /// this device.
    pub async fn load(
        conn: &mut ConnectionManager,
        device_id: &str,
    ) -> redis::RedisResult<Option<RatingState>> {
        let key = format!("{}{}", PREFIX, device_id);
        let fields: std::collections::HashMap<String, String> = conn.hgetall(key).await?;
        if fields.is_empty() {
            return Ok(None);
        }
        let parse = |name: &str| fields.get(name).and_then(|v| v.parse::<f64>().ok()).unwrap_or_default();
        Ok(Some(RatingState {
            rating: parse("rating"),
            rating_deviation: parse("rating_deviation"),
            volatility: parse("volatility"),
        }))
    }
}

/// Logic node membership: each logic node registers with a lease.
pub mod membership {
    use super::*;
    use redis::AsyncCommands;
    use redis::Script;

    /// Prefix for logic node membership keys.
    const PREFIX: &str = "ptc.logic_node.";

    /// TTL for logic node lease.
    pub const TTL_SECS: u64 = 30;

    /// `EXISTS` + `SET EX` as one atomic round trip (QA finding: the
    /// previous two-command version had a TOCTOU window — see `renew`).
    const RENEW_SCRIPT: &str = r#"
        local existed = redis.call("EXISTS", KEYS[1])
        redis.call("SET", KEYS[1], ARGV[1], "EX", ARGV[2])
        return existed
    "#;

    /// Register this logic node with a lease.
    pub async fn register(
        conn: &mut ConnectionManager,
        node_id: &str,
        metadata: Option<&str>,
    ) -> redis::RedisResult<()> {
        let key = format!("{}{}", PREFIX, node_id);
        let value = serde_json::json!({
            "metadata": metadata,
            "registered_at": now_ms(),
        });
        conn.set_ex(key, serde_json::to_string(&value).unwrap(), TTL_SECS).await
    }

    /// Renew the lease for this logic node, self-healing if the key was lost
    /// (e.g. a broker restart). Returns whether the key already existed
    /// (BUG-R1-F2: `EXPIRE` on a missing key is a silent no-op). `EXISTS` and
    /// `SET EX` run as a single Lua script (not two round trips): the
    /// previous two-command version could read `EXISTS` right as a renewal
    /// from a concurrent caller (or this same loop's next tick, under
    /// scheduling delay) raced the `SET EX`, misreporting a live key as
    /// lapsed (QA finding, confirmed as a real false-positive source after a
    /// full Redis restart — one per node per outage, not a repeating flap).
    pub async fn renew(conn: &mut ConnectionManager, node_id: &str) -> redis::RedisResult<bool> {
        let key = format!("{}{}", PREFIX, node_id);
        let value = serde_json::json!({
            "renewed_at": now_ms(),
        });
        let script = Script::new(RENEW_SCRIPT);
        let existed: i32 = script
            .key(key)
            .arg(serde_json::to_string(&value).unwrap())
            .arg(TTL_SECS)
            .invoke_async(conn)
            .await?;
        Ok(existed == 1)
    }

    /// Check if a logic node is alive.
    pub async fn is_alive(conn: &mut ConnectionManager, node_id: &str) -> redis::RedisResult<bool> {
        let key = format!("{}{}", PREFIX, node_id);
        conn.exists(key).await
    }

    /// All registered logic node ids. May include a node whose key has not yet
    /// expired; routing probes each node's subscriber count, so a stale id is
    /// skipped rather than trusted.
    pub async fn all_nodes(conn: &mut ConnectionManager) -> redis::RedisResult<Vec<String>> {
        let pattern = format!("{}*", PREFIX);
        let keys: Vec<String> = conn.keys(pattern).await?;
        Ok(keys
            .into_iter()
            .filter_map(|key| key.strip_prefix(PREFIX).map(str::to_string))
            .collect())
    }
}

/// Transport node liveness: each transport node renews a `ptc.node.<node_id>` lease.
/// When the lease expires (node crashes without publishing disconnect), the logic
/// tier treats that node's registered connections as disconnected (design D11).
pub mod liveness {
    use super::*;
    use redis::AsyncCommands;
    use redis::Script;

    /// Prefix for transport node liveness keys. Matches `channels::node_heartbeat`.
    const PREFIX: &str = "ptc.node.";

    /// TTL for transport node lease (seconds).
    pub const TTL_SECS: u64 = 30;

    /// `EXISTS` + `SET EX` as one atomic round trip (same TOCTOU fix as
    /// `membership::RENEW_SCRIPT` — see that module for the full rationale).
    const RENEW_SCRIPT: &str = r#"
        local existed = redis.call("EXISTS", KEYS[1])
        redis.call("SET", KEYS[1], ARGV[1], "EX", ARGV[2])
        return existed
    "#;

    /// Register this transport node with a lease.
    pub async fn register(
        conn: &mut ConnectionManager,
        node_id: &str,
    ) -> redis::RedisResult<()> {
        let key = format!("{}{}", PREFIX, node_id);
        let value = serde_json::json!({
            "registered_at": now_ms(),
        });
        conn.set_ex(key, serde_json::to_string(&value).unwrap(), TTL_SECS).await
    }

    /// Renew the lease for this transport node. Returns whether the key already
    /// existed: `false` means the lease had lapsed while this node was alive (a
    /// pause longer than the TTL, or lost keys), so the caller must treat its
    /// connections as void (design D11). `EXISTS` and `SET EX` run as a single
    /// Lua script (self-heals the key either way), not two round trips — the
    /// previous version had a TOCTOU window that could misreport a live key as
    /// lapsed (QA finding).
    pub async fn renew(conn: &mut ConnectionManager, node_id: &str) -> redis::RedisResult<bool> {
        let key = format!("{}{}", PREFIX, node_id);
        let value = serde_json::json!({
            "renewed_at": now_ms(),
        });
        let script = Script::new(RENEW_SCRIPT);
        let existed: i32 = script
            .key(key)
            .arg(serde_json::to_string(&value).unwrap())
            .arg(TTL_SECS)
            .invoke_async(conn)
            .await?;
        Ok(existed == 1)
    }

    /// Check if a transport node's lease is alive.
    pub async fn is_alive(conn: &mut ConnectionManager, node_id: &str) -> redis::RedisResult<bool> {
        let key = format!("{}{}", PREFIX, node_id);
        conn.exists(key).await
    }
}

/// One Redis shard's connections. Mirrors what `Broker` held as a single set
/// before sharding; every shard gets its own `client`/`manager`/`publish_conn`
/// so publishing/subscribing on one shard never contends with another.
struct ShardConn {
    client: RedisClient,
    /// Shared multiplexed connection for commands (registry, leases, ...). It
    /// reconnects automatically but arms a per-command response timer, so it is
    /// NOT used on the hot publish path.
    manager: ConnectionManager,
    /// Dedicated connection for the hot publish path. It is a plain
    /// `MultiplexedConnection` (no per-command response timeout), so publishing
    /// does not arm a tokio timer per message and does not contend on the shared
    /// timer wheel (design D1). Reconnected lazily on a connection error.
    publish_conn: RwLock<Option<MultiplexedConnection>>,
}

/// Broker client for publishing/subscribing to the Redis fabric, sharded
/// across N independent Redis instances (colocate-room-hot-path follow-up:
/// a single shared Redis becomes the throughput ceiling once more than one
/// node pair relies on it, measured directly - see
/// `openspec/changes/.../design.md` "shard the fabric" section). High-volume
/// per-move traffic is routed to a shard by room/connection key; low-volume
/// lifecycle bookkeeping (room ownership, registries, leases, sessions)
/// always uses `shards[0]`, the "control shard" - it's low enough traffic
/// that it doesn't need splitting, and keeping it on one instance avoids a
/// cross-shard consistency problem for things like "list every logic node".
pub struct Broker {
    shards: Vec<ShardConn>,
    node_id: String,
}

/// Bound on how long a control-shard command waits for Redis before failing
/// (QA finding: `redis-rs`'s own default is unbounded). Well above normal
/// command latency, far below "stalls the room actor until Redis comes
/// back" — a caller treats the timeout exactly like any other Redis error.
const COMMAND_RESPONSE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

impl Broker {
    /// Connect to every Redis shard and build each one's shared command
    /// connection. Fails if any shard is unreachable, which is how "refuse to
    /// start without Redis" (spec `realtime-transport`) is enforced at the
    /// call site, now per shard.
    pub async fn connect(redis_urls: &[String], node_id: String) -> redis::RedisResult<Self> {
        if redis_urls.is_empty() {
            return Err(redis::RedisError::from((
                redis::ErrorKind::InvalidClientConfig,
                "Broker::connect needs at least one Redis URL",
            )));
        }
        let mut shards = Vec::with_capacity(redis_urls.len());
        for url in redis_urls {
            let client = RedisClient::open(url.as_str())?;
            // The manager auto-reconnects and is used for everything except the
            // hot publish path (see `publish_connection`). `redis-rs` defaults
            // `response_timeout` to `Duration::MAX` (QA finding: a Redis
            // outage mid-command would otherwise stall the caller
            // indefinitely — e.g. the room actor awaiting `RatingMirror` before
            // it can deliver a terminal result to either player). Bounding it
            // here covers every control-shard operation at once (ownership,
            // registry, leases, sessions, rating), not just the new one.
            let manager = ConnectionManager::new_with_config(
                client.clone(),
                ConnectionManagerConfig::new().set_response_timeout(COMMAND_RESPONSE_TIMEOUT),
            )
            .await?;
            shards.push(ShardConn {
                client,
                manager,
                publish_conn: RwLock::new(None),
            });
        }
        Ok(Self { shards, node_id })
    }

    /// Get the node identifier.
    pub fn node_id(&self) -> &str {
        &self.node_id
    }

    /// Number of Redis shards this broker spreads traffic across.
    pub fn n_shards(&self) -> usize {
        self.shards.len()
    }

    /// The shard a room/connection-scoped key routes to. Shared helper so
    /// callers (the outbound batcher, inbound publish) agree on the mapping.
    pub fn shard_for(&self, key: &str) -> usize {
        shard_index(key, self.shards.len())
    }

    /// The control shard: every low-volume lifecycle operation (room
    /// ownership, registries, leases, sessions) lives here, not split.
    fn conn(&self) -> ConnectionManager {
        self.shards[0].manager.clone()
    }

    /// A no-per-command-timeout connection for the hot publish path on one
    /// shard, (re)opened lazily. A plain `MultiplexedConnection` is created
    /// with no response timeout, so publishing never arms a tokio timer and
    /// does not contend on the shared timer wheel (design D1). If the cached
    /// connection is gone (broker restart), the next call reconnects.
    async fn publish_connection(&self, shard: usize) -> redis::RedisResult<MultiplexedConnection> {
        if let Some(conn) = self.shards[shard].publish_conn.read().unwrap().clone() {
            return Ok(conn);
        }
        let conn = self.shards[shard].client.get_multiplexed_async_connection().await?;
        *self.shards[shard].publish_conn.write().unwrap() = Some(conn.clone());
        Ok(conn)
    }

    /// Probe one shard's inbound publish connection health (QA finding: this
    /// connection deliberately carries no per-command timeout — design D1 of
    /// `reduce-fabric-contention` — so it cannot detect a stalled, not just a
    /// dead, Redis on its own). The caller wraps this in a bounded timeout on
    /// an interval, not per inbound message, to avoid reintroducing the
    /// per-command timer cost that design eliminated.
    pub async fn ping_publish_shard(&self, shard: usize) -> redis::RedisResult<()> {
        let mut conn = self.publish_connection(shard).await?;
        let result = redis::cmd("PING").query_async(&mut conn).await;
        if result.is_err() {
            // QA finding: without this, a failed probe kept retrying the
            // same broken cached connection forever — recovery only
            // happened once a real client publish hit `publish_with_retry`'s
            // own reconnect-on-error path. Drop the cache here too, so the
            // next probe tick reconnects on its own (same pattern as
            // `publish_with_retry`'s error arm below).
            *self.shards[shard].publish_conn.write().unwrap() = None;
        }
        result
    }

    /// Publish `payload` on `channel` via `shard`, retrying a transient
    /// connection failure. Returns the subscriber count. A broker restart
    /// kills the cached publish connection, so the first attempt may fail and
    /// the next reconnects (design D10 / BUG-R1-F1).
    async fn publish_with_retry(
        &self,
        shard: usize,
        channel: String,
        payload: String,
    ) -> redis::RedisResult<i64> {
        let mut last_err: Option<redis::RedisError> = None;
        for attempt in 0..PUBLISH_ATTEMPTS {
            let mut conn = match self.publish_connection(shard).await {
                Ok(conn) => conn,
                Err(err) => {
                    last_err = Some(err);
                    if attempt + 1 < PUBLISH_ATTEMPTS {
                        tokio::time::sleep(PUBLISH_RETRY_DELAY).await;
                    }
                    continue;
                }
            };
            match conn
                .publish::<_, _, i64>(channel.clone(), payload.clone())
                .await
            {
                Ok(subscribers) => return Ok(subscribers),
                Err(err) => {
                    // Drop the cached connection so the next attempt reconnects.
                    *self.shards[shard].publish_conn.write().unwrap() = None;
                    last_err = Some(err);
                }
            }
            if attempt + 1 < PUBLISH_ATTEMPTS {
                tokio::time::sleep(PUBLISH_RETRY_DELAY).await;
            }
        }
        Err(last_err.expect("at least one publish attempt runs"))
    }

    /// Publish an inbound envelope to the Phase 1 logic channel. Returns the
    /// number of subscriber connections that received it; 0 means no logic tier
    /// is currently subscribed (it may still be resubscribing), so the caller
    /// must not treat a 0-count publish as delivered (BUG-F1-R1).
    pub async fn publish_inbound(&self, envelope: &InboundEnvelope) -> redis::RedisResult<i64> {
        let channel = channels::inbound_to_logic(DEFAULT_LOGIC_NODE);
        let payload = serde_json::to_string(envelope).map_err(serialize_error)?;
        INBOUND_PUBLISHES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        INBOUND_BYTES.fetch_add(payload.len() as u64, std::sync::atomic::Ordering::Relaxed);
        // Legacy/unused Phase 1 path (no caller in this repo): control shard.
        self.publish_with_retry(0, channel, payload).await
    }

    /// Publish an inbound envelope to a specific logic node (Phase 3).
    /// Returns the number of subscribers that received it. Sharded by
    /// `connection_id`: the transport tier does not know a connection's room
    /// code for an in-flight move, but it always knows the connection id, and
    /// a connection's messages always hash to the same shard (order-preserving).
    pub async fn publish_inbound_to(
        &self,
        envelope: &InboundEnvelope,
        logic_node_id: &str,
    ) -> redis::RedisResult<i64> {
        let shard = self.shard_for(&envelope.connection_id);
        let channel = channels::inbound_to_logic(logic_node_id);
        let payload = serde_json::to_string(envelope).map_err(serialize_error)?;
        INBOUND_PUBLISHES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        INBOUND_BYTES.fetch_add(payload.len() as u64, std::sync::atomic::Ordering::Relaxed);
        self.publish_with_retry(shard, channel, payload).await
    }

    /// Publish an outbound envelope to a specific transport node on `redis_shard`
    /// (chosen by the caller, which already knows the room/connection key).
    /// Returns the number of subscriber connections that received it (0 when no
    /// node is listening on that channel), which lets the caller detect a stale
    /// route (design D4 "Owning node resolution").
    pub async fn publish_outbound(
        &self,
        envelope: &OutboundEnvelope,
        redis_shard: usize,
    ) -> redis::RedisResult<i64> {
        let channel = channels::outbound_to_transport(&envelope.node_id);
        let payload = serde_json::to_string(envelope).map_err(serialize_error)?;
        OUTBOUND_PUBLISHES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        OUTBOUND_BYTES.fetch_add(payload.len() as u64, std::sync::atomic::Ordering::Relaxed);
        self.publish_with_retry(redis_shard, channel, payload).await
    }

    /// Publish a batch of outbound envelopes to a specific transport node on
    /// `redis_shard`. Returns the number of subscriber connections that
    /// received it.
    pub async fn publish_outbound_batch(
        &self,
        batch: &OutboundBatch,
        redis_shard: usize,
    ) -> redis::RedisResult<i64> {
        let channel = channels::outbound_to_transport(&batch.node_id);
        let payload = serde_json::to_string(batch).map_err(serialize_error)?;
        OUTBOUND_BATCHES_PUBLISHED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        OUTBOUND_BYTES.fetch_add(payload.len() as u64, std::sync::atomic::Ordering::Relaxed);
        self.publish_with_retry(redis_shard, channel, payload).await
    }

    /// Publish several already-serialized outbound payloads to one transport
    /// node's channel in a single pipelined round trip, in order. Returns one
    /// subscriber count per payload, in order, so the caller can run the
    /// stale-route retry per payload (design D2/D4). A transient connection
    /// failure is retried like `publish_outbound`; the cached connection is
    /// dropped so the retry reconnects. This removes the per-message round trip
    /// that dominates the shard's publish time.
    pub async fn publish_outbound_pipelined(
        &self,
        node_id: &str,
        redis_shard: usize,
        payloads: &[String],
    ) -> redis::RedisResult<Vec<i64>> {
        if payloads.is_empty() {
            return Ok(Vec::new());
        }
        let channel = channels::outbound_to_transport(node_id);
        OUTBOUND_PUBLISHES.fetch_add(payloads.len() as u64, std::sync::atomic::Ordering::Relaxed);
        let total_bytes: u64 = payloads.iter().map(|p| p.len() as u64).sum();
        OUTBOUND_BYTES.fetch_add(total_bytes, std::sync::atomic::Ordering::Relaxed);

        let mut last_err: Option<redis::RedisError> = None;
        for attempt in 0..PUBLISH_ATTEMPTS {
            let mut conn = match self.publish_connection(redis_shard).await {
                Ok(conn) => conn,
                Err(err) => {
                    last_err = Some(err);
                    if attempt + 1 < PUBLISH_ATTEMPTS {
                        tokio::time::sleep(PUBLISH_RETRY_DELAY).await;
                    }
                    continue;
                }
            };
            let mut pipe = redis::pipe();
            for payload in payloads {
                pipe.cmd("PUBLISH").arg(&channel).arg(payload);
            }
            match pipe.query_async::<Vec<i64>>(&mut conn).await {
                Ok(counts) => return Ok(counts),
                Err(err) => {
                    // Drop the cached connection so the next attempt reconnects.
                    *self.shards[redis_shard].publish_conn.write().unwrap() = None;
                    last_err = Some(err);
                }
            }
            if attempt + 1 < PUBLISH_ATTEMPTS {
                tokio::time::sleep(PUBLISH_RETRY_DELAY).await;
            }
        }
        Err(last_err.expect("at least one publish attempt runs"))
    }

    /// Subscribe to this node's outbound channel on `shard`. Uses a dedicated
    /// connection: a multiplexed/manager connection cannot enter subscriber mode.
    #[allow(deprecated)]
    pub async fn subscribe_outbound(&self, shard: usize) -> redis::RedisResult<redis::aio::PubSub> {
        let conn = self.shards[shard].client.get_async_connection().await?;
        let mut pubsub = conn.into_pubsub();
        pubsub
            .subscribe(channels::outbound_to_transport(&self.node_id))
            .await?;
        Ok(pubsub)
    }

    /// Subscribe to the Phase 1 inbound channel (transport -> logic).
    /// Legacy/unused path (no caller in this repo): control shard.
    #[allow(deprecated)]
    pub async fn subscribe_inbound(&self) -> redis::RedisResult<redis::aio::PubSub> {
        let conn = self.shards[0].client.get_async_connection().await?;
        let mut pubsub = conn.into_pubsub();
        pubsub
            .subscribe(channels::inbound_to_logic(DEFAULT_LOGIC_NODE))
            .await?;
        Ok(pubsub)
    }

    /// Subscribe to this logic node's own inbound channel on `shard` (Phase 3).
    /// Channel format: `ptc.ws.in.{logic_node_id}`.
    #[allow(deprecated)]
    pub async fn subscribe_inbound_self(&self, shard: usize) -> redis::RedisResult<redis::aio::PubSub> {
        let conn = self.shards[shard].client.get_async_connection().await?;
        let mut pubsub = conn.into_pubsub();
        pubsub
            .subscribe(channels::inbound_to_logic(&self.node_id))
            .await?;
        Ok(pubsub)
    }

    /// Subscribe to room broadcasts (Phase 2). Legacy/unused path: control shard.
    #[allow(deprecated)]
    pub async fn subscribe_room(&self, room_code: &str) -> redis::RedisResult<redis::aio::PubSub> {
        let conn = self.shards[0].client.get_async_connection().await?;
        let mut pubsub = conn.into_pubsub();
        pubsub.subscribe(channels::room(room_code)).await?;
        Ok(pubsub)
    }

    /// Subscribe to site-wide broadcasts (Phase 2). Legacy/unused path: control shard.
    #[allow(deprecated)]
    pub async fn subscribe_site(&self) -> redis::RedisResult<redis::aio::PubSub> {
        let conn = self.shards[0].client.get_async_connection().await?;
        let mut pubsub = conn.into_pubsub();
        pubsub.subscribe(channels::SITE).await?;
        Ok(pubsub)
    }

    /// Register a connection in the registry (Phase 2).
    pub async fn register_connection(
        &self,
        connection_id: &str,
        session_info: Option<&str>,
    ) -> redis::RedisResult<()> {
        let mut conn = self.conn();
        registry::register(&mut conn, connection_id, &self.node_id, session_info).await
    }

    /// Lookup the node owning a connection (Phase 2).
    pub async fn lookup_connection_node(
        &self,
        connection_id: &str,
    ) -> redis::RedisResult<Option<String>> {
        let mut conn = self.conn();
        registry::lookup_node(&mut conn, connection_id).await
    }

    /// Unregister a connection (Phase 2).
    pub async fn unregister_connection(&self, connection_id: &str) -> redis::RedisResult<()> {
        let mut conn = self.conn();
        registry::unregister(&mut conn, connection_id).await
    }

    /// Claim room ownership (Phase 3). Retries a bounded number of times
    /// (same shape as `publish_with_retry`: `PUBLISH_ATTEMPTS`/
    /// `PUBLISH_RETRY_DELAY`) on a control-shard error — QA finding: a
    /// burst of concurrent `CreateRoom`s across several logic nodes can
    /// make a single claim exceed `COMMAND_RESPONSE_TIMEOUT` from queueing
    /// alone, not a real Redis outage; left unretried, the ownership key
    /// silently never gets written and the second player's `JoinRoom`
    /// later misroutes.
    ///
    /// The retry has a subtlety a plain loop would get wrong: `SET NX` can
    /// succeed on the server before the client's timeout fires locally, so
    /// a retry can see the key already set — by this same claim, under
    /// this same node id. That must count as success, not "someone else
    /// already owns it": after exhausting retries on errors, check who the
    /// owner actually is before reporting failure.
    pub async fn claim_room(&self, room_code: &str) -> redis::RedisResult<bool> {
        let mut last_err = None;
        for attempt in 0..PUBLISH_ATTEMPTS {
            let mut conn = self.conn();
            match ownership::claim(&mut conn, room_code, &self.node_id).await {
                Ok(claimed) => return Ok(claimed),
                Err(e) => {
                    last_err = Some(e);
                    if attempt + 1 < PUBLISH_ATTEMPTS {
                        tokio::time::sleep(PUBLISH_RETRY_DELAY).await;
                    }
                }
            }
        }
        let mut conn = self.conn();
        match ownership::get_owner(&mut conn, room_code).await {
            Ok(Some(owner)) if owner == self.node_id => Ok(true),
            _ => Err(last_err.expect("at least one claim attempt ran")),
        }
    }

    /// Release room ownership (Phase 3).
    pub async fn release_room(&self, room_code: &str) -> redis::RedisResult<bool> {
        let mut conn = self.conn();
        ownership::release(&mut conn, room_code, &self.node_id).await
    }

    /// Get room owner (Phase 3). Retries a bounded number of times on a
    /// control-shard error (same QA finding as `claim_room` above) — a
    /// plain read is idempotent, so a retry here has no ambiguity to
    /// resolve.
    pub async fn get_room_owner(&self, room_code: &str) -> redis::RedisResult<Option<String>> {
        let mut last_err = None;
        for attempt in 0..PUBLISH_ATTEMPTS {
            let mut conn = self.conn();
            match ownership::get_owner(&mut conn, room_code).await {
                Ok(owner) => return Ok(owner),
                Err(e) => {
                    last_err = Some(e);
                    if attempt + 1 < PUBLISH_ATTEMPTS {
                        tokio::time::sleep(PUBLISH_RETRY_DELAY).await;
                    }
                }
            }
        }
        Err(last_err.expect("at least one get_owner attempt ran"))
    }

    /// Renew room ownership lease (Phase 3).
    pub async fn renew_room_lease(&self, room_code: &str) -> redis::RedisResult<bool> {
        let mut conn = self.conn();
        ownership::renew(&mut conn, room_code, &self.node_id).await
    }

    /// Record the transport node serving a room's first seat (colocate-room-hot-path, task 2.1).
    pub async fn set_room_transport(&self, room_code: &str, transport_node_id: &str) -> redis::RedisResult<()> {
        let mut conn = self.conn();
        room_transport::set(&mut conn, room_code, transport_node_id).await
    }

    /// Read the transport node recorded for a room, if any (colocate-room-hot-path, task 2.1).
    pub async fn get_room_transport(&self, room_code: &str) -> redis::RedisResult<Option<String>> {
        let mut conn = self.conn();
        room_transport::get(&mut conn, room_code).await
    }

    /// Renew multiple room ownership leases in a single pipeline (Phase 3, task 5.1).
    /// Returns a vec of success (true = renewed, false = key lost/owned by other)
    /// in the same order as the input rooms.
    pub async fn renew_room_leases_pipelined(&self, rooms: &[(String, String)]) -> redis::RedisResult<Vec<bool>> {
        let mut conn = self.conn();
        ownership::renew_pipelined(&mut conn, rooms).await
    }

    /// Register logic node membership (Phase 3).
    pub async fn register_logic_node(&self, metadata: Option<&str>) -> redis::RedisResult<()> {
        let mut conn = self.conn();
        membership::register(&mut conn, &self.node_id, metadata).await
    }

    /// All registered logic node ids (Phase 3). Routing probes each node's
    /// subscriber count, so a stale id is skipped rather than trusted.
    pub async fn logic_nodes(&self) -> redis::RedisResult<Vec<String>> {
        let mut conn = self.conn();
        membership::all_nodes(&mut conn).await
    }

    /// Remove this logic node's membership key on a clean shutdown (Phase 3).
    pub async fn unregister_logic_node(&self) -> redis::RedisResult<()> {
        let mut conn = self.conn();
        let key = format!("ptc.logic_node.{}", self.node_id);
        let _: i64 = conn.del(key).await?;
        Ok(())
    }

    /// Whether a logic node's membership key is present (Phase 3).
    pub async fn logic_node_alive(&self, node_id: &str) -> redis::RedisResult<bool> {
        let mut conn = self.conn();
        membership::is_alive(&mut conn, node_id).await
    }

    /// Renew logic node lease (Phase 3).
    pub async fn renew_logic_node_lease(&self) -> redis::RedisResult<bool> {
        let mut conn = self.conn();
        membership::renew(&mut conn, &self.node_id).await
    }

    /// Mirror a freshly issued session to Redis so another logic node
    /// accepts the same token (Phase 3, task 4.5). The token is
    /// hashed before it is stored; the plaintext never leaves the
    /// issuing node.
    pub async fn mirror_session(
        &self,
        token: &str,
        account_id: &str,
        username: &str,
        display_name: &str,
        expires_at_ms: i64,
    ) -> redis::RedisResult<()> {
        let mut conn = self.conn();
        session::store(
            &mut conn,
            &hash_token(token),
            account_id,
            username,
            display_name,
            expires_at_ms,
        )
        .await
    }

    /// Look up a session mirrored by any logic node. Returns the
    /// account id, its username, its display name, and the expiry
    /// when the token is still usable.
    pub async fn lookup_mirrored_session(
        &self,
        token: &str,
    ) -> redis::RedisResult<Option<(String, String, String, i64)>> {
        let mut conn = self.conn();
        session::load(&mut conn, &hash_token(token)).await
    }

    /// Remove a mirrored session (logout), revoking the token on
    /// every logic node at once.
    pub async fn revoke_mirrored_session(
        &self,
        token: &str,
    ) -> redis::RedisResult<()> {
        let mut conn = self.conn();
        session::remove(&mut conn, &hash_token(token)).await
    }

    /// Register transport node liveness lease (design D11).
    pub async fn register_node_lease(&self) -> redis::RedisResult<()> {
        let mut conn = self.conn();
        liveness::register(&mut conn, &self.node_id).await
    }

    /// Renew transport node liveness lease (design D11). Returns whether the
    /// lease was still present; `false` means it lapsed while this node ran.
    pub async fn renew_node_lease(&self) -> redis::RedisResult<bool> {
        let mut conn = self.conn();
        liveness::renew(&mut conn, &self.node_id).await
    }

    /// Check if a transport node's lease is alive (design D11).
    pub async fn node_lease_alive(&self, node_id: &str) -> redis::RedisResult<bool> {
        let mut conn = self.conn();
        liveness::is_alive(&mut conn, node_id).await
    }
}

/// The room actor's cross-node rating port, backed by the `rating` module
/// above. A mirror/lookup failure is surfaced as a `RecorderError` (a plain
/// string), same contract as `GameRecorder`/`TournamentRecorder`.
impl RatingMirror for Broker {
    fn mirror<'a>(
        &'a self,
        device_id: &'a str,
        state: RatingState,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), RecorderError>> + Send + 'a>>
    {
        Box::pin(async move {
            let mut conn = self.conn();
            rating::store(&mut conn, device_id, state, now_ms())
                .await
                .map_err(|e| e.to_string())
        })
    }

    fn lookup<'a>(
        &'a self,
        device_id: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Option<RatingState>, RecorderError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let mut conn = self.conn();
            rating::load(&mut conn, device_id).await.map_err(|e| e.to_string())
        })
    }
}

fn serialize_error(_: serde_json::Error) -> redis::RedisError {
    redis::RedisError::from((redis::ErrorKind::TypeError, "serialize envelope"))
}

/// Generate a unique connection ID.
pub fn generate_connection_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Generate a unique node ID.
pub fn generate_node_id(role: &str) -> String {
    format!("{}-{}", role, uuid::Uuid::new_v4().to_string()[..8].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interface::protocol::VERSION;

    #[test]
    fn envelope_serialization_roundtrip() {
        let inbound = InboundEnvelope {
            v: ENVELOPE_VERSION,
            connection_id: "conn-123".to_string(),
            node_id: "transport-abc".to_string(),
            payload: InboundPayload::Client(ClientMessage::CreateRoom {
                v: VERSION,
                player_id: "player-1".to_string(),
                time_control: Some("15+10".to_string()),
                token: None,
            }),
        };
        let json = serde_json::to_string(&inbound).unwrap();
        let decoded: InboundEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.v, ENVELOPE_VERSION);
        assert_eq!(decoded.connection_id, "conn-123");
        assert_eq!(decoded.node_id, "transport-abc");
        assert!(matches!(
            decoded.payload,
            InboundPayload::Client(ClientMessage::CreateRoom { .. })
        ));

        // A disconnect control event round-trips too.
        let disconnect = InboundEnvelope {
            v: ENVELOPE_VERSION,
            connection_id: "conn-123".to_string(),
            node_id: "transport-abc".to_string(),
            payload: InboundPayload::Disconnect,
        };
        let decoded: InboundEnvelope =
            serde_json::from_str(&serde_json::to_string(&disconnect).unwrap()).unwrap();
        assert!(matches!(decoded.payload, InboundPayload::Disconnect));

        // Create a dummy State for RoomReady
        let dummy_state = crate::interface::protocol::State {
            board_fen: "x".into(),
            move_list: vec![],
            side_to_move: "w".into(),
            status: crate::interface::protocol::Status::Playing,
            your_color: crate::interface::protocol::Color::White,
            white_rating: 1500.0,
            black_rating: 1500.0,
            opponent_online: true,
            time_control: "15+10".into(),
            white_time_ms: 60_000,
            black_time_ms: 60_000,
        };

        let outbound = OutboundEnvelope {
            v: ENVELOPE_VERSION,
            node_id: "transport-abc".to_string(),
            connection_id: "conn-123".to_string(),
            payload: ServerMessage::RoomReady {
                v: VERSION,
                room_code: "ABC123".to_string(),
                your_color: crate::interface::protocol::Color::White,
                state: dummy_state,
            },
        };
        let json = serde_json::to_string(&outbound).unwrap();
        let decoded: OutboundEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.v, ENVELOPE_VERSION);
        assert_eq!(decoded.node_id, "transport-abc");
        assert_eq!(decoded.connection_id, "conn-123");
    }

    #[test]
    fn channel_naming() {
        // The Phase 1 inbound channel is the well-known logic target, not a
        // per-instance name, so the transport and the logic tier always meet.
        assert_eq!(
            channels::inbound_to_logic(DEFAULT_LOGIC_NODE),
            "ptc.ws.in.logic"
        );
        assert_eq!(channels::inbound_to_logic("logic-1"), "ptc.ws.in.logic-1");
        assert_eq!(channels::outbound_to_transport("transport-1"), "ptc.ws.out.transport-1");
        assert_eq!(channels::room("ABC123"), "ptc.room.ABC123");
        assert_eq!(channels::SITE, "ptc.site");
        assert_eq!(channels::node_heartbeat("node-1"), "ptc.node.node-1");
    }

    /// Task 2.1: Verify a batch decodes to the same per-connection messages.
    #[test]
    fn batch_dispatches_to_each_connection() {
        let dummy_state = crate::interface::protocol::State {
            board_fen: "x".into(),
            move_list: vec![],
            side_to_move: "w".into(),
            status: crate::interface::protocol::Status::Playing,
            your_color: crate::interface::protocol::Color::White,
            white_rating: 1500.0,
            black_rating: 1500.0,
            opponent_online: true,
            time_control: "15+10".into(),
            white_time_ms: 60_000,
            black_time_ms: 60_000,
        };

        let batch = OutboundBatch {
            v: ENVELOPE_VERSION,
            node_id: "transport-abc".to_string(),
            items: vec![
                OutboundEnvelope {
                    v: ENVELOPE_VERSION,
                    node_id: "transport-abc".to_string(),
                    connection_id: "conn-1".to_string(),
                    payload: ServerMessage::RoomReady {
                        v: VERSION,
                        room_code: "ROOM1".to_string(),
                        your_color: crate::interface::protocol::Color::White,
                        state: dummy_state.clone(),
                    },
                },
                OutboundEnvelope {
                    v: ENVELOPE_VERSION,
                    node_id: "transport-abc".to_string(),
                    connection_id: "conn-2".to_string(),
                    payload: ServerMessage::RoomReady {
                        v: VERSION,
                        room_code: "ROOM1".to_string(),
                        your_color: crate::interface::protocol::Color::Black,
                        state: dummy_state.clone(),
                    },
                },
            ],
        };

        let json = serde_json::to_string(&batch).unwrap();
        let wire: OutboundWire = serde_json::from_str(&json).unwrap();
        assert!(matches!(wire, OutboundWire::Batch(_)));

        let decoded_batch = match wire {
            OutboundWire::Batch(b) => b,
            _ => panic!("expected batch"),
        };
        assert_eq!(decoded_batch.node_id, "transport-abc");
        assert_eq!(decoded_batch.items.len(), 2);
        assert_eq!(decoded_batch.items[0].connection_id, "conn-1");
        assert_eq!(decoded_batch.items[1].connection_id, "conn-2");
    }

    /// Task 2.2: Verify single-item envelope behaves exactly as v1 (wire compatibility).
    #[test]
    fn single_item_envelope_unchanged() {
        let dummy_state = crate::interface::protocol::State {
            board_fen: "x".into(),
            move_list: vec![],
            side_to_move: "w".into(),
            status: crate::interface::protocol::Status::Playing,
            your_color: crate::interface::protocol::Color::White,
            white_rating: 1500.0,
            black_rating: 1500.0,
            opponent_online: true,
            time_control: "15+10".into(),
            white_time_ms: 60_000,
            black_time_ms: 60_000,
        };

        let envelope = OutboundEnvelope {
            v: ENVELOPE_VERSION,
            node_id: "transport-abc".to_string(),
            connection_id: "conn-1".to_string(),
            payload: ServerMessage::RoomReady {
                v: VERSION,
                room_code: "ROOM1".to_string(),
                your_color: crate::interface::protocol::Color::White,
                state: dummy_state,
            },
        };

        // Serialize as OutboundWire::Envelope (single item)
        let wire = OutboundWire::Envelope(envelope.clone());
        let json = serde_json::to_string(&wire).unwrap();

        // The JSON should match the bare OutboundEnvelope format (untagged)
        let decoded_envelope: OutboundEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded_envelope.v, ENVELOPE_VERSION);
        assert_eq!(decoded_envelope.node_id, "transport-abc");
        assert_eq!(decoded_envelope.connection_id, "conn-1");

        // Also verify we can decode a v1-style bare envelope (no "Envelope" wrapper)
        let v1_json = serde_json::to_string(&envelope).unwrap();
        let decoded_v1: OutboundWire = serde_json::from_str(&v1_json).unwrap();
        assert!(matches!(decoded_v1, OutboundWire::Envelope(_)));
    }

    /// Task 4.1: Verify inbound frame is forwarded without re-encoding.
    /// The logic tier receives the original frame text and decodes once.
    #[test]
    fn inbound_frame_decoded_once() {
        // Client frame uses "type" field with snake_case (e.g., "move", "create_room")
        let frame = r#"{"v":1,"type":"move","uci":"e2e4"}"#;
        let envelope = InboundEnvelope {
            v: ENVELOPE_VERSION,
            connection_id: "conn-1".to_string(),
            node_id: "transport-abc".to_string(),
            payload: InboundPayload::ClientFrame { data: frame.to_string() },
        };

        let json = serde_json::to_string(&envelope).unwrap();
        let decoded: InboundEnvelope = serde_json::from_str(&json).unwrap();

        match decoded.payload {
            InboundPayload::ClientFrame { data } => {
                assert_eq!(data, frame);
                // Verify it's valid JSON that can be decoded by logic
                let msg = crate::interface::protocol::decode_incoming(&data);
                assert!(msg.is_ok(), "decode_incoming failed: {:?}", msg);
                assert!(matches!(msg.unwrap(), crate::interface::protocol::ClientMessage::Move { uci, .. } if uci == "e2e4"));
            }
            _ => panic!("expected ClientFrame"),
        }
    }

    /// Legacy v1 envelope should still decode (rollout compat).
    /// v1 format: InboundPayload was externally tagged with rename_all="snake_case",
    /// so Client(ClientMessage) -> {"client": {"type":"create_room",...}}.
    /// v2 format uses untagged enum; must accept both.
    #[test]
    fn legacy_v1_inbound_payload_decodes() {
        // v1 wire format (externally tagged InboundPayload)
        let v1_json = r#"{"v":1,"connection_id":"conn-1","node_id":"transport-abc","payload":{"client":{"type":"create_room","v":1,"player_id":"p1","time_control":"15+10","token":null}}}"#;
        let decoded: InboundEnvelope = serde_json::from_str(v1_json).unwrap();
        assert_eq!(decoded.v, 1); // v1 envelope version
        assert!(matches!(
            decoded.payload,
            InboundPayload::LegacyClient {
                client: crate::interface::protocol::ClientMessage::CreateRoom { .. }
            }
        ));
    }

    /// The v1 disconnect form (bare string "disconnect") still decodes.
    #[test]
    fn legacy_v1_disconnect_decodes() {
        let v1_json =
            r#"{"v":1,"connection_id":"conn-1","node_id":"transport-abc","payload":"disconnect"}"#;
        let decoded: InboundEnvelope = serde_json::from_str(v1_json).unwrap();
        assert!(matches!(decoded.payload, InboundPayload::LegacyDisconnect(_)));

        // The v2 form (null unit variant) also decodes.
        let v2 = InboundEnvelope {
            v: ENVELOPE_VERSION,
            connection_id: "conn-1".to_string(),
            node_id: "transport-abc".to_string(),
            payload: InboundPayload::Disconnect,
        };
        let decoded: InboundEnvelope =
            serde_json::from_str(&serde_json::to_string(&v2).unwrap()).unwrap();
        assert!(matches!(decoded.payload, InboundPayload::Disconnect));
    }
}
