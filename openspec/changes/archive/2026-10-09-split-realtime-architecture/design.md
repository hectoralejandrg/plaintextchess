# Design

## Context

See `proposal.md` - Why. The current server is one Rust process
(`server/`): `main.rs` binds an axum router with `/healthz` and `/ws`;
`infrastructure/ws.rs` runs `handle_socket`, which splits the socket into a
reader task and a writer task and owns per-connection outbound coalescing,
keepalive, protocol-close, and reconnect logic; `infrastructure/server.rs`
holds the app state (`rooms: HashMap<code, UnboundedSender<RoomMsg>>`,
`players`, `account_rooms`, ratings, auth) behind `std::sync::Mutex`, and
`create_room`/`join_room` spawn one `run_room` actor per room
(`application/room_actor.rs`) that sends `ServerMessage`s straight to each
seat's `out` sender. The wire protocol lives in `interface/protocol.rs`
(versioned JSON, `ServerMessage::{RoomReady, State, Update, Error, Session,
SessionOk, ProfileUpdated}`) and is mirrored by the iOS/Android clients, so it
must not change. Persistence is SQLite (`recorder.rs`, `auth_recorder.rs`).

The constraints that shape this design: keep the client protocol and endpoint
identical; keep SQLite; make the connection tier scalable by adding nodes; and
make the split opt-in so the current single-process mode remains available for
development and tests.

## Goals / Non-Goals

**Goals:**
- A standalone transport process that terminates WebSockets and holds no
  authoritative game state.
- A Redis event fabric between tiers, with per-connection addressing and
  per-room ordering.
- Unchanged client behavior (messages, errors, close codes, keepalive,
  reconnect grace).
- A path to N transport nodes (Phase 2) and M logic nodes with room ownership
  and failover (Phase 3).
- Keep SQLite for durable records.

**Non-Goals:**
- Changing the client wire protocol or either mobile app.
- Replacing SQLite with Postgres/Mongo in this change.
- Binary WebSocket frames, or a new transport (still JSON text frames).
- Analysis engine / Stockfish, tournaments, or spectating infrastructure.
- Guaranteeing exactly-once delivery across the fabric (reconciliation via
  snapshots is the fallback).

## Decisions

### D1 - Two Rust tiers in the same workspace, not separate repos
Add a `ws-server` binary that depends on shared crates: `interface/protocol`
(wire types), the connection-handling logic currently in
`infrastructure/ws.rs`, and a new broker module. The logic binary keeps
`domain`/`application`/`infrastructure`. Both binaries stay in this Cargo
workspace.
- **Why**: maximum code reuse (protocol types, domain rules, config), one CI,
  one version.
- **Alternatives**: separate repos/languages (Lichess uses Scala on both
  sides) - rejected as unnecessary overhead at our scale.

### D2 - Redis pub/sub as the event fabric
Use Redis pub/sub for tier messaging, plus Redis keys (with TTL) for the
Phase 2 connection registry and Phase 3 ownership map.
- **Why**: matches the architecture we are copying, low latency, one dependency
  that also serves as the shared store for routing.
- **Alternatives**: NATS (cleaner subject routing, but a second system to
  operate); direct TCP between tiers (reintroduces point-to-point coupling and
  breaks horizontal scaling).

### D3 - Broker envelope with connection identity
Every client connection is given a `connection_id` (UUID) and a `node_id`
(the transport instance) at accept time. Inbound envelopes are
`{ v, connection_id, node_id, payload }` where `payload` is an
`InboundPayload`: either `Client(ClientMessage)` - a frame the transport has
already validated - or `Disconnect`, the socket-closed control event that
preserves the monolith's `Detach`/grace behaviour. Outbound envelopes are
`{ v, node_id, connection_id, payload: ServerMessage }`.
- **Why**: the logic tier can address the exact socket without knowing the
  load-balancer topology; the transport stays stateless; and a disconnect is a
  first-class event rather than an inferred absence.
- **Alternatives**: address by room code only - rejected because not every
  message is room-scoped (auth, profile, presence). Forward the raw frame text -
  rejected because a disconnect is not a client frame and needs a typed
  envelope anyway.
