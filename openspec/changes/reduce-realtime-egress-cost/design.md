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
egress, not the fabric - but it is not yet measured.

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

### D2 - Coalesce a connection's ready frames into one write
When several frames are ready for a connection, write them in one scatter-gather
`writev` (or one buffered write) instead of one `send` per frame, cutting
socket-write syscalls and copies. The frames themselves, their order, and the
protocol are unchanged; only the number of write syscalls per move drops.

### D3 - Fallback: colocate the room hot path in-process
If the measurement shows the egress is not the binding cost, the alternative is
to place a room's two seats on the same transport+logic so the hot path (a move
and its update) never crosses the fabric or the egress split. This is a larger
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
