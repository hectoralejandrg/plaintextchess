# Tiered Realtime Architecture

This document describes the tiered server architecture introduced to enable horizontal scaling of the WebSocket transport tier while keeping the authoritative game logic in a separate tier.

## Overview

The server can run in three modes controlled by the `TIER` environment variable:

| Mode | Description | Use Case |
|------|-------------|----------|
| `all` (default) | Single process: WebSocket termination + game logic + persistence | Development, testing, simple deployments |
| `transport` | WebSocket termination only, bridges to logic tier via Redis | Scaling connection capacity horizontally |
| `logic` | Authoritative game logic only, receives/sends via Redis | Running multiple logic nodes (Phase 3) |

## Quick Start

### Monolith (Default - TIER=all)

```bash
# No extra configuration needed
cargo run --manifest-path server/Cargo.toml --bin ws-server

# Or with custom config
BIND=0.0.0.0 PORT=8765 DATABASE_URL=sqlite:./data/chess.db cargo run --manifest-path server/Cargo.toml --bin ws-server
```

### Split Mode (TIER=transport + TIER=logic)

**Prerequisites:**
- Redis server running (e.g., `redis-server` on localhost:6379)

**Terminal 1 - Logic Tier:**
```bash
TIER=logic REDIS_URL=redis://127.0.0.1:6379 \
DATABASE_URL=sqlite:./data/chess.db \
cargo run --manifest-path server/Cargo.toml --bin ws-server
```

**Terminal 2 - Transport Tier:**
```bash
TIER=transport REDIS_URL=redis://127.0.0.1:6379 \
BIND=0.0.0.0 PORT=8765 \
cargo run --manifest-path server/Cargo.toml --bin ws-server
```

The transport tier listens on `PORT` (default 8765) for client WebSocket connections. The logic tier does not listen for external connections.

## Environment Variables

| Variable | Description | Default | Required For |
|----------|-------------|---------|--------------|
| `TIER` | Server role: `all`, `transport`, `logic` | `all` | All modes |
| `REDIS_URL` | Redis connection URL (e.g., `redis://host:6379`) | - | `transport`, `logic` |
| `BIND` | Bind address | `0.0.0.0` | `all`, `transport`, `logic` |
| `PORT` | Listen port (each tier process sets its own) | `8765` | `all`, `transport`, `logic` |
| `DATABASE_URL` | SQLite database URL | `sqlite:./data/chess-server.db` | `all`, `logic` |
| `RECONNECT_GRACE_SECS` | Reconnect grace window (seconds) | `120` | All modes |
| `SESSION_TTL_SECS` | Session lifetime (seconds) | `2592000` (30 days) | All modes |
| `ARGON2_MEMORY_KIB` | Argon2 memory cost (KiB) | `19456` | All modes |
| `ARGON2_TIME_COST` | Argon2 time cost | `2` | All modes |
| `ARGON2_PARALLELISM` | Argon2 parallelism | `1` | All modes |
| `LOGIC_LANES` | Number of inbound processing lanes for logic tier (room-scoped messages); `hash(room_code) % K` partitioning preserves per-room order | `1` (serial) | `logic` |
| `LOGIC_CONTROL_LANES` | Number of control-plane lanes for logic tier (create room + auth); keyed by `connection_id` to isolate auth from moves | `1` | `logic` |

## Architecture

```
┌─────────────┐     ┌─────────────┐     ┌─────────────┐
│   Client    │────▶│  Transport  │────▶│    Logic    │
│  (iOS/Android│     │   (WS)      │     │   (Rooms)   │
│   Web)      │◀────│  Nodes      │◀────│   + Redis   │
└─────────────┘     └──────┬──────┘     └─────────────┘
                           │
                    ┌──────┴──────┐
                    │    Redis    │
                    │  (Pub/Sub)  │
                    └─────────────┘
```

### Transport Tier (`TIER=transport`)
- Accepts WebSocket connections from clients; **does not open the database**
- Handles protocol framing, keepalive, coalescing, protocol-close
- Assigns each connection a unique `connection_id` and its own `node_id`
- Validates each inbound frame with `decode_incoming`, then publishes it as an
  envelope to Redis (`ptc.ws.in.logic`)
