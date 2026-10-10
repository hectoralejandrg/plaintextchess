# Proposal

## Why

Profiling the logic tier under load shows it is **not CPU-bound**: no thread
reaches 100%, yet latency grows and throughput collapses past a point. The time
goes to **contention**:

- **~25% of the logic tier's active time** is spent waiting on a mutex because a
  tokio timer is armed on **every Redis publish** (redis-rs arms a response
  timeout per command; each one inserts/removes an entry in the shared
  timer-wheel under a lock): `publish_outbound -> redis publish -> send_recv ->
  tokio::time::sleep -> TimerEntry -> Handle::reregister -> Mutex::lock`.
- **~19% of the transport's active time** is waiting on a **global mutex** that
  is written on **every frame** (`set_conn_logic_node` updates the shared
  connection->logic map per message).
- A further ~22% of logic time is synchronous SQLite WAL `fsync` on the single
  writer thread - largely inflated by the benchmark's fast create/join/resign
  churn, so it is deferred (see Non-Goals).

Because the ceiling is contention rather than CPU, adding cores or sharding the
reader does not help; removing these two per-message shared resources is what
lets a node scale. This is measured, not assumed (see the profiling report).

## What Changes

- **Logic publish path**: stop arming a tokio timer per outbound publish so the
  publish path no longer contends on the shared timer wheel (configure the Redis
  connection so it does not arm a per-command timeout, or publish over a path
  that does not).
- **Transport affinity map**: stop taking a global lock to update a connection's
  logic-node affinity on every frame - shard the map or write only when the
  value changes.
- No change to the wire protocol, client behavior, routing semantics, or the
  fabric channels.

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `logic-throughput`: adds the requirement that the outbound publish path does
  not contend on a per-message shared timer/lock, so per-node throughput scales
  with worker threads instead of serializing.
- `transport-scale-out`: adds the requirement that updating a connection's
  owning logic node does not serialize on a global lock on the per-frame path.

## Impact

- **Server code**: `server/src/infrastructure/broker.rs` (publish path / Redis
  connection configuration), `server/src/infrastructure/transport_handler.rs`
  (affinity map update).
- **Deployment/ops**: same topology; a node sustains more games before adding
  another.
- **Clients**: none.
- **Specs**: modified `logic-throughput` and `transport-scale-out`.
