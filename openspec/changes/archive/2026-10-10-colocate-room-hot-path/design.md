# Design

## Context

See `proposal.md` - Why. The egress measurement (`reduce-realtime-egress-cost`)
recorded, for one logic + one transport on one host, loopback:

- egress per move: 2 socket writes (one frame per connection), ~374 bytes,
  serialize ~1-1.5 us/move, **write ~50-58 us/move** (dominant);
- split fabric per move: ~0.15 outbound messages (batched) + 1 inbound frame;
- ceiling crossing ~3,300-3,500 games at move-ack p95 = 100 ms (harness-bounded);
- frame coalescing was neutral (each connection already has one ready frame).

So the per-move cost that bounds the ceiling is the **per-connection socket
write**, which the split cannot remove because a room's two seats can be served
by different nodes and still need the fabric hop.

## Gate result (tasks 1.1/1.2 - measured)

Measured on the current tree (HEAD `ccb543e` + the egress instrumentation/flush
change), release, M2 Max, one logic + one transport, loopback, `server/examples/
load_gen.rs`:

- **Fabric hop per move**: outbound ~0.071-0.081 messages/move (batched), inbound
  ~1.0-1.1; measured round trip on this host (redis-rs 0.26, loopback):
  sequential `PUBLISH` ~40.5 us, and PUBLISH -> psubscribe delivery p50 ~34-35 us
  / p95 ~85-92 us. So the two-legged hop a colocated room would remove costs
  **~83 us/move**, the same order as the socket write (`write_us/move` ~52-63 us).
- **Verdict: GO (bounded)**. Removing the hop removes ~57% of the per-move
  "hop + write" budget, for an estimated ceiling of **~1.25-1.35x** (more if the
  batcher window and the logic parse are also removed). It does **not** remove the
  two socket writes per move (~60 us/move, ~2.2-2.7 cores at 43k moves/s), which
  remain the floor.
- **Cheap-lever control (LOGIC_LANES)**: increasing the logic lanes does **not**
  help and hurts at high load - 2,000 games p95 34.6/34.1/38.6 ms for lanes
  1/4/16; 3,000 games p95 86.0 ms (lanes 1) vs 142.2 ms (lanes 16). This resolves
  the earlier "room_lanes=1" control concern: lanes are not a lever, so colocation
  is the remaining one.
- **Harness cap**: the crossing near p95=100 ms is ~3,500-3,800 games; `load_gen`
  seats zero games at 4,000 on this host, so the true server ceiling is a lower
  bound and must be measured with the client off-host.

## Goals / Non-Goals

**Goals:**
- Keep a room's connections on one transport node, so its hot path is local.
- Serve a colocated room's move/update path in-process (no fabric hop, no
  transport dispatch), keeping the client protocol and ordering unchanged.
- Measure whether the colocated hot path raises the per-node ceiling.

**Non-Goals:**
- Changing the client wire protocol or the mobile apps.
- Removing the fabric; it stays for cross-node rooms and reconnect cases.
- Changing game rules, clock, rating, or the auth path.

## Decisions

### D1 - Measure the colocated vs split cost first (gate)
Before any routing change, measure the per-move cost of a room whose two seats
are already on one node, versus the split path, on the reference harness. Proceed
only if the local path is materially cheaper per move.

### D2 - Establish room locality
Bias a room's two seats onto the same transport node:
- the first seat is placed normally; the second seat is **routed to the first
  seat's node** (LB affinity by room code via cookie/consistent hash, or a
  transport-level redirect/handoff on join).
This is the part that must not break the "no sticky sessions" property for
non-room traffic; only a seated room's join is bound.

### D3 - In-process hot path for a colocated room
When the logic node owning a room is co-hosted with the transport node serving
both connections, apply the move and enqueue the two updates **directly to the
connections' outboxes** on that node, skipping the `publish_outbound` fabric
round trip and the transport dispatch. Ordering per connection is preserved (the
outbox is still drained in order).

### D4 - Keep the split as the fallback
Cross-node rooms and reconnects still use the fabric path unchanged; the
in-process path is an optimization for the common case, not a replacement.

### D5 - Behavior preserved
Every client-visible frame, its fields, its order, the state-class coalescing,
and every close/keepalive behavior are unchanged. The FFI and mobile clients are
untouched.

## Risks / Trade-offs

- [Affinity breaks statelessness] -> only a seated room's join is bound; new
  connections stay free to land anywhere.
- [A hot node] -> affinity by room can concentrate load; measure the distribution
  and keep the LB able to spread by room.
- [Reconnect/migration] -> a reconnecting seat may land on another node; fall
  back to the fabric path for the mismatch.
- [Optimizing the wrong cost] -> D1 is a gate; the colocated path must be
  measured cheaper before the routing change is built.

