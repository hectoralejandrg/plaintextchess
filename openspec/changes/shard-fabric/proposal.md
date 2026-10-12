# Proposal

## Why

The transport<->logic bridge runs on a **single Redis pub/sub instance**, and
that shared bus is why the per-node ceiling does not scale horizontally: every
move crosses the same serialized fabric, so adding logic/transport nodes gives
only ~1.10x (2 nodes) / ~1.05x (3 nodes). `optimize-fabric-message-path` and
`pipeline-fabric-publishes` reduced the *cost* per message but left the single
fabric as the shared resource; sharding Redis was explicitly deferred as a
separate, larger change ("Non-Goals: ...sharding Redis... (separate)").

Sharding the fabric removes that single serialized path: room/connection traffic
is partitioned across several Redis instances, so distinct rooms stop competing
for one bus while each room keeps its order (it stays on one shard).

## What Changes

- **Connect to N Redis shards**: the broker opens one connection per shard, from
  `REDIS_URLS` (comma-separated), with `REDIS_URL` as a 1-shard fallback.
- **Route by key**: room/connection-scoped traffic is assigned to a shard by a
  hash of its key (room code / connection id), so **a room always maps to one
  shard** and per-room/per-connection order is preserved.
- **Keep a control shard**: low-volume lifecycle operations (node leases,
  registry, "list every logic node") stay on shard 0 to avoid cross-shard
  consistency problems.
- **Publish per shard**: every outbound publish path carries its shard, with
  connection/retry scoped to that shard; the outbound batcher spawns **one
  `BatchShard` per Redis shard**, each bound to its shard's connection.
- **No client or protocol change**: this is an internal fabric change.

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `realtime-transport`: the transport<->logic bridge MUST be able to spread its
  per-room traffic across multiple Redis shards (partitioned by room/connection
  key, order preserved per room), instead of a single shared bus, without
  changing any client-visible message.

## Impact

- **Server code**: `infrastructure/config.rs` (`REDIS_URLS`),
  `infrastructure/broker.rs` (per-shard connections, `n_shards`/`shard_for`,
  control shard, shard-aware publish/retry), `infrastructure/logic_handler.rs`
  (outbound batcher one shard-set per Redis shard), and the `run_*` wiring.
- **Clients (iOS/Android)**: no change.
- **Specs**: modified `realtime-transport`.
- **Non-goals**: room ownership/routing (a separate piece of horizontal
  scaling) and moving off Redis.
