# Design

## Context

See `proposal.md` - Why. Two prior changes moved the fabric cost and did not move
the ceiling:

- `optimize-fabric-message-path` sharded the outbound batcher and cut outbound
  fabric traffic ~13x with no message loss.
- `pipeline-fabric-publishes` removed the per-publish round trip on the hot path.

The load QA (Apple M2 Max, one logic + one transport on one host, games of 20
plies, one move ~60 ms, self-paced, median of 4 reps) measured:

| games | A p95 (ms) | B p95 (ms) | A moves/s | B moves/s | B outbound fabric/move |
|---|---|---|---|---|---|
| 1000 | 45.6 | 18.8 | 15,630 | 16,084 | ~0.20 |
| 2000 | 86.5 | 35.9 | 30,071 | 31,683 | ~0.17 |
| 3000 | 85.8 | 84.3 | 45,037 | 39,752 | ~0.15 |
| 4000 | 101.6 | 109.4 | 45,600 | 41,141 | ~0.14 |
| 6000 | 193.5 | 210.2 | 38,526 | 37,428 | ~0.13 |

A = HEAD `d32cbce` (fabric-publish per seat); B = sharded batcher + backpressure.
The p95=100 ms crossing is A ~3,900 vs B ~3,630 games - flat. Yet B's fabric
traffic is ~13x lower, and the logic lanes never queue and Redis is near 0.3 of a
core; the transport's dominant syscall is the outbound WebSocket write (each move
still writes one frame per seat). The endpoint is therefore the transport's
egress, not the fabric.

## Measured baseline (task 1.1)

The transport write path is now instrumented
(`ws_socket_writes`, `ws_serialize_ns`, `ws_write_ns`, exposed at the transport
`/healthz`), and `server/examples/load_gen.rs` drives the split tiers
out-of-process (one logic + one transport, loopback, Apple M2 Max, 20-ply games,
self-paced ~60 ms/move). Measured on the current tree:

| games | ok | moves/s | p95 (ms) | writes/move | bytes/move | serialize us/move | write us/move |
|---|---|---|---|---|---|---|---|
| 1000 | 1000 | 15,975 | 18.9 | 2.0 | 374 | 1.4 | 51.9 |
| 2000 | 2000 | 31,589 | 34.8 | 2.0 | 374 | 1.1 | 59.6 |
| 3000 | 3000 | 42,516 | 78.2 | 2.0 | 374 | 1.0 | 62.1 |
| 4000 | 169 | - | - | 2.0 | 374 | - | - |

Reading (task 1.2): the **socket write time dominates** the egress
(~52-62 us per move, summed across connections) while serialization is
negligible (~1-1.4 us/move), and the write *count* is already minimal at
**2 writes per move** (one frame per connection; the two seats are on different
sockets, so they cannot be coalesced together). The write path is material: at
3,000 games, ~42.5k moves/s x ~60 us ~= 2.5 cores spent writing.

Reading (task 1.3): the harness on the same host **caps the measurable ceiling**
- at 4,000 games it could only seat 169 games before setup failed, so the
crossing near p95=100 ms (~3,300-3,500 games here) is bounded by the load
generator, not measured as a clean server ceiling. Treat the absolute ceiling as
a lower bound.

### After the flush coalescing (task 4.2)

Re-ran the same harness on the same host after D2 (feed + one flush per drained
batch). The result is **neutral in the steady workload**:

| games | p95 before (ms) | p95 after (ms) | frames/move | flushes/move |
|---|---|---|---|---|
| 1000 | 18.9 | 17.3 | 2.0 | 2.0 |
| 3000 | 78.2 | 77.5 | 2.0 | 2.0 |

No regression, but no flush reduction either: each connection has exactly one
ready frame per move (`Update` is state-class and coalesces to the newest), so a
drained batch is a single frame and one flush is paid regardless. **The egress
cost is inherent to one frame per connection per move**; frame coalescing does
not reduce it. This is why group 3's fallback (colocate the room hot path) is the
remaining lever if a higher ceiling is required.

## Goals / Non-Goals

**Goals:**
- Measure the per-move egress cost (syscalls, bytes, serialization and write
  time, where the write loop blocks) and locate the dominant component.
- Reduce the per-move egress cost while keeping the client-visible frames, their
  contents, and their per-connection order identical.
- Make the improvement measurable (writes/s, bytes/move, p95, crossing games).

**Non-Goals:**
- Changing the client wire protocol or the mobile apps.
- Changing the fabric, the batcher, or the sharding (already addressed).
- Changing game rules, state, or the auth path.

## Decisions

### D1 - Measure the egress path first (gate)
Instrument the transport's write path to report, per move and per connection:
socket writes, bytes, serialization time, and blocked time. The later decisions
proceed only for the component the measurement shows is dominant; if the write
path is not material, stop and revisit (D3).

### D2 - Coalesce a connection's ready frames into one flush
The measurement (Measured baseline) shows the write *count* is already minimal
(one frame per connection per move; `Update` is state-class and already coalesces
to the newest), so cutting writes per move is not available. The remaining
per-move egress cost is the **flush** each frame pays (~25-30 us per write). A
connection's ready frames are therefore written with one flush per drained batch
(feed all frames, then a single `flush`) instead of one flush per frame, cutting
flushes and copies when several frames are ready together (bursts: rapid games,
re-attach snapshot + updates) while the steady one-frame-per-move case is
unchanged. The frames themselves, their order, and the protocol are unchanged.

### D3 - Fallback: colocate the room hot path in-process
If the flush coalescing does not lift the ceiling (the residual is the per-write
cost of messages that inherently go to different sockets), the alternative is to
place a room's two seats on the same transport+logic so the hot path (a move and
its update) never crosses the fabric or the egress split. This is a larger
routing change (sticky per-room placement) and is chosen only on the measured
evidence.

### D4 - Behavior preserved
Every client-visible frame, its fields, its order per connection, the coalescing
of state-class frames, and every close/keepalive behavior are unchanged. The
mobile clients and the FFI are untouched.

## Risks / Trade-offs

- [Measuring the wrong cost] -> D1 is a gate; nothing changes until the egress
  measurement shows a material share.
- [Write coalescing delays a frame] -> only coalesce frames already ready; never
  hold a frame across a turn; re-measure p95.
- [The client harness co-limits the ceiling] -> the measured ceiling is partly the
  load generator on the same host; state it and compare A vs B with the same
  harness and host.
- [Colocation changes routing] -> only adopted if D1 rules out the egress; it is
  a separate, larger change.

## Open Questions

- Which dominates: syscall count, byte copy, JSON serialization, or the per-frame
  framing in the WebSocket library?
- Does the client harness on the same host cap the measured ceiling (so the true
  server ceiling is higher)?
- Whether room-affinity routing (D3) is worth its complexity even if the egress
  fix helps.

## Outcome

- **Delivered**: the egress instrumentation (transport `/healthz`), a committed
  load generator (`server/examples/load_gen.rs`), the measured baseline, and the
  flush-coalescing change (behavior-preserving, p95-neutral).
- **Not delivered here**: a ceiling increase. The measurement showed the cost is
  the inherent per-connection socket write, which frame coalescing cannot reduce.
- **Follow-on**: the colocation lever (D3) is split into the new change
  `colocate-room-hot-path`, whose first task is to measure the colocated versus
  split per-move cost before building it.
