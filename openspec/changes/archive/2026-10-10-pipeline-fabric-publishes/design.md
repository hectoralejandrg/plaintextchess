# Design

## Context

See `proposal.md` - Why. The transport<->logic bridge is Redis pub/sub. The
logic tier's outbound shard tasks (from `optimize-fabric-message-path`) publish
one `OutboundBatch` (or single envelope) per `publish_outbound_batch` and await
its reply before taking the next item from the channel. On a multiplexed
connection each `PUBLISH` resolves only after the server replies, so the shard's
throughput is `1 / round-trip`.

A standalone spike (Redis 8.8, `redis-rs` 0.26, loopback, one subscriber, the
"update" payload) measured:

| client pattern | msg/s | us/publish |
|---|---|---|
| sequential (await each) | ~12,615 | 79.3 |
| pipelined k=16 | 251,706 | 4.0 |
| pipelined k=64 | 609,806 | 1.6 |
| pipelined k=1024 | 1,202,510 | 0.8 |
| concurrent t=16 | 92,750 | 10.8 |

The spike ran while the load harness was active (Redis ~21k ops/s), so absolute
numbers are a floor; the relative gap is the point. `redis-rs` #1312 documents
the same root cause: `PUBLISH` waits for a server reply, unlike a fire-and-forget
send.

## Gate result (tasks 1.1/1.2 — measured; 1.3 still open)

Task 1.1 required new instrumentation that did not exist before this gate: the
shard loop (`publish_batch`/`publish_units` in `logic_handler.rs`) now times
itself with `broker::record_publish_wait_ns`/`record_publish_process_ns`,
exposed as `fabric_metrics.outbound_publish_wait_ns` /
`outbound_publish_process_ns` on the logic tier's `/healthz` (summed across all
16 outbound shards on the node).

Measured on the current tree (HEAD, pipelined-publish path already live),
release, M2 Max, one logic + one transport, loopback,
`server/examples/load_gen.rs --games 3000 --pace-ms 60`:

```
fabric_publish_shard_time wait_us_total=10015973.4 process_us_total=82386.9 wait_vs_process_ratio=121.57
```

- Summed across every outbound shard over the whole run, the shards spent
  ~10.0 seconds of wall time awaiting Redis publish replies and ~82 ms
  building batches/payloads — the publish round trip is **~122x** the
  processing cost.
- **Verdict (task 1.2): GO** — the publish round trip is overwhelmingly the
  dominant cost in the shard loop, confirming the spike's premise on the real
  server path (not just the standalone benchmark in the table above).
- **Task 1.3 (measured)**: reproduced on the real server path —
  `Broker::publish_outbound_batch` (awaited, one reply per publish) vs
  `Broker::publish_outbound_pipelined` (k=64) carrying the real `OutboundBatch`
  envelope (~8.6 KB for a 33-connection mid-game move), against local Redis:
  `server/tests/fabric_publish_bench.rs` reports awaited **55.4 us/publish** vs
  pipelined **4.9 us/publish** (**~11x**). The smaller ratio than the toy-payload
  spike (which used ~100 B) is the larger real envelope; the direction and the
  elimination of the per-message round trip are confirmed on the real path.

## Goals / Non-Goals

**Goals:**
- Remove the per-message round trip from the fabric publish path by pipelining
  several publishes per connection, in order.
- Keep per-room/per-connection ordering and every client-visible message
  identical.
- Bound in-flight work and apply backpressure past the bound.
- Make the change measurable (msg/s, p95, crossing concurrency).

**Non-Goals:**
- Replacing Redis or adding another broker (a separate architectural change).
- Changing the client wire protocol, the message contents, or the batcher
  sharding from `optimize-fabric-message-path`.
- Persistence, replay, or delivery guarantees beyond at-most-once pub/sub.

## Decisions

### D1 - Measure first (gate)
Extend the load harness to report, per shard: publishes awaited vs pipelined,
messages/s, and move-ack p95. The change proceeds only if the publish round trip
is a material share of the shard's time (the spike says it is).

### D2 - Pipeline publishes, in order, with a bounded depth
Instead of awaiting each publish, the shard sends up to `PIPELINE_DEPTH`
publishes in a single round trip (a Redis pipeline / `pipe()` query) and awaits
once. Redis executes pipelined commands in the order sent, so a room's messages
keep their order; the batch reply also surfaces any per-command error. Depth is
capped to bound memory and in-flight work.

Alternative: fire-and-forget with no reply (fastest) - rejected because a
connection error must be observed to trigger the stale-route retry and to avoid
silently losing messages.

### D3 - Backpressure past the bound
When the pipeline/queue is at its cap, the shard stops accepting more and the
forwarder awaits (backpressure) rather than growing without bound or dropping,
matching the behavior `optimize-fabric-message-path` established.

### D4 - Keep the envelope and retry semantics
The single `OutboundEnvelope` and `OutboundBatch` shapes are unchanged. On a
`Ok(0)` (no subscriber) the existing stale-route re-resolution still runs, per
message as today.

### D5 - Behavior preserved
Client messages, per-connection order, state-class coalescing, and every close
code are unchanged. The FFI and mobile clients are untouched.

## Risks / Trade-offs

- [Ordering across a pipeline] -> Redis executes pipelined commands in order on
  one connection, and a connection's messages stay on one shard; a per-room
  order test guards it.
- [A pipelined command fails] -> the batch reply reports per-command errors;
  handle them like today's single-publish errors.
- [Latency vs throughput] -> pipelining trades a little added latency (waiting
  to fill the batch) for much higher throughput; cap the batch and re-measure
  p95.
- [Optimizing the wrong cost] -> D1 measures first; the spike must be confirmed
  on the real server path with the load harness.

## Open Questions

- The ideal `PIPELINE_DEPTH` and flush policy under the real load profile.
- Whether to also pipeline the inbound (transport -> logic) publishes.
- Whether the same round-trip cost argues for a different broker (NATS /
  brokerless) if pipelining is not enough.