- Subscribes **once per node** to outbound envelopes on `ptc.ws.out.{node_id}`
  and fans each to its connection via an in-memory `connection_id -> Outbox` map
- Publishes a `disconnect` event when a socket closes, so the logic tier can
  honour the reconnect grace exactly as the monolith did
- Re-establishes its outbound subscription on broker loss (bounded backoff) and,
  while the broker is down, closes live clients with `1011 broker unavailable`
  (never a silent socket)
- Holds **no authoritative game state** (rooms, boards, clocks, ratings)

### Logic Tier (`TIER=logic`)
- Owns authoritative room state (board, move list, clocks, ratings)
- Subscribes to inbound envelopes from transport tier
- **Processes inbound messages through a sharded lane model:**
  - A single reader parses envelopes from Redis and routes them to `LOGIC_LANES` bounded `mpsc` channels.
  - Room-scoped messages (join by room code, moves, resigns, leaves, and disconnects for a seated connection) are routed to lane `hash(room_code) % K`, so a room's messages stay ordered while independent rooms run in parallel.
  - Control-plane messages (create room and auth) are routed to `LOGIC_CONTROL_LANES` channels keyed by `connection_id`, isolating argon2 auth and create-room Redis round trips from the move path.
  - Lanes are bounded; when a lane is full the reader pauses (backpressure) rather than growing memory without limit. Because there is a single reader, a saturated lane also backpressures the other lanes (a documented limitation; per-shard consumption is the future fix).
  - Setting `LOGIC_LANES=1` (the default) reproduces the original serial behavior.
- Runs room actors (`run_room`) that apply game rules
- Publishes outbound `ServerMessage` envelopes to the correct transport node
- Handles authentication, session management, persistence (SQLite)
- Places each new room on a live logic instance in round-robin order, so
  several instances share new rooms (a single instance takes every room)
- Exposes `GET /healthz` on `BIND`/`PORT` (per-tier health)
- Re-establishes its inbound subscription on broker loss; if the broker stays
  down past a bounded number of attempts it exits non-zero so a supervisor
  notices

### Redis Fabric
- **Inbound channel**: `ptc.ws.in.logic` (transport → logic)
- **Outbound channel**: `ptc.ws.out.{transport_node_id}` (logic → transport)
- **Room broadcasts**: `ptc.room.{room_code}` (Phase 2+)
- **Site broadcasts**: `ptc.site` (Phase 2+)
- **Connection registry**: `ptc.conn.{connection_id}` (Phase 2+)
- **Room ownership**: `ptc.room_owner.{room_code}` (Phase 3)
- **Logic node membership**: `ptc.logic_node.{node_id}` (Phase 3)
- **Shared session mirror**: `ptc.session.{token_hash}` (Phase 3)

## Message Envelopes

All cross-tier messages use versioned JSON envelopes. The envelope version
(`ENVELOPE_VERSION`) is separate from the client wire `VERSION`.

**Inbound (Transport → Logic):** the payload is either a validated client
message or a `disconnect` control event.

```json
{
  "v": 1,
  "connection_id": "uuid-v4",
  "node_id": "transport-abc123",
  "payload": { "client": { "type": "create_room", "v": 1, "player_id": "device-1" } }
}
```

```json
{
  "v": 1,
  "connection_id": "uuid-v4",
  "node_id": "transport-abc123",
  "payload": "disconnect"
}
```

**Outbound (Logic → Transport):**

```json
{
  "v": 1,
  "node_id": "transport-abc123",
  "connection_id": "uuid-v4",
  "payload": { "type": "room_ready", "v": 1, "room_code": "ABC123" }
}
```

## Running Tests

### Unit Tests (no Redis required)
```bash
cargo test --manifest-path server/Cargo.toml --lib
```

### Integration Tests (no Redis required - runs in TIER=all mode)
```bash
cargo test --manifest-path server/Cargo.toml --test rooms
cargo test --manifest-path server/Cargo.toml --test auth
cargo test --manifest-path server/Cargo.toml --test integration
```

