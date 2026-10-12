# Design

## Context

See `proposal.md` - Why. The fabric is Redis pub/sub. A single instance is the
shared resource every move crosses: the measured per-node ceiling (~2,650-2,700
games at move-ack p95 = 100 ms on the M2 Max reference) does not scale
horizontally (2 logic nodes ~1.10x, 3 ~1.05x), and the earlier fabric changes
explicitly deferred "sharding Redis" as a separate concern. The implementation
landed in the working tree (`4eeae20`).

## Goals / Non-Goals

**Goals:**
- Spread per-room fabric traffic across N Redis shards so distinct rooms no
  longer serialize on one bus.
- Preserve per-room / per-connection ordering (a room stays on one shard).
- Keep the client wire protocol and every message identical.
- Make the change measurable (1 shard vs N shards on the same harness).

**Non-Goals:**
- Room ownership/routing across logic nodes (the other half of horizontal
  scaling) - separate.
- Replacing Redis or changing the fabric's message format.
- Any client or FFI change.

## Decisions

### D1 - Shard count from configuration, with a 1-shard fallback
`REDIS_URLS` (comma-separated) sets the shards; with only `REDIS_URL` the system
runs a single shard (the previous behavior). A non-`all` tier refuses to start
without at least one URL. This keeps a sharded deploy opt-in and the default
unchanged.

### D2 - Route by key, so a room stays on one shard
A shard is chosen by `shard_index(key, n_shards)` over the **room code** (or the
connection id when a message is not room-scoped). Because a room maps to exactly
one shard, its messages keep their order on that shard and never split across
shards.

### D3 - A dedicated control shard for low-volume lifecycle work
Node leases, the connection registry, and "list every logic node" are low-volume
and need a single consistent view; they always use shard 0 (the control shard),
avoiding a cross-shard consistency problem for lifecycle operations.

### D4 - Shard-aware publishing with per-shard connections and retry
Every publish path carries its shard; each shard has its own client / manager /
lazily-opened publish connection, so publishing or retrying on one shard never
contends with another. Connection failures drop only that shard's cached
connection.

### D5 - The outbound batcher is per shard
The per-transport-node outbound batcher spawns **one `BatchShard` per Redis
shard**, each permanently bound to that shard, so batching and pipelining happen
within a shard and per-room order holds.

### D6 - Behavior preserved
The client wire protocol, message fields/order, and all close/keepalive behavior
are unchanged. The FFI and mobile clients are untouched.

## Risks / Trade-offs

- [Order across shards] -> avoided by D2: a room is on exactly one shard, and the
  batcher is per shard; a per-room order test guards it.
- [Cross-shard consistency for lifecycle] -> avoided by D3: lifecycle ops stay on
  the control shard.
- [A shard becomes the new hot spot] -> shard by room spreads rooms; measure the
  distribution and how many shards are needed.
- [No measured payoff yet] -> the sharded path was never exercised: all prior
  measurements used the 1-shard default, so the scaling claim is unproven until
  the N-shard A/B is run.

## Open Questions

- How many shards are needed to lift the ceiling, and where does it saturate?
- Does sharding actually raise the per-node/aggregate ceiling on the harness, or
  is the socket-write floor still binding (as the egress work suggests)?
- Room ownership + a shared DB (SQLite is node-local) are still required for true
  multi-node horizontal scaling.
