# Design

## Context

The server crate (`server/`) is currently a flat module set inside one
crate: `app.rs` (room registry, player index, rating store wiring),
`config.rs` (env config), `protocol.rs` (wire types + `decode_incoming` +
the pinned `wire_form_is_stable` test), `rating.rs` (in-memory Glicko2
store), `room.rs` (the room actor: `RoomMsg`/`Seat`/`Room`, move
application, disconnect grace), `ws.rs` (WebSocket handler), `main.rs`.
Tests live in `tests/rooms.rs` and `tests/integration.rs` (44 tests) plus
inline unit tests; `examples/online_buddy.rs` is the E2E client. The wire
form is pinned by `wire_form_is_stable` and mirrored by hand-written Swift
and Kotlin codecs. See proposal.md for the motivation.

Constraints that shape this design:

- The wire form is a protocol contract: the change is additive only
  (`v:1` stays), so the pin test is extended, never rewritten.
- The room actor already serializes all room state mutations through a
  message channel; the clock must plug into that same serialization point.
- Both clients render a fixed full-width board (board layout stability
  specs); the clock UI must not resize the board.
- No production URL exists yet (DEBUG-only MVP); ratings and rooms are
  in-memory.

## Goals / Non-Goals

Goals:

- A layered, single-crate server: `domain` → `application` →
  `infrastructure` → `interface`, ports as traits, one-way dependencies,
  behavior-preserving first.
- Server-authoritative Fischer clock for the presets 15+10, 10+0, 5+0,
  3+2, 1+0: flag fall, in-flight move rule, reconnect with remaining
  time, snapshots carrying both clocks.
- Both clients: time-control choice at create, live two-clock UI,
  on-time end-of-game wordings with ratings.

Non-Goals:

- Clocks in local two-player or CPU modes (follow-up change).
- Köhler increments, custom base/increment, time-control negotiation in
  the lobby, turn timers that pause the opponent's clock.
- TLS / production URL (follow-up ops change), persistent ratings
  (follow-up change), chat/spectators/rematch.
- Any rework of the core FFI surface or of the 120 s disconnect-grace
  semantics.

## Decisions

### D1 — Layered module tree in one crate, behavior-preserving first

Target layout (single crate, module tree):

```
server/src/
  main.rs                  # binary entry: build infra wiring, run axum (infrastructure)
  lib.rs
  domain/                  # no tokio/axum/serde imports
    room.rs                # Room, Seat, RoomCode (alphabet + length constants), one-room-per-device rule
    time_control.rs        # TimeControl presets, "m+i" parse/format, validation
    clock.rs               # Fischer clock math: remaining ms, elapsed deduction, increment on completion, flag detection
    material.rs            # insufficient-material-to-mate check (flag-fall draws)
    rating.rs              # rating policy: initial 1500, result mapping (1.0 / 0.5 / 0.0)
  application/
    ports.rs               # trait ChessEngine, trait TimeSource
    room_actor.rs          # use cases: create/join/move/resign/leave/disconnect/reconnect/flag — orchestrates domain + ports
  infrastructure/
    server.rs              # (was app.rs) App state: room registry, player index, Conn, wiring
    ws.rs                  # (was ws.rs) WebSocket transport
    config.rs              # (was config.rs) PORT/BIND/RECONNECT_GRACE_SECS
    engine.rs              # ChessEngine impl around the chess-core session
  interface/
    protocol.rs            # (was protocol.rs) wire DTOs, decode_incoming, wire-form pin test
```

Dependency rule: `domain` depends on nothing (std only); `application`
depends on `domain` + its own ports; `infrastructure` implements the ports
and wires everything (tokio/axum/serde live here and in `interface`);
`interface` maps wire DTOs ↔ domain types and owns the wire pin test.
Phase 1 of implementation is a pure move+rename with the full suite green
before any clock code is added (the pin test and the 44 tests are the
canary).

Alternative considered: multi-crate split (`chess-server-domain` /
`-api` / `-infra`). Rejected for an MVP side project: more build and
release overhead without a second consumer of the domain layer; the
module tree already enforces the same boundaries with `use` discipline.

### D2 — Timestamp-based Fischer clock, no client authority

