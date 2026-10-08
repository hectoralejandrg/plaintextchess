# Proposal

## Why

The online socket path does more work than it needs to. Every accepted move broadcasts a **full state snapshot** (the entire `board_fen` plus the whole `move_list`) to both players, so a game of `n` moves sends `O(n²)` move bytes; the clients already replay moves into a mirror session, so most of that payload is redundant. The server also keeps **unbounded** outbound channels per connection (one slow client can grow memory without limit, and a slow write blocks reading on the same `select!`), wakes **every room every 200 ms** even when no clock is running, and never sends its own **keepalive pings**. The clients parse every frame on the **main thread**.

## What Changes

- **Incremental move updates.** A new server→client message carries the applied move (when there is one) plus the resulting side to move, status, clocks, and ratings. The full `State` snapshot is reserved for when a player connects, re-attaches, or needs a resync. Clients replay the move into their mirror session instead of rebuilding from `board_fen` on every move.
- **Backpressure.** Per-connection outbound buffering is bounded; intermediate snapshots are coalesced so the last state always wins, while one-shot frames (auth replies, structured errors) are never dropped. Read and write are separated so a slow write cannot stall reads.
- **Cheaper broadcasts.** The snapshot's shared fields are computed once per broadcast instead of once per seat.
- **Reactive clock timer.** The deadline timer is armed only while a clock is running, not on a fixed 200 ms tick for every room.
- **Keepalive.** The server pings idle connections so a dead peer is noticed before the reconnect-grace window elapses.
- **Client decode off the main thread.** Both clients decode incoming frames on a background queue and publish on the main thread.

## Capabilities

### New Capabilities
- None.

### Modified Capabilities
- `server`: "Server Game Authority" changes from "send a state snapshot after each move" to incremental per-move updates plus full snapshots on connect/re-attach; a new "Server Connection Resilience" requirement covers bounded outbound buffering and keepalive.
- `ios`: "iOS Online Multiplayer" applies incremental updates and uses the full snapshot only on connect/re-attach.
- `android`: "Android Online Multiplayer" applies incremental updates and uses the full snapshot only on connect/re-attach.

## Impact

- **Server**: `server/src/interface/protocol.rs` (new `Update` message), `server/src/application/room_actor.rs` (emit updates, shared snapshot, reactive timer), `server/src/infrastructure/ws.rs` (bounded outbound, split read/write, keepalive).
- **Clients**: `OnlineProtocol.swift`/`.kt` (decode the new message), `OnlineConnectionManager.swift`/`.kt` and `GameViewModel.swift`/`.kt` (apply updates; decode off the main thread).
- **Specs**: `server`, `ios`, `android`.
- No persistence, rating, or authentication changes; JSON text frames stay the transport (no binary switch).
