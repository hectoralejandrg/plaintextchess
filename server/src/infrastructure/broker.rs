//! Redis event fabric for the split realtime architecture (design D2-D4).
//!
//! Provides versioned envelopes, channel naming, and connection addressing
//! for communication between the transport tier and the logic tier.

use redis::{
    aio::ConnectionManager, AsyncCommands, Client as RedisClient, ExistenceCheck, SetExpiry,
    SetOptions,
};
use serde::{Deserialize, Serialize};

use crate::infrastructure::password::{hash_token, now_ms};
use crate::interface::protocol::{ClientMessage, ServerMessage};

/// Version of the cross-tier envelope format (design D3). Distinct from the
/// client wire `VERSION`: an envelope may evolve independently of the client
/// protocol.
pub const ENVELOPE_VERSION: u16 = 1;

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

/// What a transport node forwards to the logic tier (design D3/D5).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboundPayload {
    /// A client frame, already validated by the transport with
    /// `decode_incoming` so a malformed frame still closes the socket with the
    /// protocol close code.
    Client(ClientMessage),
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

/// Logic node membership: each logic node registers with a lease.
pub mod membership {
    use super::*;
    use redis::AsyncCommands;

    /// Prefix for logic node membership keys.
    const PREFIX: &str = "ptc.logic_node.";

    /// TTL for logic node lease.
    pub const TTL_SECS: u64 = 30;

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
    /// (BUG-R1-F2: `EXPIRE` on a missing key is a silent no-op).
    pub async fn renew(conn: &mut ConnectionManager, node_id: &str) -> redis::RedisResult<bool> {
        let key = format!("{}{}", PREFIX, node_id);
        let existed: bool = conn.exists(&key).await?;
        let value = serde_json::json!({
            "renewed_at": now_ms(),
        });
        let _: () = conn
            .set_ex(key, serde_json::to_string(&value).unwrap(), TTL_SECS)
            .await?;
        Ok(existed)
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

    /// Prefix for transport node liveness keys. Matches `channels::node_heartbeat`.
    const PREFIX: &str = "ptc.node.";

    /// TTL for transport node lease (seconds).
    pub const TTL_SECS: u64 = 30;

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
    /// connections as void (design D11). Uses SET EX, which self-heals the key.
    pub async fn renew(conn: &mut ConnectionManager, node_id: &str) -> redis::RedisResult<bool> {
        let key = format!("{}{}", PREFIX, node_id);
        let existed: bool = conn.exists(&key).await?;
        let value = serde_json::json!({
            "renewed_at": now_ms(),
        });
        let _: () = conn
            .set_ex(key, serde_json::to_string(&value).unwrap(), TTL_SECS)
            .await?;
        Ok(existed)
    }

    /// Check if a transport node's lease is alive.
    pub async fn is_alive(conn: &mut ConnectionManager, node_id: &str) -> redis::RedisResult<bool> {
        let key = format!("{}{}", PREFIX, node_id);
        conn.exists(key).await
    }
}

/// Broker client for publishing/subscribing to the Redis fabric.
pub struct Broker {
    client: RedisClient,
    /// Shared multiplexed connection for commands (publish, registry, ...).
    manager: ConnectionManager,
    node_id: String,
}

impl Broker {
    /// Connect to Redis and build the shared command connection. Fails if the
    /// broker is unreachable, which is how "refuse to start without Redis"
    /// (spec `realtime-transport`) is enforced at the call site.
    pub async fn connect(redis_url: &str, node_id: String) -> redis::RedisResult<Self> {
        let client = RedisClient::open(redis_url)?;
        let manager = client.get_connection_manager().await?;
        Ok(Self {
            client,
            manager,
            node_id,
        })
    }

    /// Get the node identifier.
    pub fn node_id(&self) -> &str {
        &self.node_id
    }

    fn conn(&self) -> ConnectionManager {
        self.manager.clone()
    }