Each room stores, per side, `remaining_ms` (last settled value) and the
monotonic `turn_started_at` of the side to move. For display and deadline
checks the server computes
`remaining(now) = remaining_ms - (now - turn_started_at)` for the side to
move; the other side's remaining time is frozen while they wait.

On a move that is applied (turn correct, legal, game still playing):

1. `remaining_ms -= now - turn_started_at` (clamped to 0),
2. `remaining_ms += increment` (Fischer: the increment is added when the
   move is completed, so it is available on that player's *next* move),
3. switch side to move, `turn_started_at = now`.

A zero-increment control (10+0, 5+0, 1+0) can therefore reach 0:00 for
the next player, which is then an immediate flag fall on their deadline —
standard blind-bullet behavior.

The deadline for the side to move is `turn_started_at + remaining_ms`.
Flag detection runs in the room actor: a periodic check every ~200 ms
plus an exact check when a move is applied. Both run inside the same
serialization point as moves/resign/disconnect, so "move applied vs flag
fell" is decided by ordering in the actor — no separate lock, no race.
The monotonic clock (`tokio::time::Instant`) is behind the `TimeSource`
port so unit tests inject a fake time source and never sleep.

Alternatives considered: client-reported timestamps (rejected — the
server is the authority; a lying clock must not work); a single
`sleep_until(deadline)` timer per turn (rejected in favor of the 200 ms
tick + move-time check: the tick keeps a *disconnected* mover's clock
settling without a live timer, and the move-time check gives exact
in-flight semantics with no timer cancellation edge cases).

### D3 — In-flight move semantics

A move is honored if and only if the actor applies it before the flag is
settled: the move-time deadline check (D2) either accepts the move (and
the deadline is recomputed from the new position) or the flag has already
been recorded and the move is rejected with `game_over`. There is no
client-side grace period and no "last move at 0:00" fuzz; the server's
decision at apply time is final. This matches the spec scenario "A move
in flight is honored if applied before the flag."

### D4 — Flag-fall outcome, material check, ratings

- Wire: `Status::TimedOut { winner: Color }` serialized externally tagged
  as `"timed_out": { "winner": "black" }`, consistent with `checkmated` /
  `resigned` / `forfeited`.
- Insufficient material (standard flag-fall rule, deliberately
  simplified): if the side that would win by flag has **no pawns** and at
  most **one** non-king piece (or exactly two bishops of the same square
  color), the game ends `Drawn` instead. Rationale: K vs K, K+N vs K,
  K+B vs K, K+N+B vs K and same-color KB vs KB are the realistic cases;
  the simplified test covers all of them with negligible false-positive
  risk (e.g. K+R vs K obviously still wins).
- Ratings: `TimedOut` scores exactly like a win/loss (1.0 / 0.0, same
  path as forfeit); a flag-fall draw scores 0.5 / 0.5 like any draw.
- Room lifecycle: identical to forfeit — terminal rooms stay listed until
  both players disconnect, then are removed.

### D5 — Additive wire change (still `v:1`)

- `create_room` gains `time_control` (string, e.g. `"15+10"`). A
  `create_room` **without** the field (an old client build) is accepted
  and defaults to `15+10`, so the new server stays compatible with the
  currently shipped client builds during rollout.
- `State` gains `time_control` (string), `white_time_ms` and
  `black_time_ms` (integers, milliseconds, clamped ≥ 0).
- `Status` gains `timed_out` (D4).
- `wire_form_is_stable` is extended to assert the new fields and the new
  status variant; the Swift and Kotlin codecs mirror the fields
  (JSON decoders ignore unknown keys, so old clients keep working against
  the new server).

### D6 — Client clock UX

- Layout: a fixed-height clock row between the status area and the board —
  top clock = the side rendered on the top edge of the board, bottom
  clock = the side on the bottom edge (so flipping the board swaps the
  clock labels, keeping "my clock / opponent's clock" unambiguous).
  `m:ss` format; the active (side-to-move) clock is emphasized (accent
  color / bold); either clock turns red at ≤ 10 s remaining.
- Time source: snapshot `*_time_ms` is authoritative; a ~10 Hz local
  ticker only interpolates the side-to-move clock downward between
  snapshots, and every snapshot re-syncs both clocks (drift is bounded
  and self-correcting).
- Lobby: both clocks show the base time, inactive; the waiting state shows
  the chosen control next to the room code (`ABC123 · 15+10`).
- Reconnecting banner: clocks keep the last displayed values; re-attach
  re-syncs from the snapshot.
- The clock row is a reserved fixed slot, so the board keeps its exact
  full-width size in every online state (board layout stability specs);
  this is verified by the existing board-measurement E2E step.

### D7 — Time-control choice UI

The create sub-screen of the new-game sheet shows the five preset chips
(15+10, 10+0, 5+0, 3+2, 1+0), `15+10` pre-selected; the pick is sent in
`create_room`. The join flow is unchanged; the joiner sees the room's
control in the lobby and may simply not join — no negotiation (MVP).

### D8 — Flag fall while disconnected: deferred settlement

While a seat is disconnected its clock keeps counting (the actor keeps
settling both clocks). If the deadline fires while the player is
disconnected, the outcome is *deferred*: on re-attach (within the 120 s
grace) the game immediately ends by flag fall with the same winner/draw
as if the player had been connected; if the grace window expires first,
the existing forfeit rule applies. Either way the connected player wins
(or it is a draw on insufficient material). Rationale: a disconnect does
not resurrect time, but the game is not killed before the reconnect
window is exhausted — consistent with the existing disconnect/forfeit
spec.

### D9 — Testing strategy

- Unit (no real time): `TimeSource` fake drives `domain/clock.rs` tests —
  tick math, Fischer increment on completion, flag at exactly 0, clamping,
  zero-increment 0:00 handoff; `time_control` parse/format round-trips and
  rejection of non-presets; `material` insufficient-mate cases.
- Integration (real, short timeouts): 1+0 room where the mover stalls →
  both clients receive `timed_out` with the opponent as winner, further
  moves rejected `game_over`; flag with K vs K on the board → `Drawn`;
  disconnect + reconnect within grace → snapshot with reduced remaining
  time; deferred flag (1+0, disconnect before 60 s, reconnect after the
  deadline) → `timed_out` on re-attach; flag after grace expiry →
  `forfeited`.
- Wire: pin test extended (D5); buddy example client updated to parse the
  new fields and print clock values.
- Clients: codec round-trip tests extended per platform (Swift 12 → more,
  Kotlin 15 → more); E2E cross-platform runs per tasks §5.

## Risks / Trade-offs

- [200 ms tick + apply-time check gives ±200 ms flag precision] →
  acceptable for an MVP side project; the exact apply-time check means a
  move that lands before the next tick is never lost, which is the
  player-visible property that matters.
- [Old client vs new server (and vice versa) during rollout] → D5 keeps
  both directions working: missing `time_control` defaults to 15+10 on the
  server; unknown `State` fields are ignored by the decoders. Practical
  exposure is low (no production URL; local dev only) and the release
  order is server-first anyway.
- [Additive wire fields must not break the pin-driven codec mirror] → the
  pin test is extended in the same commit as the DTO change, and each
  client's codec tests pin the mirrored fields; the E2E cross-platform
  run is the final cross-check.
- [Insufficient-material simplification] → documented rule (D4); only the
  near-impossible K+same-color-2B+minor edge is ever wrong, and it would
  only ever convert a flag-fall *win* into the correct-looking win case
  rarely enough to matter for an MVP.
- [Large refactor + feature in one change] → phase 1 (move-only, suite
  green, smoke E2E with the buddy) lands before any clock behavior;
  the pin test and the 44 tests are the behavior-preservation canary.
- [Clock UI vs board stability] → fixed-height reserved slot (D6) plus
  the board-measurement E2E step with clocks visible on both platforms.

## Migration Plan

1. Server: no data to migrate (rooms/ratings are in-memory by design;
   restart = clean state, documented limitation). Build and restart the
   new server; it is the rollout first step (accepts old clients).
2. Clients: ship the new iOS/Android builds; they read the new fields and
   send `time_control`. No per-device migration.
3. Rollback: revert the change's commits and restart the server; because
   there is no persisted state, rollback has no data step.

## Open Questions

None. The production `wss://` URL and persistent rating storage remain
deliberate follow-up changes, out of scope for this one.