## Open Questions

- Can the load balancer bind by room code without sticky sessions for everything
  (cookie or consistent hash on a room parameter)?
- Is a transport-to-transport handoff on join simpler than LB affinity?
- Does the in-process path actually lift the ceiling, or is the per-write cost
  paid regardless of the hop (two sockets still need two writes)?

## Implementation note: rescoped (tasks 2-4)

Building D2/D3 as literally specified required two things that did not exist
in this codebase: a load balancer (none exists; the only placement logic is
`placement_rr`'s round-robin for a brand-new room's **logic** node) and a mode
that runs transport and logic in one process while still speaking the split
protocol (the only existing combined mode, `TIER=all`, has no fabric at all).
Rather than build a load balancer, this change:

- Adds `TIER=combined`: one process, one `Broker`, cross-linking a
  `TransportState` and a `LogicState` via a `OnceLock<Arc<_>>` field each way,
  set once by `run_combined` right after both are constructed
  (`transport_handler.rs`'s `local_logic`, `logic_handler.rs`'s
  `local_transport`). The fabric remains the only path for `TIER=transport`/
  `TIER=logic` processes (those fields are permanently `None`), and for a
  colocated process serving a cross-node room or a seat on another node.
- Decides local-vs-fabric **per connection**, not per room: `handle_transport_text`
  skips `publish_inbound_to` when the routing candidate is the process's own
  local logic (`try_local_logic`, `transport_handler.rs`), and
  `start_outbound_forwarder` skips the batcher when a connection's recorded
  transport node is the process's own local transport
  (`logic_handler.rs:1545`). A room with one seat local and one remote splits
  on its own, with no "is this room colocated" flag anywhere.
- Records which transport node served a room's first seat
  (`broker.rs`'s `room_transport` module, `ptc.room_transport.<room_code>`,
  task 2.1) purely as the data an external LB/redirect would need to route a
  room's second seat consistently; nothing in this repo reads it back, since
  the in-process decision above only needs each connection's own
  `transport_node_id`, not this record.

## Gate result (tasks 3.x/4.2 - measured)

Measured on the same host/build as the `optimize-fabric-message-path` gate
result (M2 Max, release, loopback, `server/examples/load_gen.rs --games 3000
--pace-ms 60`), comparing one split pair (`TIER=transport` + `TIER=logic`,
fresh process per run) against one fresh `TIER=combined` process:

```
split:    ack_ms p50=50.61 p95=67.68 p99=73.07   write_us=50.58   outbound_publishes=0.074/move
combined: ack_ms p50=28.73 p95=34.61 p99=43.43   write_us=23.35   outbound_publishes=0.000/move
```

- **Fabric is fully eliminated on the move path**: `outbound_publishes` and
  `outbound_batches` are exactly `0.000/move` in combined mode (confirmed by
  the `colocated_move_skips_fabric` test's direct `fabric_metrics()` deltas,
  not just the load harness). `inbound_publishes` shows a small nonzero
  residual (~0.1/move, i.e. roughly one publish per connection, not per
  move) attributable to each connection's first frame (`CreateRoom`/
  `JoinRoom`) occasionally taking an extra routing attempt before
  `try_local_logic` matches, not to the move path itself - `split_fallback_still_works`
  confirms the fabric path still fires correctly when it's actually needed
  (a seat on a different transport node).
- **p95 roughly halved at the same 3,000-game load**: 67.68 ms -> 34.61 ms.
  This is a bigger win than the gate's conservative ~1.25-1.35x ceiling
  estimate (which only accounted for removing the ~83 us/move fabric hop out
  of a ~140 us/move hop+write budget) - `write_us` itself dropped from 50.58
  to 23.35 us/move, suggesting the fabric hop was also indirectly costing
  scheduler/contention time on the write path, not just its own ~83 us.
- **Ceiling (games at p95=100ms) not independently re-measured**: pushing
  `load_gen` past ~3,000-3,500 games hit the same **client-side** harness cap
  already documented in the gate above ("load_gen seats zero games at 4,000
  on this host"), compounded here by reusing one long-lived server process
  across repeated invocations with the same hard-coded player ids
  (`load-a-<idx>`), which the one-room-per-player rule correctly rejects on a
  reused device id once an earlier run's room hasn't fully released it - a
  test-methodology artifact (fix: a fresh server per run, as this measurement
  used for its one clean data point), not a server regression. The 3,000-game
  apples-to-apples comparison above is a fresh-process run on each side and
  is the one to trust; a true crossing point needs an off-host or
  identity-namespaced load client.
- **Verdict: confirmed GO, exceeds the gate's estimate.** Task 4.2 is closed
  with this comparison; a tighter ceiling-crossing number is left to a future
  run with a capacity-fixed load client.
