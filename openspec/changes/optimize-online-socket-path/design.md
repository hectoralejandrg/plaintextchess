# Design

## Context

See proposal.md - Why. Today `Room::send_state_both` builds a full `State` (including `board_fen` and the whole `move_list`) and sends it to both seats after every accepted action; `handle_socket` forwards it over an unbounded channel and awaits the socket write in the same `select!` that reads; `run_room` ticks every 200 ms regardless; and both clients decode frames on the main thread. See `openspec/specs/server`, `ios`, `android`.

## Goals / Non-Goals

**Goals:**
- Cut per-move payload and CPU (incremental updates; shared snapshot fields computed once).
- Bound per-connection memory and stop a slow write from stalling reads.
- Wake rooms only when a clock is running; detect dead peers with keepalive.
- Move client JSON decode off the main thread.

**Non-Goals:**
- Switching JSON text frames to a binary encoding.
- Changing authentication, persistence, ratings, or the room/color rules.
- Reworking reconnect semantics (the grace window and re-attach rules stay).

## Decisions

- **D1 — New `Update` message, keep `State` full.** Add a `ServerMessage::Update { v, uci: Option<String>, side_to_move, status, white_rating, black_rating, opponent_online, white_time_ms, black_time_ms }`. It carries only the fields that change, plus the applied move. `State` keeps its meaning (a snapshot that rebuilds the screen) and is sent on connect/re-attach/resync. Alternative: reuse `State` with optional fields — rejected because it muddies "a snapshot rebuilds the screen" and bloats every update.
- **D2 — Clients replay the update.** On `Update` the client plays `uci` (when present) into its mirror session, appends it to the move list, and updates side-to-move, status, clocks, and ratings; `board_fen` is never used per move. `room_ready`/`state` still rebuild everything. This is what makes the payload cut safe.
- **D2b — Incremental seam for tests.** `Config.incremental_updates` (true from `from_env`, false in `Config::for_test`, like `random_colors`) lets the server fall back to full `state` broadcasts, so the existing socket tests (which read one snapshot per broadcast) stay stable; dedicated tests turn it on to exercise the `Update` path and assert connect/re-attach still send `state`.
- **D3 — Bounded outbound with coalescing.** Each connection holds a small outbox with at most one pending *state-class* frame; a newer state frame replaces the pending one, so the client always converges to the newest authoritative state. One-shot control frames (auth replies, structured errors) are never coalesced and preserve their order relative to neighbors. If a client is so slow that even the bounded control queue would overflow, the server closes the connection (which the existing disconnect/grace path already handles). Alternative: unbounded (rejected: memory) or drop-oldest blindly (rejected: could drop a one-shot error).
- **D4 — Split read and write.** The connection task spawns a writer that owns the socket's send half and drains the outbox; the reader only feeds inbound frames to the actor. A slow write therefore cannot block reads. Alternative: keep one `select!` and accept the stall — rejected, it is the root of the head-of-line blocking.
- **D5 — Compute the snapshot once per broadcast.** `snapshot()` results are built from shared fields (board FEN, move list, status, clocks, ratings) computed once, then cloned per seat with only `your_color`/`opponent_online` differing. Same for the terminal/`room_ready` paths.
- **D6 — Reactive clock timer.** Replace the fixed 200 ms interval with a timer armed only while a clock is running (`sleep_until(deadline)`), recomputed on moves and cancelled on a terminal result or when the room has no time control.
- **D7 — Keepalive.** The connection sends a WebSocket Ping on an interval and treats a missing Pong (or a write failure) as a drop, feeding the existing `Detach` path. Interval is a tunable constant well under the reconnect grace.
- **D8 — Client decode off the main thread.** iOS: `URLSessionWebSocketTask.receive` completion decodes on a background queue and hops to main to publish; Android: OkHttp `onMessage` decodes on its own executor and hops to main (`onMain`) only for state publication.

## Risks / Trade-offs

- [Coalescing could drop an intermediate update a client needed] → each state/update is self-contained after D2 replay (updates are ordered and each carries its own move; the client converges from the newest), and connect/re-attach always sends a full `State`. Verify with a slow-client test.
- [New message could break an old client] → the version stays 1; old clients that never expect `Update` would ignore unknown types only if they decode leniently. Mitigation: add `Update` to the pinned wire-form test and to both codecs **before** the server emits it; the clients are released together with the server in this repo.
- [Closing an overloaded slow client changes UX] → it becomes an ordinary disconnect/reconnect, which the client already handles; the grace window is unchanged.
- [Splitting read/write complicates the connection task] → keep the actor mailbox as the single point of game-state mutation; the split is confined to `ws.rs`.

## Migration Plan

1. Add `Update` to the protocol and both client codecs (decode-only on clients first), extend the pinned `wire_form_is_stable` test.
2. Server emits `Update` for moves/terminal; keeps `State` for connect/re-attach. Clients apply updates (D2).
3. Add backpressure (D3/D4), shared snapshot (D5), reactive timer (D6), keepalive (D7), client off-main decode (D8), each with its own test.
4. Rollback: the server can fall back to always sending `State` (clients still accept it), so the protocol addition is additive and reversible.

## Open Questions

- None that change the specs, the approach, or the task breakdown.
