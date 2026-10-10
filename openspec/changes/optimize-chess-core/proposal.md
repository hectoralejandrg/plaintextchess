# Proposal

## Why

The shared Rust core (`core/`) runs on iOS and Android: every legal-move query
and every CPU move is spent on the client's CPU and battery. Two implementation
choices cost more than they need to. The negamax evaluation allocates a heap
`Vec` of every piece at every leaf node (`search.rs`), so a single hard move
makes tens of thousands of short-lived allocations. And `get_valid_moves(square)`
regenerates the position's entire legal-move set on every call (`board.rs`), so a
board UI that asks per square or per interaction repeats the whole generation.
There is also no benchmark baseline - only a `< 1s` assertion on one move - so
neither improvement can be measured or protected against regression.

## What Changes

- Add a benchmark harness for the core (search and legal-move generation) to
  establish a measurable baseline and catch regressions.
- Replace the per-leaf allocation in the evaluation with two allocation-free
  passes over the board; the returned scores are unchanged.
- Cache the position's legal moves so repeated square queries do not regenerate
  the full legal-move set; the cache is invalidated on every move, so queries
  always reflect the current position.
- Avoid cloning the whole position in `play_move` when the move is already known
  to be legal.
- No change to the UniFFI surface, the move semantics, or any game behavior:
  iOS and Android receive exactly the same results as today.

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `rust-core`: adds a guarantee that legal-move queries reflect the current
  position (any internal caching is transparently invalidated by a move and
  never changes results), while the CPU move stays legal for every difficulty
  and deterministic for medium/hard; the UniFFI surface is unchanged.

## Impact

- **Core code**: `core/src/domain/search.rs` (evaluation), `core/src/domain/board.rs`
  (legal-move cache and play path), `core/Cargo.toml` (benchmark dev-dependency).
- **Clients (iOS/Android)**: no UI, protocol, or FFI change; identical behavior,
  less CPU/battery.
- **Server**: unaffected (it uses the same crate but no behavior changes).
- **Specs**: modified `rust-core`.