### Split-Mode E2E Tests (requires Redis)
```bash
# Start the broker (docker) or a local redis-server
docker compose up -d redis      # or: redis-server &

# Run the split-mode tests (marked #[ignore] by default) via the Make target
make test-split

# Equivalent to:
cd server && REDIS_URL=redis://127.0.0.1:6379 \
  cargo test --test split_e2e -- --ignored --test-threads=1
```
The Phase 1 inbound channel is a single shared channel, so the split tests run
single-threaded.

## Rollback

To rollback to monolith mode, simply deploy with `TIER=all` (or unset `TIER`) and remove the Redis dependency. No code changes required.

## Phase Roadmap

- **Phase 1** (current): Single transport node + single logic node, Redis pub/sub bridge
- **Phase 2**: Multiple transport nodes behind load balancer, connection registry, cross-node delivery
- **Phase 3**: Multiple logic nodes, room ownership, leases, failover

## Troubleshooting

### "REDIS_URL must be set when TIER=transport or TIER=logic"
Set the `REDIS_URL` environment variable to point to your Redis instance.

### Connection drops immediately
Check Redis connectivity: `redis-cli -u $REDIS_URL ping` should return `PONG`.

### A split tier exits at startup
Both `TIER=transport` and `TIER=logic` require `REDIS_URL` and a reachable
Redis; they refuse to start otherwise (design D8, spec `realtime-transport`).

### Clients disconnect with "broker unavailable" (1011)
The transport lost its Redis subscription and closed live clients instead of
leaving them silent (design D10). It reconnects with backoff; once Redis is
back, new connections are served again. If the logic tier cannot resubscribe
after a bounded number of attempts it exits non-zero so a supervisor restarts
it.

### "protocol error" on connect
Ensure the client sends `{"type":"create_room","v":1,...}` or `{"type":"join_room","v":1,...}` as the first message after the WebSocket handshake.

## Multi-Node Transport Scale-Out (Phase 2)

### Overview

Phase 2 enables running **multiple transport nodes** behind a load balancer, while a single logic tier remains authoritative. Transport nodes are completely stateless and interchangeable — any node can serve any client because:

1. The connection registry (`ptc.conn.{connection_id}`) maps each connection to its owning transport node
2. The logic tier routes outbound messages to the node recorded in the registry
3. Clients can reconnect to any node within the grace window and re-attach to their session

**No sticky sessions are required.** The load balancer can distribute connections round-robin.

### Docker Compose Multi-Node Setup

The repository includes a `docker-compose.yml` that starts the full multi-node stack:

```bash
# Build and start: Redis + logic + 2 transport nodes + NGINX reverse proxy
make multi-node-up
# or: docker compose up -d --build
```

This starts:
- `redis` — Redis 7 event fabric
- `logic` — Authoritative logic tier (TIER=logic, port 3000)
- `transport-1` — First transport node (TIER=transport, port 3001)
- `transport-2` — Second transport node (TIER=transport, port 3002)
- `proxy` — NGINX reverse proxy (port 8080, round-robin)

### Load Balancer Configuration (NGINX)

The `config/nginx-lb.conf` provides a ready-to-use reverse proxy configuration:

```nginx
upstream transport_nodes {
    server transport-1:3001;
    server transport-2:3002;
    # No ip_hash, no sticky sessions — round-robin is correct!
}
```

Key points:
- **WebSocket upgrade headers** are forwarded (`Upgrade`, `Connection`)
- **No sticky sessions** — round-robin distribution is correct because routing targets the connection's owning node, not the node a client keeps hitting
- `proxy_read_timeout 3600s` — long timeout to accommodate WebSocket keepalive pings (every 20s)

### Verifying the Multi-Node Setup

```bash
# Run the smoke test (requires docker compose running)
make multi-node-smoke
```

This verifies:
1. `/healthz` through the proxy returns 200 with `tier: "transport"` and a `node_id`
2. Two consecutive healthz requests hit different transport nodes (proves round-robin without sticky sessions)

### Running Clients Through the Proxy

Clients connect to the proxy's `/ws` endpoint:

```bash
# WebSocket endpoint through the load balancer
ws://localhost:8080/ws
```

The proxy forwards to one of the transport nodes. All subsequent routing (create room, join, moves) works identically to the single-node case because the logic tier routes responses to the correct node via the connection registry.

### Health Checks

