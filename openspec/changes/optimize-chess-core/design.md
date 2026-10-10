# Design

## Context

See `proposal.md` - Why. The core (`core/src/domain/`) wraps `shakmaty` in a
`BoardManager` plus a small negamax search. The mobile apps call it through
UniFFI (`GameSession`), so the FFI surface and every returned value are a
contract. Two hot paths matter on a phone:

- `search::best_move` (difficulty 1-3 => depth 2-4) runs negamax with
  alpha-beta and MVV-lite move ordering, with no transposition table or
  quiescence. `evaluate` builds `Vec<(Square, Role, Color)>` at every leaf.
- `BoardManager::get_valid_moves` calls `self.board.legals()` (full legal-move
  generation, including check detection) and then filters by square, on every
  call.

There is no benchmark: the only guard is `hard_move_from_start_fits_time_budget`
asserting one move under 1 s.

## Goals / Non-Goals

**Goals:**
- Remove per-leaf heap allocation from the search evaluation.
- Stop regenerating the full legal-move set for repeated square queries, without
  changing any query result.
- Establish a benchmark baseline so both changes are measurable and protected.
- Keep the FFI surface, move semantics, and determinism identical.

**Non-Goals:**
- Changing search strength (quiescence, transposition tables, iterative
  deepening) - a separate concern.
- Any change to the UniFFI surface, the online server, or the mobile UI.
- Changing the difficulty-to-depth mapping or the evaluation's chess meaning.

## Decisions

### D1 - Cache legal moves per position
`BoardManager` keeps a lazily-computed `Vec<Move>` of the position's legal moves,
invalidated on every `play_move`. `get_valid_moves(square)` filters that list,
and a new crate-level `legal_moves()` exposes the whole list. A single source of
truth means a query and a move check cannot disagree. Alternative: cache per
square; rejected because the set is small and one list covers all squares and
the play-path legality check.

### D2 - Allocation-free evaluation
Rewrite `evaluate` as two passes over the 64 squares: the first sums non-king
material (to pick endgame vs middlegame tables), the second accumulates
material + piece-square bonuses. No `Vec` is built, so leaf nodes allocate
nothing. Scores are computed identically to today (same tables, same sign
convention, same `noisy` jitter).

### D3 - Play path without a full clone
When a move is checked against the cached legal set (D1), it is known legal, so
`play_move` can apply it in place (`shakmaty`'s in-place play) instead of cloning
the whole `Chess`. This is a minor win and is only adopted if the pinned
`shakmaty` version exposes an in-place play; otherwise the existing
clone-then-play is kept (the clone is small and not the target of this change).

### D4 - Benchmark baseline
Add a `criterion` benchmark covering `best_move` at each difficulty from a
representative midgame FEN and a legal-move query loop. It is the measuring
instrument for D1/D2 and a regression guard. A dependency-free alternative
(`std::time` loop in a test) is the fallback if adding `criterion` is unwanted.

### D5 - Invariants preserved
The FFI surface is untouched. Medium/hard stay deterministic (same position +
difficulty => same move) and every difficulty returns a legal move; easy keeps
its xorshift variation. These are asserted by the existing tests plus the new
cache scenarios.

## Risks / Trade-offs

- [Stale cache returns wrong moves] -> the cache is invalidated on every
  `play_move` and a test asserts a query after a move reflects the new position;
  cached vs freshly computed answers are asserted equal.
- [shakmaty lacks an in-place play] -> keep clone-then-play (D3 is optional;
  the measured win is D1/D2).
- [Benchmark flakiness across machines] -> treat the benchmark as a local
  baseline/trend, not a hard CI gate.
- [Behavior drift while "optimizing"] -> the existing chess tests, the FFI
  smoke tests, and the new cache tests all run unchanged.

## Open Questions

- Whether to adopt quiescence / a transposition table for strength (and its
  effect on think time) - a separate change if wanted.
- Whether the mobile UI benefits from a `legal_moves()` all-at-once API or keeps
  querying per square (the crate-level API is added either way).
