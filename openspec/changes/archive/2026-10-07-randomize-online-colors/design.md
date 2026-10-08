# Design

## Context

See proposal.md - Why. Today `RoomActor::handle_connect` seats the creator in `WHITE` and the second player in `BLACK`, and `started()` is defined as "the `BLACK` seat is occupied". Every state snapshot already carries the receiving seat's color in `your_color`, and both clients render and orient from it, so the protocol does not need to change. See `openspec/specs/server`, `ios`, `android`.

## Goals / Non-Goals

**Goals:**
- Assign the creator a color at random at room creation, announced in its ready snapshot; the joiner takes the other color.
- Keep White moving first and keep all re-attach / one-room-per-player behavior unchanged.
- Keep the existing deterministic test suite stable while making the random path testable.

**Non-Goals:**
- No wire-protocol, persistence, or rating changes.
- No client-side protocol change (clients already use `your_color`).
- No "rematch with swapped colors" flow.

## Decisions

- **D1 — Randomize the creator's seat at creation, not at join.** The creator is placed in seat 0 or 1 (White/Black) by a coin flip when the room is created, so its very first `room_ready` already reports the final color; the joiner takes the empty seat. Alternative: keep the creator White in the lobby and swap at join time — rejected because the creator would see one color then another (flicker) and a swap would move already-tracked identities.
- **D2 — Redefine `started()` as "both seats occupied".** The current `seats[BLACK].is_some()` assumed the creator held White. The joiner branch also assumed it, and the hijack guard keyed off `seats[BLACK]`. Generalize all three to "find the empty seat / the seat this device names", independent of color.
- **D3 — Injectable color choice.** `RoomServices` gains a small seam (a `ColorAssigner` or a `random_colors: bool` plus an injectable RNG) so the production path flips a coin while tests can keep a fixed creator color and stay deterministic; a dedicated test turns randomness on and asserts both colors occur. Alternative: make every integration test color-agnostic — rejected as a large, noisy rewrite that obscures the actual behavior under test.
- **D4 — Clients stay as they are.** They read `your_color` from the snapshot and (already) orient the board to that color; the join/create screens only need to surface the assigned color. The DEBUG E2E scripts that drive fixed moves are updated to read the assigned color instead of assuming creator = White.

## Risks / Trade-offs

- [Randomness makes integration tests flaky] → D3 injectable seam: deterministic in the existing suite, one targeted randomness test.
- [Re-attach assumed fixed seats] → `seat_index_of` matches by account/device, never by color; verify re-attach still lands on the same seat for either random assignment.
- [Client board orientation] → already driven by `your_color`; verify a Black creator and a Black joiner both render with Black at the bottom.
- [DEBUG online scripts break] → update them to branch on the reported color.

## Migration Plan

1. Generalize the seat logic and add the injectable color choice; keep the default random in production, deterministic in tests.
2. Update the room/actor tests that assumed creator = White and add the randomness test.
3. Surface the assigned color on the client create screen; adjust the DEBUG scripts.
4. Rollback: restore the fixed creator = White / joiner = Black assignment; no persisted-state migration is involved.

## Open Questions

- None that change the specs, the approach, or the task breakdown.
