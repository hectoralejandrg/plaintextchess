# Proposal

## Why

The split architecture bridges transport and logic over a Redis pub/sub fabric,
and every fabric message is a `PUBLISH` that the sender **awaits**. On a
multiplexed Redis connection a `PUBLISH` resolves only after the server replies
(`redis-rs` #1312), so the send cost is one full round trip per message. The
performance QA of `optimize-fabric-message-path` measured ~190 us per publish and
a per-node ceiling that does not scale horizontally; a local spike on the same
broker shows the round trip is the dominant cost and that pipelining removes it:

| client pattern (Redis 8.8, redis-rs 0.26) | msg/s | us/publish |
|---|---|---|
| sequential (await each PUBLISH; today) | ~12,615 | 79.3 |
| pipelined k=16 | 251,706 | 4.0 |
| pipelined k=64 | 609,806 | 1.6 |
| pipelined k=256 | 1,010,790 | 1.0 |
| pipelined k=1024 | 1,202,510 | 0.8 |
| concurrent (16 connections) | 92,750 | 10.8 |

Prefixing multiple `PUBLISH` commands on one connection and awaiting once
pipelining) is ~48-95x faster than awaiting each reply, while Redis executes the
pipelined commands **in order**, so per-room/per-connection ordering is
preserved. This is a publisher-side change; the broker stays Redis.

## What Changes

- **Pipeline fabric publishes**: keep multiple outbound `PUBLISH` commands in
  flight on the connection (bounded batch), send them in order, and await the
  batch reply instead of one reply per message on the hot path.
- **Preserve order and contents**: pipelined commands execute in order on the
  Redis connection, so per-room/per-connection order and the client-visible
  messages are unchanged. The single/multi (`OutboundBatch`) envelope shapes
  stay the same.
- **Bound the in-flight work**: cap the pipeline depth / buffered bytes and
  apply backpressure past the cap (no unbounded memory, no silent drops).
- **Measure before/after**: re-run the per-node load harness on the same host
  and report messages/s, move-ack p95, and the crossing concurrency.

This is independent of, and complementary to, `optimize-fabric-message-path`
(which reduced the *number* of messages). This change reduces the *cost per
message*.

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `realtime-transport`: the transport<->logic bridge MUST NOT pay a full
  round trip per fabric message; it MUST keep multiple publishes in flight
  (pipelined) in order, without changing any client-visible message and without
  dropping messages while the fabric is healthy.

## Impact

- **Server code**: `server/src/infrastructure/broker.rs` (a pipelined publish
  path) and `server/src/infrastructure/logic_handler.rs` (the shard publish
  loop awaits a batch instead of each message).
- **Clients (iOS/Android)**: no protocol, UI, or FFI change.
- **Specs**: modified `realtime-transport`.
- **Non-goals**: replacing Redis, adding a broker, or changing the batcher
  sharding from `optimize-fabric-message-path`.
