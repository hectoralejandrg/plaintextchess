# Proposal

## Why

The per-node ceiling (~3.9k games at move-ack p95 = 100 ms, one logic + one
transport on one host) does not scale, and it was **not** raised by reducing the
fabric traffic. An A/B load QA measured that sharding the outbound batcher cut
the outbound fabric from ~2.0 to ~0.15 messages per move (~13x) while the ceiling
stayed flat (A ~3.9k vs B ~3.6k games; both within run noise). At the crossing
the logic lanes never queue, Redis sits near 0.3 of a core, and both tiers spend
most of their time parked in `cond_wait`; the transport's dominant syscall is the
**outbound WebSocket write**. Together this says the binding cost per move is on
the transport's **egress path**, not on the fabric.

There is no committed measurement of that path (writes/s, bytes, serialization
time, time in the write loop, per connection and per move), so the first step is
to measure it and find the dominant component before changing anything - the same
measure-first discipline the fabric and core changes used.

## What Changes

- **Measure the per-move transport egress cost** on the reference hardware: socket
  writes/s and bytes/s, the time spent serializing and writing frames, and where
  the write loop blocks. Record the baseline.
- **Reduce the per-move egress cost without changing the client protocol**: e.g.
  coalesce a connection's ready frames into a single scatter-gather write
  (`writev`) instead of one `send` per frame, cutting socket-write syscalls and
  copies while keeping per-connection order and every client-visible frame
  identical.
- **Gate**: only proceed if the measurement shows the write path is a material
  share of the per-move cost. If it is not, fall back to the documented
  alternative - **colocating a room's hot path in-process** so the common case
  (both seats on one node) never crosses the fabric or the egress split - decided
  by the measurement, not by guesswork.

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `realtime-transport`: the transport tier MUST bound its per-move egress cost
  (socket writes and bytes per move) and MUST keep the client-visible frames and
  their per-connection order unchanged.

## Impact

- **Server code**: `server/src/infrastructure/transport.rs` (the write loop /
  frame egress) and `server/src/infrastructure/transport_handler.rs` (the outbound
  dispatcher).
- **Clients (iOS/Android)**: no protocol, UI, or FFI change.
- **Specs**: modified `realtime-transport`.
- **Non-goals**: changing the fabric, the batcher, or the client protocol; the
  previous changes already removed the fabric as the binding cost.
