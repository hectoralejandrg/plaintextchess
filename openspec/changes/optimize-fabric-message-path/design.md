# Design

## Context

See `proposal.md` - Why. The measured ceiling is set by the per-message fabric
path, not by CPU: logic lanes never queue, Redis is near 0.3 of a core, and both
tiers park in `cond_wait`. Today, per move:

- transport -> logic: one `publish_inbound_to` (a JSON `InboundEnvelope` whose
  payload is the decoded `ClientMessage`, re-serialized after the transport
  already parsed the client frame);
- logic -> transport: one `publish_outbound` **per seat** (`deliver_outbound` is
  called once per connection), so two seats on the same transport node cost two
  fabric messages to the same `ptc.ws.out.<node>` channel;
- transport -> clients: one WebSocket text frame per connection (the per-connection
  `Outbox` already coalesces state-class frames to the newest).

Adding logic or transport nodes shares this path, so the fix is fewer/cheaper
messages per move.

## Measurement instrumentation (task 1.1)

The counters needed to measure the per-move budget are built into the code and
exposed for the load run (there is no committed load harness; the load run is
external):

- `broker.rs`: `INBOUND_PUBLISHES`, `OUTBOUND_PUBLISHES`,
  `OUTBOUND_BATCHES_PUBLISHED`, `INBOUND_BYTES`, `OUTBOUND_BYTES`, read together
  via `fabric_metrics()` and reset with `reset_fabric_metrics()`. Incremented in
  `publish_inbound`/`publish_inbound_to`, `publish_outbound`, and
  `publish_outbound_batch`.
- `transport.rs`: `WS_FRAMES_WRITTEN`, `WS_BYTES_WRITTEN`, read via
  `transport_metrics()` and reset with `reset_transport_metrics()`. Counted in
  `write_loop` (per frame) and `send_json` (per text frame's bytes).
- `logic_handler.rs` `/healthz` exposes `fabric_metrics` and per-lane
  `move_dispatch`; `transport_handler.rs` `/healthz` exposes
  `transport_metrics`. A dropped outbound message (only possible if a shard's task
  has gone) is counted as `fabric_metrics.outbound_drops`.

The numeric baseline (task 1.2: inbound/outbound publishes, WS frames and bytes,
socket syscalls per move) and the before/after comparison (task 5.2) are read
from these counters during a representative load run; they are recorded here once
that run is executed.

## Goals / Non-Goals

**Goals:**
- Cut the number of fabric messages and socket operations per move (start with
  the outbound-per-seat fan-out).
- Remove a redundant JSON encode/decode across the bridge.
- Keep the client wire protocol, message contents, ordering, and close/keepalive
  semantics identical.
- Make the change measurable (message counts / syscalls / capacity).

**Non-Goals:**
- Changing any client-visible message or the mobile apps.
- Replacing Redis pub/sub, sharding Redis, or moving off loopback (separate).
- Changing the per-connection coalescing semantics of the `Outbox`.

## Decisions

### D1 - Measure the per-move budget before optimizing
Instrument a representative load to count, per move: inbound publishes, outbound
publishes, WebSocket frames, and bytes, plus the transport's socket syscalls. The
outbound-per-seat fan-out is the first target; the measurement confirms its share
and guards against optimizing something else.

### D2 - A bounded outbound batch envelope
Add an internal outbound envelope variant that carries several per-connection
messages for one transport node (`Batch { node_id, items: Vec<OutboundEnvelope> }`).
The transport dispatcher fans a batch out to each connection's `Outbox` as if the
items had arrived individually (so per-connection order and coalescing are
unchanged). This is an internal fabric change; the client never sees it.

### D3 - Batch at the logic forwarder, sharded per node, with backpressure
The logic's outbound forwarder groups messages for the same transport node within
a short window (or until a small **item** cap of 32) and publishes one batch. A
batch of one is published immediately, so a lone message pays nothing; the window
is small enough not to add a perceptible move-ack delay. There is no byte cap: it
could not be enforced without re-serializing every item to measure it (the exact
cost D4 removes), and the item cap already bounds the batch.

Publishes are **sharded per node** (`BATCH_SHARDS_PER_NODE`, keyed by room code):
a game's two seats share a shard and coalesce into one message, while distinct
rooms publish in parallel, so one node's outbound is not serialized through a
single task. Each shard's channel is bounded (1024) and `submit` **awaits** when
full, applying backpressure instead of dropping: under sustained load latency
rises (as it did before this change) but no Update is lost. A per-connection
ordering guarantee holds because a seated connection stays in one shard for the
lifetime of its forwarder.

Alternative: no batching, keep per-seat publishes - rejected because the fan-out
is the measured cost. Alternative: a single per-node task - rejected by the
performance QA: it serialized publishes and its `try_send` dropped Updates,
halving the per-node ceiling. Alternative: shard by connection - rejected because
a game's two seats would land in different shards and never coalesce.

### D4 - Forward the decoded frame without re-encoding
The transport already decodes the client frame to validate and route it. Instead
of re-serializing the typed message inside the envelope, forward the original
frame payload so the logic decodes once, removing one JSON encode per inbound
frame. The envelope stays JSON on the wire (cross-process); only the redundant
encode is removed.

### D5 - Behavior preserved
Client messages, per-connection order (the `Outbox` is drained in order per
connection), state-class coalescing, and every close code are unchanged. The FFI
and mobile clients are untouched.

## Rollout order (task 5.1)

The v1<->v2 fabric compatibility is **asymmetric**, so the deploy order matters:

- The new logic accepts every v1 inbound form (`{"client": {...}}` and the
  `"disconnect"` string) and the new v2 forms (`Client`, `ClientFrame`).
- An **old** logic rejects the new transport's `ClientFrame` (its v1 decoder
  knows only `client`/`disconnect`), so a transport upgraded first would have all
  client frames dropped.
