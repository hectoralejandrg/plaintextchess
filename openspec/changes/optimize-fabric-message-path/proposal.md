# Proposal

## Why

The measured per-node ceiling of the logic tier is ~2,650-2,700 concurrent games
at move-ack p95 = 100 ms (~23.9k moves/s) with one logic node and one transport.
It does **not** scale horizontally: two logic instances give ~1.10x and three
~1.05x, even though balanced placement distributes rooms ~50/50 across them. The
same measurements show the logic lanes never queue (depth 0, ~65-80 us), Redis
stays near 0.3 of a core, and both tiers spend most of their time parked in
`cond_wait`; the transport's dominant syscall is the outbound socket write. The
shared cost is the **per-message fabric path**: over loopback, each move is
roughly one inbound Redis publish, an outbound Redis publish **per seat** (two for
a game whose seats share a transport), and WebSocket writes on both sides. Adding
logic or transport nodes shares that same path, so more nodes cannot lift the
ceiling - fewer and cheaper messages per move can.

## What Changes

- **Coalesce outbound fan-out**: when several server messages for the same
  transport node are produced together (e.g. a move's `Update` to both seats on
  one node), deliver them to that node in **one** bounded fabric message instead
  of one per seat.
- **Drop redundant serialization on the hot path**: the transport already parses
  the client frame; avoid re-serializing it into a JSON envelope that the logic
  parses again, so a move crosses each boundary once.
- **Keep everything else identical**: the client wire protocol, per-connection
  message order, coalescing semantics, and close/keepalive behavior are unchanged.

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `realtime-transport`: adds that the transport<->logic bridge MUST coalesce
  outbound fan-out (no one fabric message per seat when a batch suffices) and
  MUST NOT re-serialize a frame it has already decoded, so per-node throughput is
  not bounded by the number of socket-level messages per client action - without
  changing any client-visible message.

## Impact

- **Server code**: `infrastructure/logic_handler.rs` (outbound forwarder batches
  per transport), `infrastructure/broker.rs` (a bounded batch envelope), and
  `infrastructure/transport_handler.rs` / `infrastructure/transport.rs` (dispatch a
  batch to its connections).
- **Clients (iOS/Android)**: no protocol, UI, or FFI change.
- **Specs**: modified `realtime-transport`.