    /// Publish `payload` on `channel`, retrying a transient connection failure.
    /// Returns the subscriber count. A broker restart can leave the manager's
    /// connection dead, so the first command may fail before it reconnects
    /// (design D10 / BUG-R1-F1).
    async fn publish_with_retry(
        &self,
        channel: String,
        payload: String,
    ) -> redis::RedisResult<i64> {
        let mut last_err: Option<redis::RedisError> = None;
        for attempt in 0..PUBLISH_ATTEMPTS {
            let mut conn = self.conn();
            match conn
                .publish::<_, _, i64>(channel.clone(), payload.clone())
                .await
            {
                Ok(subscribers) => return Ok(subscribers),
                Err(err) => last_err = Some(err),
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
        self.publish_with_retry(channel, payload).await
    }

    /// Publish an inbound envelope to a specific logic node (Phase 3).
    /// Returns the number of subscribers that received it.
    pub async fn publish_inbound_to(
        &self,
        envelope: &InboundEnvelope,
        logic_node_id: &str,
    ) -> redis::RedisResult<i64> {
        let channel = channels::inbound_to_logic(logic_node_id);
        let payload = serde_json::to_string(envelope).map_err(serialize_error)?;
        self.publish_with_retry(channel, payload).await
    }

    /// Publish an outbound envelope to a specific transport node. Returns the
    /// number of subscriber connections that received it (0 when no node is
    /// listening on that channel), which lets the caller detect a stale route
    /// (design D4 "Owning node resolution").
    pub async fn publish_outbound(
        &self,
        envelope: &OutboundEnvelope,
    ) -> redis::RedisResult<i64> {
        let channel = channels::outbound_to_transport(&envelope.node_id);
        let payload = serde_json::to_string(envelope).map_err(serialize_error)?;
        self.publish_with_retry(channel, payload).await
    }

    /// Subscribe to this node's outbound channel. Uses a dedicated connection:
    /// a multiplexed/manager connection cannot enter subscriber mode.
    #[allow(deprecated)]
    pub async fn subscribe_outbound(&self) -> redis::RedisResult<redis::aio::PubSub> {
        let conn = self.client.get_async_connection().await?;
        let mut pubsub = conn.into_pubsub();
        pubsub
            .subscribe(channels::outbound_to_transport(&self.node_id))
            .await?;
        Ok(pubsub)
    }

    /// Subscribe to the Phase 1 inbound channel (transport -> logic).
    #[allow(deprecated)]
    pub async fn subscribe_inbound(&self) -> redis::RedisResult<redis::aio::PubSub> {
        let conn = self.client.get_async_connection().await?;
        let mut pubsub = conn.into_pubsub();
        pubsub
            .subscribe(channels::inbound_to_logic(DEFAULT_LOGIC_NODE))
            .await?;
        Ok(pubsub)
    }

    /// Subscribe to this logic node's own inbound channel (Phase 3).
    /// Channel format: `ptc.ws.in.{logic_node_id}`.
    #[allow(deprecated)]
    pub async fn subscribe_inbound_self(&self) -> redis::RedisResult<redis::aio::PubSub> {
        let conn = self.client.get_async_connection().await?;
        let mut pubsub = conn.into_pubsub();
        pubsub
            .subscribe(channels::inbound_to_logic(&self.node_id))
            .await?;
        Ok(pubsub)
    }

    /// Subscribe to room broadcasts (Phase 2).
    #[allow(deprecated)]
    pub async fn subscribe_room(&self, room_code: &str) -> redis::RedisResult<redis::aio::PubSub> {
        let conn = self.client.get_async_connection().await?;
        let mut pubsub = conn.into_pubsub();
        pubsub.subscribe(channels::room(room_code)).await?;
        Ok(pubsub)
    }

    /// Subscribe to site-wide broadcasts (Phase 2).
    #[allow(deprecated)]
    pub async fn subscribe_site(&self) -> redis::RedisResult<redis::aio::PubSub> {
        let conn = self.client.get_async_connection().await?;
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

    /// Claim room ownership (Phase 3).
    pub async fn claim_room(&self, room_code: &str) -> redis::RedisResult<bool> {
        let mut conn = self.conn();
        ownership::claim(&mut conn, room_code, &self.node_id).await
    }

    /// Release room ownership (Phase 3).
    pub async fn release_room(&self, room_code: &str) -> redis::RedisResult<bool> {
        let mut conn = self.conn();
        ownership::release(&mut conn, room_code, &self.node_id).await
    }

    /// Get room owner (Phase 3).
    pub async fn get_room_owner(&self, room_code: &str) -> redis::RedisResult<Option<String>> {
        let mut conn = self.conn();
        ownership::get_owner(&mut conn, room_code).await
    }

    /// Renew room ownership lease (Phase 3).
    pub async fn renew_room_lease(&self, room_code: &str) -> redis::RedisResult<bool> {
        let mut conn = self.conn();
        ownership::renew(&mut conn, room_code, &self.node_id).await
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
}