- An **old** transport decodes only a single `OutboundEnvelope`, so it **drops a
  batch**; a logic upgraded first (with batching on) would lose multi-seat
  fan-outs during the window.

Therefore neither raw order (`v` is currently stamped but never validated, so it
cannot gate) is safe on its own. The safe rollout is:

1. Deploy **logic** first with batching disabled: it accepts v1 inbound, still
   publishes single per-seat envelopes, and loses nothing.
2. Deploy **transport**: it starts sending `ClientFrame` (accepted by step 1's
   logic) and decodes both single envelopes and batches.
3. **Enable batching** only once every transport node is on the new build, so no
   batch is ever sent to a v1 transport.

Batching is enabled today, so a real deploy must gate it (a config flag or by
requiring all transports to report v2) before step 3. This section is the
documented requirement until that flag exists.

## Risks / Trade-offs

- [Batching adds latency] -> flush a batch immediately when it has one item, cap
  the window tightly, and never hold a message across a turn; measure p95.
- [Ordering across a batch] -> items keep their per-connection order when fanned
  out, and the window is small; a per-connection order test guards it.
- [Envelope change is internal but broad] -> `ENVELOPE_VERSION` is bumped to 2,
  and the inbound payload decodes both wire forms during rollout: the v1
  externally-tagged form (`{"client": {...}}`, decoded as `LegacyClient`) and the
  v2 form (`Client(ClientMessage)` / `ClientFrame { data }`). The outbound wire is
  `untagged`, so a batch decodes to a batch and a single envelope stays a single
  envelope.
- [Optimizing the wrong cost] -> D1 measures first and the change is gated on the
  measured share.
- [Batcher saturation can drop] -> the per-node outbound path is sharded
  (`BATCH_SHARDS_PER_NODE`) and each shard applies backpressure (`submit` awaits
  when its bounded channel is full), so a saturated shard slows the affected
  rooms instead of dropping their Updates. The old single per-node task with a
  dropping `try_send` halved the per-node ceiling in the performance QA; the
  redesign removes that failure mode. `fabric_metrics.outbound_drops` counts only
  the rare case of a shard whose task has gone (no receiver).

## Open Questions

- How much of the ceiling comes from the outbound fan-out vs the inbound/WS writes;
  D1 answers it before the batch work is committed to.
- Whether Redis pipelining of several outbound messages (instead of a batch
  envelope) is simpler and equally effective.
- Whether WebSocket write batching (fewer `send` calls) matters once each move is
  already down to one or two frames per client.
