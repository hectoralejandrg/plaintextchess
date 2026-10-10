# Chess core benchmarks and measurements

Instruments for change `optimize-chess-core` (design D1/D5):
the measurement gate's baseline and the regression guard for
the optimizations that follow it.

## Running

```sh
# Timing baseline/regression guard (criterion, release):
cargo bench --bench chess_bench

# Allocation-counting harness (release, like a phone build):
cargo run --release --example alloc_harness
```

Both use the same two representative midgame positions:

- `italian` — the Italian game after 2...Nc6
  (`r1bqkbnr/pppp1ppp/2n5/4p3/4P3/5N2/PPPP1PPP/RNBQKB1R w KQkq - 2 3`)
- `ruy_lopez` — the Ruy Lopez after 4...Nf6
  (`r1bqk2r/pppp1ppp/2n2n2/1B2p3/4P3/5N2/PPPP1PPP/RNBQK2R w KQkq - 2 5`)

## Reference hardware

Apple M2 Max, 12 cores, 32 GB (the dev machine that doubles as
the reference hardware for the CPU move time budget). Numbers
below are from that machine, release build, Rust 1.99.0.

## Baseline (before optimization, 2026-10-10)

### `best_move` think time (criterion, median estimate)

| position    | difficulty 1 (easy) | difficulty 2 (medium) | difficulty 3 (hard) |
|-------------|---------------------|-----------------------|---------------------|
| italian     | ~248 µs             | ~1.27 ms              | ~22.4 ms            |
| ruy_lopez   | ~248 µs             | ~1.44 ms              | ~8.95 ms            |

### Allocations per `best_move` (alloc harness, stable across runs)

| position    | difficulty 1 | difficulty 2 | difficulty 3       |
|-------------|--------------|--------------|--------------------|
| italian     | 3,369 allocs / 155 KB | 12,668 allocs / 656 KB | **273,844 allocs / 13.0 MB** |
| ruy_lopez   | 3,374 allocs / 155 KB | 14,249 allocs / 741 KB | **102,601 allocs / 5.0 MB** |

`Chess`/`Board` are pure bitboards (no heap), so every
allocation above comes from the search itself: one `Vec<Move>`
per node in `ordered_moves` and one `Vec<(Square, Role, Color)>`
per leaf in `evaluate`. At depth 4 leaves dominate the node
count, so the per-leaf `Vec` is the large majority of the
102k-274k allocations per hard move.

### Legal-move generation (criterion)

- `legals()` full generation: ~105 ns (italian), ~126 ns
  (ruy_lopez). Stack-allocated `ArrayVec` — no heap allocation
  from generation itself.
- `get_valid_moves` per call: ~155 ns including the
  `Vec<String>` collection (~30 allocations, ~1 KB).
- Worst-case query loop (one `get_valid_moves` per square, all
  64 squares): ~9.9 µs.

## Finding (task 1.3): how the apps call `get_valid_moves`

Code inspection of both clients — the only call sites:

- iOS `ios/app/PlainTextChess/GameViewModel.swift`, `select(_:)`:
  one `session.getValidMoves(square:)` call when the player taps
  one of their own pieces.
- Android `android/app/.../GameViewModel.kt`, `select(...)`: the
  same — one `session.getValidMoves(square = squareName)` call
  per piece selection.

Neither app queries per square, per tap, or per frame: the call
happens **once per piece selection**, i.e. at human-tap rate
(seconds apart). `getCpuMove` is likewise called once per CPU
turn. (The drag gesture asks `canPickup`, which reads the local
board snapshot and never calls the core.)

## Gate outcome (task 1.4)

- **Per-leaf allocation: material.** 102k-274k heap allocations
  (5-13 MB of churn) per hard `best_move`, the dominant share of
  its allocations and a large share of its 9-22 ms think time —
  all of it spent on the client's CPU and battery for every CPU
  move. → Group 2 (allocation-free evaluation) proceeds.
- **Repeated legal-move generation: not material.** One full
  generation is ~105-126 ns and the apps call it once per
  selection (task 1.3), seconds apart. Even the worst-case
  all-squares loop is ~10 µs. → Group 3 (legal-move cache) is
  dropped per task 3.5: the cache is not worth its state, and
  group 4's in-place play loses its precondition (a known-legal
  move from the cache), so `play_move` keeps clone-then-play
  (design D4's fallback).

## After optimization

Task 5.4 re-runs both instruments and compares against this
baseline. After the allocation-free evaluation (group 2):

### `best_move` think time (criterion, median estimate)

| position    | difficulty 1 | difficulty 2 | difficulty 3 |
|-------------|--------------|--------------|--------------|
| italian     | ~152 µs (−39%) | ~997 µs (−21%) | ~16.2 ms (−28%) |
| ruy_lopez   | ~156 µs (−37%) | ~1.10 ms (−24%) | ~6.56 ms (−27%) |

### Allocations per `best_move` (alloc harness, stable across runs)

| position    | difficulty 1 | difficulty 2 | difficulty 3 |
|-------------|--------------|--------------|--------------|
| italian     | 29 allocs / 4 KB (−99%) | 864 allocs / 125 KB (−93%) | 5,896 allocs / 925 KB (−98%) |
| ruy_lopez   | 34 allocs / 4 KB (−99%) | 869 allocs / 139 KB (−94%) | 3,381 allocs / 493 KB (−97%) |

The per-leaf `Vec` is gone: the remaining allocations are the
per-node `ordered_moves` `Vec`s (one per visited internal node —
5,896 at italian/hard). The legal-move benches are unchanged
(~104/124 ns generation, ~9.9 µs query loop): that code was not
touched, consistent with the gate outcome (group 3 dropped).

## CPU move time budget (task 5.1)

Per-difficulty budget on the reference hardware (Apple M2
Max, release build, Rust 1.99.0 — what ships to the apps),
with a comfortable margin above the measured median think
times to account for variance and thermal throttling:

| difficulty | budget | measured (release) |
|------------|--------|--------------------|
| 1 (easy)   | 5 ms   | ~0.15 ms           |
| 2 (medium) | 50 ms  | ~1 ms              |
| 3 (hard)   | 100 ms | ~6.6-16.2 ms       |

The `cpu_move_fits_documented_budget` test asserts every
difficulty stays within its budget, from the start position
and the two representative midgame positions. Run
`cargo test --release` to assert these budgets; plain
`cargo test` builds in debug, which is ~20x slower on this
hardware, so it allows 2 s per difficulty instead.

A synchronous `get_cpu_move` call therefore has a bounded
cost on the reference hardware (a hard move cannot exceed
100 ms there). On slower devices the call scales up
proportionally; moving the CPU move off the caller's
thread remains a follow-up (design.md, Open Questions).
