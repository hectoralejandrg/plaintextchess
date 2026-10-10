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

### D3 - Batch at the logic forwarder, with a bounded size and a quick flush
The logic's outbound forwarder groups messages for the same transport node within
a short window (or until a small count/byte cap) and publishes one batch. A batch
of one is published immediately, so a lone message pays nothing; the window is
small enough not to add a perceptible move-ack delay. Alternative: no batching,
keep per-seat publishes - rejected because the fan-out is the measured cost.

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

## Risks / Trade-offs

- [Batching adds latency] -> flush a batch immediately when it has one item, cap
  the window tightly, and never hold a message across a turn; measure p95.
- [Ordering across a batch] -> items keep their per-connection order when fanned
  out, and the window is small; a per-connection order test guards it.
- [Envelope change is internal but broad] -> the batch envelope is versioned like
  the existing envelopes; decode both forms during rollout.
- [Optimizing the wrong cost] -> D1 measures first and the change is gated on the
  measured share.

## Open Questions

- How much of the ceiling comes from the outbound fan-out vs the inbound/WS writes;
  D1 answers it before the batch work is committed to.
- Whether Redis pipelining of several outbound messages (instead of a batch
  envelope) is simpler and equally effective.
- Whether WebSocket write batching (fewer `send` calls) matters once each move is
  already down to one or two frames per client.
