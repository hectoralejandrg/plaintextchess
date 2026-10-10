# Proposal

## Why

A release load test on an M2 Max (12 cores, 32 GB) measured the ceiling of a
single logic node at ~950 concurrent games (move-ack p95 = 100 ms), with move
throughput saturating at ~6.5–6.9k/s and logic CPU at only ~20%. Past ~750
games latency grows superlinearly while throughput stalls, and create/join
latency degrades in step (p95 29→186 ms) even though dispatching a move is
cheap. Redis is never the bottleneck (peak ~35k ops/s). The limit is the single
serial inbound lane in `logic_handler.rs` ("One task processes every envelope,
in arrival order"), which shares one queue between cheap move dispatch and
blocking control-plane work (argon2 auth, Redis create-room/claim). Phase 3
room routing multiplies nodes but each node keeps its single lane, so per-node
structure is the lever for capacity and cost.

Profiling after the sharded lanes were built **corrected that hypothesis**: with
the handler parallelized, the lanes sat idle (queue depth 0) and capacity did
not change (4 lanes == 1 lane), because the real per-node bottleneck was a
**fabric lookup on the transport's hot path** - a `logic_nodes()` Redis call made
for **every** frame, even a move on a known connection. Removing it from the hot
path (one publish instead of list + publish) raised the measured per-node
ceiling from ~950-1250 to ~2000-2700 concurrent games at p95 = 100 ms and from
~7.9k to ~23k moves/s. The remaining, larger levers (balanced room placement
across logic nodes, and per-message lock/timer contention) are out of scope here
and tracked in their own changes.

## What Changes

- **Sharded inbound processing**: keep one sequential broker reader that only
  routes, and fan envelopes out to a small number of concurrent lanes keyed by
  room code, so rooms assigned to different lanes are processed in parallel
  while messages for the same room stay in arrival order.
- **Control-plane isolation**: process create-room and auth on a separate lane
  set keyed by connection id, so an in-flight authentication or room creation
  never stalls move acknowledgements for games already in play. Room-scoped
  join/leave/resign stay on the room's lane so they keep the room's order.
- **Fabric hot path**: forward a frame whose target logic node is already known
  with a single publish, and consult the logic-node list only when that target
  does not accept (the fallback path), instead of listing nodes on every frame.
- **Lease-renewal hardening**: renew all owned-room ownership leases in a single
  pipelined round trip per sweep and assert at startup that the lease TTL safely
  exceeds the worst-case sweep, eliminating the self-heal re-claim storm
  observed under load (7104 spurious re-claims in one run).
- **Observability**: expose per-lane queue depth and processing latency, plus
  move-ack latency, so per-node capacity is measurable and regressions are
  caught rather than discovered in production.
- **Configuration**: the lane count is configurable with a safe default that
  preserves current behavior when unset.

No **BREAKING** change for clients: the wire protocol, endpoints, and mobile
apps are unchanged.

## Capabilities

### New Capabilities
- `logic-throughput`: how a single logic node processes inbound messages
  concurrently across independent rooms while preserving per-room order,
  isolating control-plane work from the move path, bounding memory with
  backpressure, and remaining observable.

### Modified Capabilities
- `room-routing`: the "Logic Node Membership and Leases" requirement gains a
  batched, pipeline-renewed ownership sweep and a startup invariant that the
  sweep keeps up with the lease TTL, so an active node never spuriously re-claims
  its own rooms under load.

## Impact

- **Server code**: `infrastructure/logic_handler.rs` (reader fan-out and lane
  workers; partition of the shared connection map), `infrastructure/broker.rs`
  (pipelined lease renewal), `infrastructure/transport_handler.rs` (fabric hot
  path: no per-frame node listing), and `config` (lane count with a safe default).
- **Deployment/ops**: topology unchanged; a logic node serves more games before
  another node is needed, and lease renewal costs one round trip per sweep
  instead of one per room.
- **Clients (iOS/Android)**: no protocol or UI changes.
- **Specs**: new `logic-throughput`; modified `room-routing`.
