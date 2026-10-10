# Design

## Context

See `proposal.md` - Why. The logic tier currently consumes its inbound channel
in `infrastructure/logic_handler.rs`: one task iterates the broker's pub/sub
stream and `await`s `handle_inbound` for every envelope in arrival order. Cheap
move dispatch (a mailbox `send`) shares that single queue with blocking
control-plane work: `handle_create_room`/`handle_join_room` await Redis
(`claim_room`, identity resolution) and `handle_auth` runs argon2, both inline.
Room actors and outbound forwarders already run as separate tasks, so the move
*work* is concurrent; the serialization is the inbound lane itself.

Measured baseline (release, M2 Max, 12 cores): ~950 concurrent games at
move-ack p95 = 100 ms; move throughput plateaus at ~6.5–6.9k/s; logic CPU ~20%;
latency is superlinear past ~750 games; create/join p95 degrade in step
(29→186 ms); Redis peaks at ~35k ops/s (not the bottleneck). A saturated
10 s-lease renewal sweep also produced 7104 spurious room re-claims.

Constraints: the client wire protocol, endpoints, close codes, and mobile apps
must not change; per-room message order is a guarantee of `realtime-transport`
and `room-routing`; SQLite stays the durable store with a single writer.

## Goals / Non-Goals

**Goals:**
- Raise per-logic-node move throughput and concurrent-game capacity by a factor
  of the configured lane count, at the same latency budget.
- Keep per-room ordering and all client-visible behavior identical.
- Stop control-plane work (create-room/auth) from delaying moves already in
  play, while room-scoped join/leave/resign keep the room's order.
- Make the renewal sweep cost a bounded number of round trips and keep it ahead
  of the lease TTL under load.
- Make per-node capacity measurable.

**Non-Goals:**
- Changing the wire protocol, clients, or the room-ownership model.
- Replicating game state or resuming lost rooms (`room-routing` failover stays
  as-is).
- Replacing Redis pub/sub in this change (evaluated as an open question).
- Scaling the transport tier (already handled; buffers were tuned separately).

## Decisions

### D1 - Shard by room with a single reader
The broker reader stays single and cheap: it parses each envelope and routes it
to one of K bounded `mpsc` lanes by `hash(room_code) % K`. A room always maps to
the same lane, so per-room FIFO is preserved while independent rooms run in
parallel on K workers. Alternatives: per-room spawned tasks (unbounded task
count and no natural backpressure) and multiple broker subscriptions (breaks
the single arrival order and adds Redis connections). Chosen for the smallest
ordering surface and a clear backpressure point.

### D2 - Isolate the control plane
Control-plane messages that are not room-scoped (create room and auth) are
routed to a separate lane pool keyed by `connection_id`, preserving per-connection
ordering (create before any follow-up) while keeping argon2 and the create-room
Redis round trip off the move path. Room-scoped actions (join by room code,
leave, resign) stay on the room lane so they keep the room's order. Alternative:
one shared pool keyed by a single hash of room-or-connection; rejected because a
slow auth would still occupy a worker a room could use.

### D3 - Bounded lanes and backpressure
Lanes are bounded. When a lane is full the reader stops pulling, applying
end-to-end backpressure instead of buffering without limit; no envelope is
dropped silently (a closed lane is logged). Because there is one reader, a
saturated lane also backpressures the other lanes - accepted as a documented
limitation, since a single lane only fills when one room is pathologically hot.
Redis pub/sub offers no broker-side backpressure, so the durable alternative -
Redis Streams with consumer groups and `XACK`, which also enables per-shard
consumption without a shared reader - is recorded as an open question rather
than adopted here.

### D4 - Batched, pipeline-renewed leases
Owned-room ownership leases are renewed with a single Redis pipeline per sweep
instead of one call per room, and the node asserts at startup that the sweep
interval plus worst-case sweep duration is less than the lease TTL (mirroring
the existing grace-window assert). This removes the self-heal re-claim storm
observed under load. Alternative: lengthen the TTL alone; rejected because it
delays genuine failover.

### D5 - Configurable lanes with a safe default
`LOGIC_LANES` (and the control-lane count) are configurable; unset uses a safe
default and zero/invalid is rejected at startup. A default of `1` must reproduce
today's behavior for tests; the documented deployment default is >1. Setting
`LOGIC_LANES=1` is the rollback switch.

### D6 - Contention on shared maps
Inbound handling currently locks `connections: AsyncMutex<HashMap<...>>` per
message. Under K lanes this becomes a shared hot lock; the map is partitioned
per lane (each lane owns the connections it seats) or moved behind a
read-optimized lock, so lanes do not serialize on it.

## Risks / Trade-offs

- [Per-room ordering regression] → a room is pinned to one lane by
  `hash(room_code)`; ordering is asserted by a dedicated test and the existing
  split E2E.
- [Reader becomes the bottleneck] → the reader only parses and routes; if it
  saturates, shard reading (Streams consumer groups) - tracked as an open
  question rather than silently assumed away.
- [Pub/sub loss during backpressure] → a paused reader can let Redis client
  output buffers overflow and drop; documented, and Streams is the mitigation if
  it proves real.
- [Too many lanes thrash cores] → the lane count is configurable and tuned with
  the load harness; the default is conservative.
- [Complexity in a correctness-critical path] → no wire or ownership change; the
  lane split is internal and covered by ordering and isolation tests.

## Migration Plan

1. Add the lane fan-out and workers behind `LOGIC_LANES`, default 1, so behavior
   is unchanged until configured.
2. Add control-plane isolation and partition the connection map.
3. Add pipelined lease renewal and the startup invariant.
4. Turn on K>1 in the documented deployment and verify capacity with the load
   harness. Rollback: set `LOGIC_LANES=1`.

## Open Questions

- Redis Streams + consumer groups vs pub/sub + in-process sharding (durability
  and true broker-side backpressure vs simplicity).
- Optimal lane count relative to cores and the create/join/move mix.
- Whether the outbound forwarder needs the same treatment (it is already
  per-room and was not the measured bottleneck).
