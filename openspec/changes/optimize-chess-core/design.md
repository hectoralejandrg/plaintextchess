# Design

## Context

See `proposal.md` - Why. The core (`core/src/domain/`) wraps `shakmaty` in a
`BoardManager` plus a small negamax search. The mobile apps call it through
UniFFI (`GameSession`), so the FFI surface and every returned value are a
contract. Code inspection suggests two hot paths on a phone:

- `search::best_move` (difficulty 1-3 => depth 2-4) runs negamax with alpha-beta
  and MVV-lite ordering, with no transposition table or quiescence. `evaluate`
  builds `Vec<(Square, Role, Color)>` at every leaf.
- `BoardManager::get_valid_moves` calls `self.board.legals()` (full legal-move
  generation, including check detection) and filters by square, on every call.

Critically, **neither cost is measured**: there is no benchmark, only
`hard_move_from_start_fits_time_budget` asserting one move under 1 s on the dev
machine, and no knowledge of how often the apps call `get_valid_moves`. The
server work in this repo showed the cost of optimizing an unmeasured guess, so
this change measures first.

## Goals / Non-Goals

**Goals:**
- Measure the core before optimizing: allocations per `best_move`, think time per
  difficulty, and the apps' real `get_valid_moves` usage.
- Remove per-leaf heap allocation from the search evaluation (scores identical).
- Stop regenerating the full legal-move set for repeated square queries, without
  changing any query result.
- Document and assert a CPU move time budget per difficulty.
- Keep the FFI surface, move semantics, and determinism identical.

**Non-Goals:**
- Changing search strength (quiescence, transposition tables, iterative
  deepening) - a separate concern.
- Running the CPU move off the caller's thread (the FFI is synchronous); if the
  budget shows the UI can stall, that is a follow-up (see Open Questions).
- Any change to the UniFFI surface, the online server, or the mobile UI.
- Changing the difficulty-to-depth mapping or the evaluation's chess meaning.

## Decisions

### D1 - Measure the core first (gate)
Before any rewrite, add a benchmark and an allocation count: allocations per
`best_move` at each difficulty and think time on fixed positions, plus a check of
how the iOS/Android apps call `get_valid_moves`. The later decisions proceed only
for costs the measurement confirms are material; if neither dominates, stop and
revisit the proposal. Alternative: optimize straight from code reading - rejected
because it is the mistake the server work already showed to avoid.

### D2 - Cache legal moves per position
`BoardManager` keeps a lazily-computed `Vec<Move>` of the position's legal moves,
invalidated by **every** board-changing operation: `play_move`, and any session
restore that replaces the board. `get_valid_moves(square)` filters that list and a
new crate-level `legal_moves()` exposes the whole list. A single source of truth
means a query and a move check cannot disagree. Adopted only if D1 shows the apps
query repeatedly. Alternative: cache per square; rejected as unnecessary.

### D3 - Allocation-free evaluation
Rewrite `evaluate` as two passes over the 64 squares: the first sums non-king
material (to pick endgame vs middlegame tables), the second accumulates material +
piece-square bonuses. No `Vec` is built, so leaf nodes allocate nothing. Scores
must be **bit-identical** to today (same tables, sign convention, and `noisy`
jitter), guarded by an equivalence test against the previous implementation.

### D4 - Play path without a full clone
When a move is checked against the cached legal set (D2), it is known legal, so
`play_move` can apply it in place (`shakmaty`'s in-place play) instead of cloning
the whole `Chess`. Minor, and only adopted if the pinned `shakmaty` exposes an
in-place play; otherwise keep clone-then-play.

### D5 - Benchmark and a documented time budget
Add a `criterion` benchmark covering `best_move` at each difficulty from
representative midgame positions and a legal-move query loop; it is the measuring
instrument for D1/D2/D3 and a regression guard. From the measured think times,
document a **per-difficulty time budget** on the reference hardware and assert the
hardest move stays within it. A dependency-free `std::time` test is the fallback
if `criterion` is unwanted.

### D6 - Invariants preserved
The FFI surface is untouched. Medium/hard stay deterministic (same position +
difficulty => same move) and every difficulty returns a legal move; easy keeps its
xorshift variation. Asserted by the existing tests plus the new spec scenarios.

## Risks / Trade-offs

- [Optimizing an unmeasured cost] -> D1 is a gate; the rewrite stops if the cost
  is not material.
- [Stale cache returns wrong moves] -> the cache is invalidated by every
  board-changing operation (moves and restores), and cached vs freshly computed
  answers are asserted equal.
- [Efficiency rewrite changes engine strength] -> an equivalence test asserts the
  new `evaluate` returns the same scores on fixed positions.
- [shakmaty lacks an in-place play] -> keep clone-then-play (D4 is optional).
- [Benchmark flakiness across machines] -> treat it as a local baseline/trend, not
  a hard CI gate; the time budget is per reference hardware.

## Open Questions

- How often do the iOS/Android apps call `get_valid_moves`? D1 answers whether the
  cache is worth its state. If not, D2 is dropped.
- Whether the measured think time warrants running the CPU move off the caller's
  thread (the FFI is synchronous today) - a follow-up if the budget is tight on
  slow devices.
- Whether to adopt quiescence / a transposition table for strength - a separate
  change.