- **Validation, once**: the transport validates every inbound frame with
  `interface::protocol::decode_incoming`, so a malformed or unknown frame still
  closes the socket with `PROTOCOL_CLOSE_CODE`; only then does it forward the
  parsed `ClientMessage`. Both tiers share the one protocol type, so the client
  wire format is untouched.
- **Envelope version is its own constant**: the envelope `v` is
  `broker::ENVELOPE_VERSION`, not the client `VERSION`, so the fabric can evolve
  independently of the client protocol.

### D4 - Channel layout
- Inbound (transport → logic): one channel per logic target, e.g.
  `ptc.ws.in.<logic_node>`, with the envelope naming the connection.
- Outbound (logic → transport): one channel per transport node, e.g.
  `ptc.ws.out.<node_id>`, filtered by `connection_id` at the node.
- Room-scoped events are delivered with the same per-connection addressing;
  a dedicated `ptc.room.<code>` channel is available for Phase 2 fan-out, while
  a site-wide broadcast channel (`ptc.site`) is out of scope for this change.
- **Why**: per-node outbound channels avoid a Redis pattern-subscribe per
  connection and keep routing cheap.
- **Alternatives**: one channel per connection (thousands of subscriptions) -
  rejected for overhead; a single global channel - rejected for fan-out cost.
- **Ordering**: per-room order needs no sequencer. A single client's frames
  arrive in order because one transport connection publishes them serially; a
  single logic instance consumes the single inbound channel
  `ptc.ws.in.<logic_node>` and processes it in series; and `run_room` already
  serializes that room's mutations through its mailbox. Ordering *between* the
  two players is resolved by turn rules (only the player to move can apply a
  transition). Multiple transport publishers therefore need no cross-publisher
  ordering guarantee from Redis beyond single-connection FIFO.
- **One outbound subscription per transport node**: the transport subscribes
  once to `ptc.ws.out.<node_id>` and fans each envelope to its socket through an
  in-memory `connection_id -> Outbox` map. A per-connection subscription is
  rejected: it would create one Redis subscriber per client.
- **Owning node resolution**: the logic addresses an outbound message to the
  node recorded for the connection - the `node_id` carried on the inbound
  envelope, falling back to the `ptc.conn.<id>` registry when it is not already
  known. The registry is a fallback and a source of stale-entry detection, not
  the primary routing path (design D11).

### D5 - Transport is protocol-aware, logic is authoritative
The transport tier keeps framing, protocol-close on malformed JSON/unknown
types, keepalive pings, per-connection outbound coalescing, and reconnect
bookkeeping. It does not apply game rules, seat players, authenticate, or hold
rooms. All business decisions stay in the logic tier.
- **Why**: these behaviors are per-socket and must live where the socket is;
  moving them keeps the split transparent to clients.
- **Alternatives**: a dumb byte proxy - rejected because coalescing/keepalive/
  close codes would be lost or would have to move to the logic tier, defeating
  isolation.

### D6 - Single logic instance first; routing and leases later
Phase 1–2 run one logic instance. Phase 3 introduces a `room:<code> -> node`
ownership map in Redis and logic-node membership via leases (a `node:<id>` key
with a TTL renewed by heartbeat). A transport node resolves a room's owner
before routing a room-scoped frame and re-resolves on a stale miss.
- **Why**: incremental delivery; the big capacity win (N transport nodes) does
  not require multiple logic nodes.
- **Alternatives**: sharded logic nodes from day one - rejected as premature.

### D7 - SQLite stays; single writer is enforced
Durable records keep using SQLite. In Phases 1–2 the single logic instance is
the writer. In Phase 3 either one designated writer instance performs all
record writes, or the database is partitioned per instance. Session/auth state
that must be shared across logic nodes moves to Redis, with SQLite remaining the
durable mirror.
- **Why**: honors the "Redis + SQLite" decision while avoiding concurrent
  SQLite writers.
- **Alternatives**: migrate to Postgres now - deferred to a later change.

### D8 - Config seam: `TIER=all|transport|logic`
`Config` gains a tier role and `REDIS_URL`. `TIER=all` keeps today's
single-process behavior (and is the default for tests/dev); `TIER=transport`
and `TIER=logic` run the split, and each refuses to start without Redis. This
mirrors the existing `Config.incremental_updates` seam (production on, tests
off) and makes the split independently testable.
- **Why**: zero-risk rollout, easy rollback, tests unaffected by default.
- **Alternatives**: always split - rejected because it makes local dev and the
  existing socket tests depend on Redis.

