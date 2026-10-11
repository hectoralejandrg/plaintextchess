# Proposal

## Why

The egress measurement (`reduce-realtime-egress-cost`) showed the per-node
ceiling is fixed by the **per-move socket write**, which is inherent to the split
architecture: a move produces one update for each of two connections, they sit on
different sockets (often different transport nodes), so the writes cannot be
coalesced, and the move still crosses logic -> transport over the fabric before
those writes happen. Frame coalescing (feed + one flush per batch) was measured
neutral because each connection already has a single frame per move.

The common case, though, is a room whose two seats are served by the **same**
transport node. Today the move still hops logic -> transport over Redis and is
then written by that node. If a room's connections were kept on one node, its
move -> update could be produced and enqueued **in-process** on that node,
removing the fabric hop and the split for the hot path.

Hypothesis: colocating a room's hot path (both seats on one transport + logic)
raises the per-node ceiling without changing the client protocol.

## What Changes

- **Establish room locality**: keep (or strongly bias) a room's two connections
  on the same transport node, so the logic node owning the room is also the node
  whose transport serves both connections.
- **Handle a colocated room in-process**: apply the move and enqueue the resulting
  updates directly to the two connections' outboxes on that node, skipping the
  fabric hop; keep the fabric only for cross-node/fallback cases.
- **Keep the client protocol, message contents, and per-connection order
  identical.**
- **Measure before/after** on the same host and harness.

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `realtime-transport`: the system SHOULD keep a room's connections on one
  transport node and SHOULD serve a colocated room's move/update path in-process,
  so a room's per-move cost does not include a cross-node fabric hop, without
  changing any client-visible message.

## Impact

- **Server code**: placement/affinity (transport accept + join routing), and a
  same-node fast path from the room actor to the connections' outboxes.
- **Clients (iOS/Android)**: no protocol, UI, or FFI change.
- **Specs**: modified `realtime-transport`.
- **Non-goals**: changing the client protocol; removing the fabric (it remains
  for cross-node rooms).
