# Proposal

## Why

The shared Rust core (`core/`) runs on iOS and Android: every legal-move query
and every CPU move is spent on the client's CPU and battery. Reading the code
suggests two candidate costs - the negamax evaluation allocates a heap `Vec` of
every piece at every leaf node (`search.rs`), and `get_valid_moves(square)`
regenerates the position's entire legal-move set on every call (`board.rs`) - and
there is no benchmark baseline (only a `< 1s` assertion). Unlike the server work,
this is **code inspection, not measurement**: both costs must be confirmed on the
core before being optimized, and `get_valid_moves` only matters if the mobile UI
actually queries it per square or per interaction, which is unverified. There is
also no documented time budget for a CPU move, and the FFI call is synchronous,
so a hard move on a slow phone could stall the caller.

## What Changes

- **Measure first (gate)**: instrument the core to count allocations per
  `best_move` and measure think time per difficulty, and check how the apps call
  `get_valid_moves`; the optimizations below proceed only for costs the
  measurement confirms are material.
- Add a benchmark harness for the core (search and legal-move generation) to
  establish a measurable baseline and catch regressions.
- Replace the per-leaf allocation in the evaluation with two allocation-free
  passes over the board; the returned scores are **identical** (verified by an
  equivalence test).
- Cache the position's legal moves so repeated square queries do not regenerate
  the full legal-move set; the cache is invalidated by **every** board-changing
  operation, so queries always reflect the current position.
- Avoid cloning the whole position in `play_move` when the move is already known
  to be legal.
- **Document a CPU move time budget per difficulty** on the reference hardware
  and assert it, so a synchronous `get_cpu_move` has a bounded cost.
- No change to the UniFFI surface, the move semantics, or any game behavior:
  iOS and Android receive exactly the same results as today.

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `rust-core`: adds that legal-move queries reflect the current position after
  any board-changing operation (any internal caching is transparently invalidated
  and never changes results), and that a CPU move is produced within a documented
  time budget for each difficulty, while the CPU move stays legal for every
  difficulty and deterministic for medium/hard; the UniFFI surface is unchanged.

## Impact

- **Core code**: `core/src/domain/search.rs` (evaluation), `core/src/domain/board.rs`
  (legal-move cache and play path), `core/Cargo.toml` (benchmark dev-dependency).
- **Clients (iOS/Android)**: no UI, protocol, or FFI change; identical behavior,
  less CPU/battery.
- **Server**: unaffected (it uses the same crate but no behavior changes).
- **Specs**: modified `rust-core`.