### D9 - Rollback by configuration
Because the split is opt-in, rolling back is flipping the deployment back to
`TIER=all` (one process, no Redis), not reverting code.

### D10 - Broker loss: reconnect or exit, and never a silent client
The fabric is not assumed to be always up. The transport's outbound
subscription is re-established with bounded backoff (100 ms → 2 s); while it is
down, live clients are closed with a distinct `1011` "broker unavailable" frame
(never the client protocol's `4000`), and new connections are refused the same
way. The logic tier re-establishes its inbound subscription with backoff and
exits non-zero if it cannot, so a supervisor notices. Each tier exposes
`GET /healthz`. Connection cleanup (publishing the disconnect and unregistering
the connection) is best-effort and time-bounded, so a dead broker cannot keep a
closing socket open.
- **Why**: spec `realtime-transport` requires a broker loss to be surfaced and
  the tiers to "reconnect or exit rather than silently misbehaving"; a
  subscription stream that simply ends must not turn a socket into a zombie.
- **Publish survives a reconnect**: the first command after a broker restart can
  fail on a dead connection (`broken pipe`); publishes are retried a bounded
  number of times and the logic node lease self-registers, so a client that
  arrives right after recovery is still served (BUG-R1-F1/F2).
- **A 0-subscriber publish is not delivery**: pub/sub does not persist, so while
  the logic tier is still resubscribing a publish reaches nobody. The transport
  waits briefly for a subscriber and otherwise closes the client with a visible
  signal, so an inbound frame is never silently dropped (BUG-F1-R1).
- **The reconnect budget covers the re-subscription (both tiers)**: the transport
  dispatcher and the logic tier both retry their subscription with backoff inside
  the failure budget; the subscribe call must not short-circuit it with `?`, or a
  broker outage longer than one retry would kill the process after a single
  attempt (BUG-P3-4).
- **Alternatives**: exit on first loss (rejected: a transient blip would drop
  every connection); rely on the broker/TCP to recover silently (rejected: a
  redis-rs `PubSub` stream ends on error and does not resubscribe itself).

### D11 - Transport node liveness and orphaned seats
A transport node holds a lease `ptc.node.<node_id>` (a key with a TTL, renewed on
an interval) while it is alive. The logic tier watches the leases of the
transport nodes that currently hold connections (the distinct `node_id`s on its
in-memory `LogicConnection`s). When a node's lease has been missing for a couple
of consecutive checks - i.e. it crashed without publishing a `disconnect` for its
sockets - the logic synthesizes the same grace/forfeit path as an inbound
`Disconnect` for each of that node's connections and drops their `ptc.conn.<id>`
registry entries, so an opponent observes the absence and a game never hangs on a
silently dead node.
- **Why**: a crashed node cannot publish `Disconnect` for its sockets, and Phase 1
  only covers graceful closes; without this a seat would stay "connected" forever
  and the opponent would never see it go offline (spec `transport-scale-out`,
  "Transport Node Failure Isolation").
- **Enumeration is from the authoritative side**: the logic already holds one
  `LogicConnection` per connection keyed by its owning node, so a dead node's
  connections are found in memory; the `ptc.conn.<id>` registry is used for
  stale-entry cleanup, not for the sweep. (A per-node Redis index would only be
  needed if that map stopped being authoritative - e.g. across multiple logic
  nodes in Phase 3.)
- **Lease TTL < reconnect grace (asserted at startup)**: the transport-node
  lease must expire before the reconnect grace window so a dead node's seats
  enter grace in time to be reclaimed. `lease TTL + detection window <
  reconnect grace` is checked when a split tier starts.
- **A lapsed lease is not proof of death**: a node paused longer than the TTL
  (GC/SIGSTOP) or whose keys were lost can lose its lease while its sockets are
  still alive. Such a node MUST treat the lapse as a fault: on its next renewal
  it detects the missing key and closes its connections with a `1012` "node
  lease lost; reconnect" frame, so clients reconnect and re-attach instead of a
  present player being silently orphaned.
- **Idempotent**: a synthesized disconnect that races a real one is a no-op,
  because the logic tier removes the connection and ignores a stale detach
  (Phase 1's supersede guard).
- **Alternatives**: a per-node Redis set index (`ptc.node.<id>.conns`) - deferred;
  the in-memory map already bounds the work in Phase 2. A `KEYS`/`SCAN` sweep of
  `ptc.conn.*` - rejected as unnecessary.

### D12 - Per-seat grace: simultaneous disconnects keep the room
The room actor holds a grace deadline **per seat**, not one shared slot. A detach
never frees the opponent's seat; each seat is released only when its own grace
expires (or the player re-attaches, which cancels its own deadline). So two
simultaneous disconnects keep the room alive for both to reconnect; a seat whose
grace expires while the other player is connected forfeits to that player; and
if every grace expires with no one connected, the room is dropped with no result.
- **Why**: the transport tier can close every connection of a node at once (a
  lost lease, a crash), and the load balancer routinely puts both players on the
  same node, so two disconnects arriving together are normal, not exotic. The
  one-slot grace turned that into a destroyed game (BUG-P2-R1) and made the
  transport's "reconnect" close a lie.
- **Alternatives**: keep the single slot and drop the room when the second player
  leaves (rejected: destroys live games and breaks the reconnect contract);
  stagger the transport's closes (rejected as unreliable and incomplete).

### D13 - Phase 3: ownership, leases, and abandoned connections
Phase 3 lets M logic instances run, each owning a subset of rooms, reusing the
Phase 1/2 primitives.
- **Ownership store**: `room:<code> -> node` in Redis, claimed with `SET NX`
  (+ TTL) and released with a compare-and-delete Lua script; the transport
  re-resolves a stale or unknown owner before dropping a frame (the `ownership`
  module already implements claim/release/get/renew).
- **Logic-node leases**: each logic node holds a `ptc.logic_node.<id>` lease,
  self-healing on renewal (`SET EX`, not `EXPIRE`) exactly like the transport
  lease (D11); an expired lease makes its rooms eligible for re-claiming.
- **Failover without double ownership**: re-claiming goes through the ownership
  store's atomic compare-and-set, so a room never has two owners. Game state is
  not replicated, so a lost room does not resume: its ownership record is freed
  and its players get a clear terminal close (the transport watches logic-node
  membership), not silence.
- **Ownership is kept alive, not just refreshed**: a logic node renews the
  ownership of every room it still holds and re-claims any whose key was lost
  (DEL, expiry, or a broker restart wiping the keyspace), so an active room is
  never left ownerless. The transport reads the ownership record fresh on every
  join rather than caching it (BUG-P3-1).
- **Abandoned connections**: a disconnect lost during a broker outage must not
  block a player forever; node/liveness detection or a bounded per-connection
  TTL reclaims it, and the identity can always re-join its room (spec
  `room-routing`, "Abandoned Connection Reclamation").
- **Single writer**: durable records keep a single writer (or per-instance
  partitioning); shared session/auth state moves to Redis with SQLite as the
  durable mirror.
- **Why**: reuse the existing primitives (publish retry, self-healing leases,
  per-seat grace) rather than inventing new ones for Phase 3.

## Interface Sketch

Concrete layout the implementation follows, so both tiers compile from the same
definitions.

### Workspace layout

Two binaries in the existing `chess-server` crate, sharing `server/src/lib.rs` -
not a sibling crate:

```
server/
  src/lib.rs                              # unchanged module roots
  src/main.rs                             # unchanged monolith binary (TIER=all)
  src/bin/ws-server.rs                    # tiered entry: all | transport | logic
  src/infrastructure/broker.rs            # Redis client + envelopes + channels
  src/infrastructure/transport.rs         # shared connection handling (from ws.rs)
  src/infrastructure/transport_handler.rs # transport tier: accept + bridge + dispatch
  src/infrastructure/logic_handler.rs     # logic tier: consume + dispatch + forward
```

`ws-server.rs` is the tiered entry point: `Tier::Transport` starts only the
transport handler and **does not open the database**; `Tier::All`/`Tier::Logic`
build the `App` first.

### Broker module (`infrastructure/broker.rs`)

```rust
pub const ENVELOPE_VERSION: u16 = 1;
pub const DEFAULT_LOGIC_NODE: &str = "logic"; // Phase 1: one logic instance

#[serde(rename_all = "snake_case")]
pub enum InboundPayload { Client(ClientMessage), Disconnect }

pub struct InboundEnvelope {
    pub v: u16,                 // ENVELOPE_VERSION
    pub connection_id: String,
    pub node_id: String,
    pub payload: InboundPayload,
}

pub struct OutboundEnvelope {
    pub v: u16,                 // ENVELOPE_VERSION
    pub node_id: String,
    pub connection_id: String,
    pub payload: ServerMessage,
}

pub fn inbound_channel(logic_node: &str) -> String;  // "ptc.ws.in.<logic_node>"
pub fn outbound_channel(node_id: &str) -> String;    // "ptc.ws.out.<node_id>"
pub fn room_channel(code: &str) -> String;           // "ptc.room.<code>"  (Phase 2)
pub const SITE: &str = "ptc.site";                   // (optional; non-goal)

pub struct Broker { /* shared ConnectionManager + dedicated pub/sub conns */ }

impl Broker {
    pub async fn connect(url: &str, node_id: String) -> redis::RedisResult<Self>;
    pub async fn publish_inbound(&self, e: &InboundEnvelope) -> redis::RedisResult<()>;
    pub async fn publish_outbound(&self, e: &OutboundEnvelope) -> redis::RedisResult<()>;
    pub async fn subscribe_inbound(&self) -> redis::RedisResult<PubSub>;
    pub async fn subscribe_outbound(&self) -> redis::RedisResult<PubSub>;

    // Phase 2 (design D11): transport-node liveness + the per-node connection index.
    pub async fn register_transport_node(&self) -> redis::RedisResult<()>;
    pub async fn renew_transport_node(&self) -> redis::RedisResult<bool>;
    pub async fn add_node_connection(&self, connection_id: &str) -> redis::RedisResult<()>;
    pub async fn remove_node_connection(&self, connection_id: &str) -> redis::RedisResult<()>;
    pub async fn node_connections(&self, node_id: &str) -> redis::RedisResult<Vec<String>>;
}
```

Phase 2 adds transport-node liveness (design D11): each transport node renews
`ptc.node.<node_id>` and maintains the `ptc.node.<node_id>.conns` set; the logic
tier sweeps expired node leases and synthesizes a disconnect for each indexed
connection.

Redis dependency: `redis = { version = "0.26", features = ["tokio-comp",
"connection-manager", "script"] }`. Commands go over a shared
`ConnectionManager`; pub/sub uses a **dedicated connection** (a manager
connection cannot enter subscriber mode). The dispatcher creates its message
stream **once** and reuses it - re-creating it per iteration drops messages
that arrive between polls.

### Tier seams (the room actor is unchanged)

The room actor still writes each seat's `ServerMessage`s to its
`mpsc::UnboundedSender<ServerMessage>`; `room_actor.rs` is untouched. The
transport tier owns one `Outbox` per socket and a single `connection_id ->
Outbox` map that its dispatcher fans outbound envelopes through. The logic tier
keeps one `LogicConnection` per `connection_id` and drains each `Conn.out_rx`
into an outbound envelope. No new trait is required.

## Touch Map

What happens to each existing symbol. Anything marked **unchanged** MUST NOT be
rewritten by this change.

| Existing symbol | Action |
| --- | --- |
| `interface/protocol.rs` (`ClientMessage`, `ServerMessage`, `VERSION`, `PROTOCOL_CLOSE_CODE`, `decode_incoming`) | **Unchanged.** Both tiers import it. |
| `infrastructure/ws.rs` (`Outbox`, `Outbound`, `is_state_class`, `write_loop`, `send_json`, `protocol_close`, `KEEPALIVE_INTERVAL`) | **Move** to `infrastructure/transport.rs`, shared by both binaries. |
| `infrastructure/ws.rs` (`handle_socket`, `ws_upgrade`, `healthz`) | **Stay** as the monolith (`TIER=all`) path. |
| `infrastructure/ws.rs` auth (`register`, `login`, `logout`, `set_profile`, `issue_session`, `persist`) | **Move into `App`** (`server.rs`) so the logic tier can call them; the transport never handles auth (D5). |
| `infrastructure/ws.rs::handle_text` | **Split**: the transport validates + forwards a typed `ClientMessage`; the logic tier (`logic_handler.rs`) dispatches it into `App`/rooms and publishes outbound. |
| `infrastructure/server.rs` (`App`, `Conn`, `create_room*`, `join_room`, `resolve_identity`, `link_*`, auth methods) | **Stay on the logic tier.** In `TIER=logic`, `Conn`'s mailbox/`out_rx` are drained by `logic_handler`. |
| `application/room_actor.rs` (`RoomMsg`, `run_room`, `Room`, `Seat`) | **Changed (D12)**: grace is per seat so simultaneous disconnects keep the room; the seat's `mpsc` sender/receiver is reused as-is. |
| `infrastructure/broker.rs` (new) | Redis envelopes, channels, registry, ownership, membership. |
| `infrastructure/transport_handler.rs` (new) | Transport tier: accept, validate, forward, the single node dispatcher, disconnect publication. |
| `infrastructure/logic_handler.rs` (new) | Logic tier: consume envelopes in series, dispatch, forward room output, handle disconnect, and reconnect/exit on broker loss (D10). |
| Per-tier health | The monolith and transport serve `ws::healthz`; the logic tier binds its own `GET /healthz` (D10). |
| Client endpoint + version handshake | **Unchanged**; clients observe no difference (D5, `realtime-transport`). |

## Risks / Trade-offs

- **[Added hop latency]** client → ws → Redis → logic → Redis → ws → client →
  **Mitigation**: Redis pub/sub is sub-millisecond locally; coalescing already
  minimizes frames; measure against the current path and keep the `all` mode
  as the latency baseline.
- **[Pub/sub is fire-and-forget]** a dropped envelope could lose an inbound
  move → **Mitigation**: address per connection, keep accepted-message handling
  idempotent, and reconcile with full snapshots (the existing resync contract);
  evaluate Redis Streams if measured loss matters.
- **[Redis becomes a single point of failure]** → **Mitigation**: run a
  managed/replicated Redis with persistence; both tiers fail fast and never
  serve with a dead broker (spec `realtime-transport`).
- **[Duplicated connection logic]** keepalive/coalescing could drift between
  the two tiers → **Mitigation**: move it exclusively into the transport tier
  and delete it from the logic tier.
- **[SQLite concurrent writers in Phase 3]** → **Mitigation**: single-writer
  designation or per-instance partitioning (spec `room-routing`).
- **[Session/auth not shared across logic nodes]** → **Mitigation**: shared
  Redis session store in Phase 3; SQLite remains the durable mirror.
- **[Operational complexity grows]** more processes, a load balancer, Redis →
  **Mitigation**: per-tier health endpoints, docker-compose for local, and CI
  jobs that exercise the split; `all` mode remains the simple path.
- **[Credentials on the fabric]** login passwords and session tokens traverse
  Redis inside the envelope payloads → **Mitigation**: production Redis MUST
  require `AUTH` and TLS (or a private network), and no tier may log payloads.
  Out of Phase 1 code scope, recorded as an operational requirement.

## Migration Plan

1. **Phase 1** - add `ws-server`, the broker module, and `TIER`/`REDIS_URL`
   config; run transport + logic as two processes (no load balancer). Verify
   protocol fidelity with the existing socket tests plus a split-mode E2E.
   Default remains `TIER=all`.
2. **Phase 2** - run N `ws-server` instances behind nginx (no sticky sessions
   required); wire the Redis connection registry into routing, add transport-node
   liveness so a crashed node's seats enter grace (D11), and verify clients on
   different nodes see the same rooms.
3. **Phase 3** - run M logic nodes; add the ownership map, node leases, and
   failover; move shared session state to Redis and enforce the single-writer
   rule for SQLite.
4. **Rollback** - set `TIER=all` and drop Redis from the deployment.

## Test Strategy

- **Default `cargo test` stays Redis-free**: `TIER=all` is the default and every
  existing socket/room/unit test keeps running without a broker.
- **Split-mode tests are env-gated**: the transport+logic+Redis integration
  tests (tasks 2.1-2.5, 3.x, 4.x) are marked `#[ignore]` and run only when
  `REDIS_URL` is set, e.g.
  `REDIS_URL=redis://localhost:6379 cargo test -- --ignored`. A `make test-split`
  target wraps this with `--test-threads=1`: the Phase 1 inbound channel is a
  single shared channel, so split tests must not run concurrently.
- **Local broker**: a `docker-compose.yml` with one `redis:` service provides it;
  run `docker compose up -d redis` before `make test-split`.
- **CI**: the split test job runs Redis as a service container and exports
  `REDIS_URL`; the existing `TIER=all` job is unchanged.
- **Pure unit tests need no Redis**: envelope (de)serialization, channel naming,
  config parsing (`TIER`/`REDIS_URL`), and coalescing stay unit tests.
- **Failure-mode coverage lives in the ignored split suite**: `broker_loss_surfaces`
  (broker dropped → clients disconnected, then recovery),
  `dead_node_seats_enter_grace` (a dead node's seats enter grace, with a
  detection-below-grace assertion), and `lease_lost_while_alive_forces_reconnect`
  (a paused/lapsed node reconnects its players) join the full-game, reconnect,
  and D8 regressions.

## Open Questions

- Which Redis topology to operate (managed service vs self-hosted with
  replica + Sentinel): deferrable; does not change the specs or task
  breakdown.
- Whether pub/sub is sufficient or Redis Streams are needed for reliability:
  decide after measuring loss/ordering in Phase 1; the specs' reconciliation
  contract holds either way.
- **Phase 1 residual (OBS-2)**: a single transient "failed to renew logic node
  lease" warning can appear right after a broker reconnect; it self-heals.
  Cosmetic; consider lowering it to debug.
- **Phase 1 residual (GAP-1)**: the logic tier's non-zero exit when the broker
  stays down is verified manually; automating it needs a Redis the test can kill
  (environment-dependent). Consider a process test gated on a `redis-server`
  binary.
- **Phase 2 residual (real kill E2E, QA GAP-1)**: the node-crash test drops the
  client rather than killing a transport process; a true `kill -9` end-to-end
  (spawning the transport as a child process) is exercised only manually.
- **Phase 2 residual (incremental re-sync, QA GAP-5)**: an incrementally
  coalesced `Update` can be dropped for a very slow client, leaving its local
  board behind; a full snapshot only re-syncs on connect. Consider a re-sync on
  a detected gap; deferrable.
- **De-flaked (QA BUG-P2-3)**: the pre-existing flaky `auth`
  test `guest_and_authenticated_play_identically_over_the_wire` now reads until
  the expected move count instead of assuming one frame per move.
- **Phase 3 residual (QA-P3-E4)**: account/login state is not shared across logic
  partitions - a login routed to a different node returns `invalid_credentials`,
  and the same username can be registered on two nodes. Sessions are shared via
  Redis but accounts are not; decide whether to share accounts (like sessions) or
  pin auth to a single node.
- **Phase 3 deployment note**: run each logic instance with its own SQLite file
  (partitioned), the spec-allowed alternative to a single writer; do not point
  two logic instances at the same SQLite file (concurrent writers can hit
  `SQLITE_BUSY`).
- **Phase 3 residual (QA RESIDUAL-P3-C, minor)**: for up to ~10 s after a room's
  ownership key is lost (before the owning node re-claims it), a join whose owner
  is absent from the membership list can get `room_not_found`; it self-heals on
  the next re-claim. No production trigger for a stale owner on a live room;
  possible follow-up: on `room_not_found`, re-resolve and try the next candidate.
- **Client E2E deferred to release (task 5.1)**: the iOS-simulator/Android-emulator
  end-to-end check is a manual release gate. The change leaves the clients
  (`core/`, `ios/`, `android/`) and the wire protocol untouched, and the split
  tests already assert protocol fidelity, so it is not run as part of this change.
- **Phase 2 residual (ghost connections, QA BUG-F1-R2)**: after a broker loss
  some `disconnect` publishes are lost (the outage outlasts the publish retry),
  so the logic keeps connections and a reused device sees `already_in_room` until
  the room/lease expires; re-joining the same room still recovers. A proactive
  prune on recovery (or a per-connection TTL) is future work.
