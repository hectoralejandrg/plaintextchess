# Proposal

## Why

The online server is a single process that terminates every WebSocket
connection, owns all game logic, and keeps all room state in memory. Capacity
can therefore only grow by adding RAM/CPU to that one machine, and our
estimate showed a single 512 MB node tops out at a few thousand concurrent
games. The ceiling is the monolithic process, not chess: terminating a
connection and relaying JSON is cheap, while applying game rules and writing
durable records is expensive. Lichess scales by separating the cheap
connection tier from the expensive logic tier and brokering events over Redis,
which lets connection capacity grow by adding nodes. We adopt the same shape,
in phases, keeping SQLite for durable data.

## What Changes

A phased migration to a tiered realtime architecture, using Redis as the event
fabric and keeping SQLite for durable auth/game records. Each phase is its own
capability and its own spec.

- **Phase 1 — dedicated transport tier** (`realtime-transport`): extract a
  standalone `ws-server` process that terminates client WebSockets and bridges
  to the existing logic server over Redis pub/sub. The logic server stays a
  single authoritative instance, and the client endpoint and wire protocol are
  unchanged.
- **Phase 2 — transport scale-out** (`transport-scale-out`): run N `ws-server`
  instances behind a load balancer with a Redis-backed connection registry, so
  connection capacity scales horizontally while one logic instance remains
  authoritative.
- **Phase 3 — room routing** (`room-routing`): allow M logic instances, with
  each room owned by exactly one instance. Redis holds the room→node ownership
  map, `ws-server` routes each connection's frames to the owning node, and node
  membership, leases, and failover are defined.
- **BREAKING**: Redis becomes a required runtime dependency, and deployment
  changes (a new process/binary, a load balancer, multiple instances). The
  mobile clients are not expected to change.

## Capabilities

### New Capabilities
- `realtime-transport`: WebSocket termination separated from game logic and
  bridged by Redis pub/sub, preserving the client wire protocol, message
  ordering, backpressure/coalescing, keepalive, and reconnect semantics.
- `transport-scale-out`: multiple stateless `ws-server` nodes behind a load
  balancer with a Redis-backed connection registry (routing fallback and
  stale-node detection) and transport-node liveness, so a crashed node's seats
  still enter the reconnect grace.
- `room-routing`: room→node ownership, routing of client frames to the owning
  logic node, node membership/leases, and failover.

### Modified Capabilities
- `server`: a simultaneous disconnect of both players now holds each seat for
  its own grace window instead of dropping the room immediately, so both players
  can reconnect and resume (previously the second disconnect dropped the room
  with no result). The rest of the `server` requirements are unchanged.

## Impact

- **Server code**: split the connection handling in
  `server/src/infrastructure/ws.rs` into a new `ws-server` binary; add a broker
  module (Redis client, channels per room/user/site); `main.rs`/`config.rs` gain
  the tier role and `REDIS_URL`; add Redis client dependencies.
- **Deployment**: Redis becomes required; a second binary/process appears in
  Phase 1, multiple instances and a load balancer in Phases 2–3.
- **Persistence**: SQLite is retained. In Phases 1–2 durable writes stay on the
  single logic instance; Phase 3 must keep a single writer for records or
  partition them per node (documented trade-off).
- **Clients (iOS/Android)**: no wire-protocol or UI changes expected.
- **Specs**: new `realtime-transport`, `transport-scale-out`, `room-routing`.