Each transport node exposes `GET /healthz` reporting:
```json
{
  "status": "ok",
  "tier": "transport",
  "node_id": "transport-abc123",
  "protocol_version": 1
}
```

The logic tier similarly reports `tier: "logic"` with its node id.

### Transport Node Liveness (Design D11)

Each transport node registers a lease key `ptc.node.{node_id}` with TTL 30s, renewed every 10s. If a node crashes without publishing disconnects for its sockets, its lease expires. The logic tier's liveness watcher detects the missing lease (after 2 consecutive failed checks at 5s intervals) and synthesizes `Disconnect` events for all connections registered to that node. Their seats enter the reconnect grace, opponents observe them as offline, and clients can reconnect to any node within the grace window.

### Scaling

To add more transport nodes:
1. Add a new service to `docker-compose.yml` (e.g., `transport-3`)
2. Add the new node to the nginx `upstream transport_nodes` block
3. Run `docker compose up -d --build` (or `make multi-node-up`)

### Stopping the Multi-Node Stack

```bash
make multi-node-down
# or: docker compose down -v
```

## Multi-Logic Room Routing (Phase 3)

### Overview

Phase 3 lets several **logic** instances run together, each owning a
subset of rooms. The transport tier routes every frame to the logic
instance that owns the relevant room or that seats the relevant
connection, so the client wire protocol and behavior are unchanged
from `TIER=all`.

Each logic instance owns its own in-memory `App` (rooms, seats, auth
cache). The Redis fabric carries the shared state that lets the
instances cooperate:

- **Room ownership** (`ptc.room_owner.{room_code}`): the logic
  instance that created a room is its owner and the sole writer of
  that room's state. Claimed with `SET NX` on create (only one
  writer can win), released by an atomic Lua script on removal, and
  renewed on a lease so a dead owner's rooms can be reclaimed.
- **Logic node membership** (`ptc.logic_node.{node_id}`): a lease
  (TTL 30s, `SET EX`, renewed every 10s) that lets the transport
  tier discover live logic instances. A missing key is healed on the
  next renewal (`SET EX`, never `EXPIRE`, so a lost key is
  re-registered instead of silently dropped).
- **Per-connection affinity**: the transport caches
  `connection_id -> logic_node_id`. A connection's room-scoped
  frames (move/resign/leave/disconnect) are routed to the instance
  that seats it, so several logic instances never mis-route a move.
  JoinRoom is routed to the room's owner; other frames fall back to
  any live instance, probing subscribers and skipping stale members.
- **Shared session mirror** (`ptc.session.{token_hash}`): a logic
  instance that issues a session mirrors the token's SHA-256 hash
  (account id, username, display name, expiry) to Redis, so any
  other instance accepts the same token. The plaintext token never
  leaves the issuing instance (design D6). SQLite remains the
  durable mirror, written by the issuing instance (one row per
  session, so instances never write the same record concurrently).
  Logout deletes the mirrored key, revoking the token everywhere.

### Failover and Re-Claiming

Rooms are reclaimed only within the bounded window of the ownership
lease. Because the claim is a Redis `SET NX` and the release/renew
paths are atomic Lua scripts, the ownership record has **exactly one
owner at every step** - there is no double-owner window, even when
two instances contend for the same room. When an owner dies, its
`ptc.room_owner.{room_code}` lease expires; another instance may
then claim the room. A re-claimed room keeps each seat's reconnect
grace (design D12), so a player whose connection was lost re-attaches
instead of being dropped.

### Failover Procedure

1. **Detect**: the ownership lease (`ptc.room_owner.{room_code}`)
   expires after the TTL; the membership lease
   (`ptc.logic_node.{node_id}`) expires after 30s without renewal.
2. **Re-route**: the transport tier skips instances whose membership
   lease is gone and probes subscribers before publishing, so frames
   never black-hole on a dead instance.
3. **Re-claim**: a surviving logic instance claims the orphaned room
   with `SET NX`; exactly one instance wins.
4. **Recover**: clients reconnect to any transport node within the
   grace window and re-attach; a session issued before the failover
   is still accepted because it lives in the shared mirror.

### Abandoned Connections

