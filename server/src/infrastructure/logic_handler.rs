//! Logic tier handler: subscribes to inbound envelopes from the Redis fabric,
//! dispatches them into the existing `App`/room actors, and publishes outbound
//! envelopes back to the owning transport connection (design D5/D6).
//!
//! Inbound envelopes are processed **in series** in one task, so a connection's
//! messages keep their order and the logic tier does not need a sequencer
//! (design D4 "Ordering").

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
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

/// Exponential backoff, capped, from a 1-based failure count.
fn reconnect_backoff(failures: u32) -> Duration {
    (RECONNECT_MIN * (1u32 << failures.min(4))).min(RECONNECT_MAX)
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

/// Shared state for the logic tier.
pub struct LogicState {
    app: Arc<App>,
    broker: Arc<Broker>,
    node_id: String,
    /// Maps connection_id -> LogicConnection.
    connections: AsyncMutex<HashMap<String, LogicConnection>>,
}

impl LogicState {
    pub fn new(app: Arc<App>, broker: Arc<Broker>, node_id: String) -> Self {
        Self {
            app,
            broker,
            node_id,
            connections: AsyncMutex::new(HashMap::new()),
        }
    }

    /// Start the logic tier: register the node lease, subscribe to inbound,
    /// then consume envelopes in series. The subscription is re-established when
    /// the broker drops; if it stays gone past a bounded number of attempts the
    /// tier exits non-zero so a supervisor notices (spec "Broker loss is surfaced").
    ///
    /// If `ready_tx` is provided, a signal is sent when the logic node has
    /// subscribed to its inbound channel and is ready to receive messages.
    /// This is useful for test harnesses that need to wait for the node to be ready.
    pub async fn run(self: Arc<Self>, ready_tx: Option<oneshot::Sender<()>>) -> Result<(), Box<dyn std::error::Error>> {
        info!(node_id = %self.node_id, "logic tier starting");

        // Fail fast at startup: no broker, no service.
        self.broker.register_logic_node(None).await?;

        // Subscribe to our inbound channel immediately so we're ready to receive
        // messages before spawning background tasks (helps with test harness timing).
        let mut pubsub = self.broker.subscribe_inbound_self().await?;

        // Signal that we're ready to receive messages.
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

                // Collect distinct transport node ids currently holding connections.
                let nodes: Vec<String> = {
                    let conns = watch_state.connections.lock().await;
                    let mut seen = Vec::new();
                    for conn in conns.values() {
                        if !conn.transport_node_id.is_empty() && !seen.contains(&conn.transport_node_id) {
                            seen.push(conn.transport_node_id.clone());
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
                                // Collect connection ids on this node.
                                let ids: Vec<String> = {
                                    let conns = watch_state.connections.lock().await;
                                    conns.iter()
                                        .filter(|(_, c)| c.transport_node_id == node)
                                        .map(|(id, _)| id.clone())
                                        .collect()
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

        // One task processes every envelope, in arrival order, reconnecting
        // when the broker restarts (design D4/D10).
        let mut failures: u32 = 0;
        loop {
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
                    self.handle_inbound(envelope).await;
                }
            }

            error!(node_id = %self.node_id, "inbound subscription ended (broker lost); reconnecting");
            // Re-subscribe WITHIN the failure budget. The subscribe itself must
            // be inside the budget: a `?` here would end the process on the first
            // failed attempt, making the budget dead code and killing the whole
            // logic tier on any broker outage longer than one retry (BUG-P3-4).
            pubsub = loop {
                failures += 1;
                if failures >= MAX_SUBSCRIBE_FAILURES {
                    // Give up: drop the membership key so this node is not
                    // mistaken for a live logic node, then exit non-zero.
                    let _ = self.broker.unregister_logic_node().await;
                    return Err(format!("broker unavailable after {failures} attempts").into());
                }
                tokio::time::sleep(reconnect_backoff(failures)).await;
                match self.broker.subscribe_inbound_self().await {
                    Ok(pubsub) => {
                        info!(node_id = %self.node_id, "broker reconnected: inbound subscription restored");
                        failures = 0;
                        break pubsub;
                    }
                    Err(e) => {
                        warn!(error = %e, failures, "failed to re-subscribe to the inbound channel; retrying");
                    }
                }
            };
        }
    }

    /// Refresh this node's ownership of every room it still holds, re-claiming a
    /// room whose ownership key was lost (BUG-P3-1). The room list comes from the
    /// authoritative `App`, not from scanning Redis keys (a lost key would make a
    /// scan-based sweep miss exactly the room that needs re-claiming).
    async fn keep_room_ownership(&self) {
        for code in self.app.room_codes() {
            match self.broker.renew_room_lease(&code).await {
                Ok(true) => {}
                Ok(false) => match self.broker.claim_room(&code).await {
                    Ok(true) => {
                        warn!(room_code = %code, "re-claimed a room whose ownership lease was lost");
                    }
                    Ok(false) => {
                        let owner = self.broker.get_room_owner(&code).await.ok().flatten();
                        if owner.as_deref() != Some(self.node_id.as_str()) {
                            warn!(room_code = %code, owner = ?owner, "another node owns a room this node holds");
                        }
                    }
                    Err(e) => {
                        warn!(room_code = %code, error = %e, "failed to re-claim room ownership");
                    }
                },
                Err(e) => {
                    warn!(room_code = %code, error = %e, "failed to renew room ownership");
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
        let mut connections = self.connections.lock().await;
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
        let mut connections = self.connections.lock().await;
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