A disconnect lost during a broker outage must not block a player
indefinitely (design D13). The identity can re-join its room within
the grace window - re-attached with an up-to-date snapshot - even
when it reconnects through a different transport node. An abandoned
connection's registry entry (`ptc.conn.{connection_id}`) carries a
bounded TTL, so it is reclaimed even if its disconnect was never
delivered, and transport-node liveness (D11) synthesizes disconnects
for a crashed node's connections.

### Lease TTL Invariant (Task 5.2)

The logic tier asserts at startup that the room ownership TTL safely
exceeds the renewal sweep interval plus the worst-case sweep duration.
This prevents a live node from spuriously re-claiming its own rooms
under load.

- **Room ownership TTL**: 60 seconds (`ownership::TTL_SECS`)
- **Renewal sweep interval**: 10 seconds (`ROOM_LEASE_RENEW_INTERVAL`)
- **Maximum sweep duration**: 5 seconds (`ROOM_LEASE_RENEW_SWEEP_MAX`, one pipelined round trip)
- **Invariant**: `TTL > sweep_interval + sweep_max` → `60 > 10 + 5` ✓

If this invariant is violated at startup, the logic tier refuses to
start with a clear error message, preventing silent misconfiguration
that would cause spurious re-claims under load.

### Observability and Metrics (Task 6.1)

The logic tier's `/healthz` endpoint reports live per-lane metrics so
per-node capacity and regressions are measurable:

- **Per-lane queue depth** (`depth`): pending envelopes in that lane,
  derived from the bounded channel (`max_capacity - capacity`).
- **Per-lane processing latency** (`last_us`/`max_us`/`avg_us`):
  microseconds from the reader routing an envelope to its worker
  finishing `handle_inbound` (queue wait + processing).
- **Inbound move-dispatch latency** (`move_dispatch`): the same
  measurement restricted to `Move` envelopes. It covers routing to
  dispatch; it does not include the room actor applying the move or the
  outbound `Update` publish.

Each `room_lane_metrics[i]` / `control_lane_metrics[i]` object carries
`depth`, `processed`, `last_us`, `max_us`, and `avg_us`.

Example `/healthz` response (2 room lanes, 1 control lane):
```json
{
  "status": "ok",
  "tier": "logic",
  "node_id": "logic-abc123",
  "protocol_version": 1,
  "room_lanes": 2,
  "control_lanes": 1,
  "room_lane_metrics": [
    { "depth": 0, "processed": 1542, "last_us": 38, "max_us": 210, "avg_us": 41 },
    { "depth": 3, "processed": 1487, "last_us": 55, "max_us": 305, "avg_us": 44 }
  ],
  "control_lane_metrics": [
    { "depth": 0, "processed": 12, "last_us": 120, "max_us": 980, "avg_us": 310 }
  ],
  "move_dispatch": { "processed": 3013, "last_us": 40, "max_us": 305, "avg_us": 42 }
}
```

## Per-Node Capacity (measured)

Release build, Apple M2 Max (12 cores, 32 GB), one logic node and one transport
on the same host, driven by a self-paced load (games of 20 plies, a move every
~60 ms). Move-ack latency is measured end-to-end from the client.

| Concurrent games | Players | move-ack p95 |
|---|---|---|
| 1,000 | 2,000 | 4 ms |
| 2,000 | 4,000 | 24 ms |
| ~2,000–2,700 | ~4,000–5,400 | 100 ms (crossing) |
| 3,000 | 6,000 | 106 ms |
| 4,000 | 8,000 | 286 ms (degraded) |

Peak sustained throughput is ~23k moves/s at 2,000 games. Before this change the
ceiling was ~950–1,250 games (~2.0k–2.5k players) and ~7.9k moves/s; the gain
came from removing a per-frame fabric lookup from the transport hot path, not
from the handler lanes (4 lanes == 1 lane). The sharded lanes preserve per-room
ordering and isolate the control plane; they do not add per-node capacity.

The remaining lever is per-message lock/timer contention in the fabric
(`reduce-fabric-contention`); new rooms are now distributed across live logic
nodes, so adding logic instances multiplies capacity.

Caveats: single host, synthetic uniform load, one sample per point. These are a
per-node baseline, not a production SLA.
