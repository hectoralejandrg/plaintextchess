# Tasks

## 1. Benchmark and measurement (gate)

- [x] 1.1 Add a `criterion` benchmark in `core/benches/` covering `best_move` at difficulties 1-3 from representative midgame positions and a legal-move query loop, and verify `cargo bench` runs and prints a baseline
- [x] 1.2 Measure allocations per `best_move` (an allocation-counting harness) and think time per difficulty, and record the baseline in `design.md` or a bench README
- [x] 1.3 Verify how the iOS/Android apps call `get_valid_moves` (per square/tap/frame vs once per selection) and record the finding
- [x] 1.4 Confirm which candidate cost (per-leaf allocation, repeated legal-move generation) is material, and gate: if neither is, stop and revisit the proposal before implementing groups 2-5

## 2. Search evaluation without per-leaf allocation

- [x] 2.1 Rewrite `search::evaluate` as two allocation-free passes over the board (material pass, then material + PST pass) and verify the existing search tests pass unchanged (`each_difficulty_returns_a_legal_move_from_start`, `promotion_capture_uses_full_uci`, `medium_and_hard_are_deterministic`, `hard_move_from_start_fits_time_budget`)
- [x] 2.2 Add an equivalence test asserting the new evaluation returns the same scores as the previous implementation on fixed positions (start, middlegame, endgame), so engine strength cannot drift
- [x] 2.3 Verify the per-leaf allocation is gone: the allocation count per `best_move` drops (from the harness in 1.2) and the benchmark shows no regression

## 3. Legal-move cache, invalidated by every board change

- [x] 3.1 Dropped: gate outcome shows `get_valid_moves` called once per selection (seconds apart), cache not worth its state
- [x] 3.2 Dropped: gate outcome shows `get_valid_moves` called once per selection (seconds apart), cache not worth its state
- [x] 3.3 Dropped: gate outcome shows `get_valid_moves` called once per selection (seconds apart), cache not worth its state
- [x] 3.4 Dropped: gate outcome shows `get_valid_moves` called once per selection (seconds apart), cache not worth its state
- [x] 3.5 Gate outcome: apps query `get_valid_moves` only once per piece selection (iOS `select(_:)`, Android `select(...)`), not per square/tap/frame. The cache is not worth its state; group 3 dropped.

## 4. Play path

- [x] 4.1 Gate outcome: no cache (group 3 dropped), so no known-legal move from cache. `play_move` keeps clone-then-play (design D4's fallback). Existing behavior verified unchanged by tests.

## 5. Time budget and integration verification

- [x] 5.1 Document a CPU move time budget per difficulty on the reference hardware and assert the hardest move stays within it (`cpu_move_fits_documented_budget`)
- [x] 5.2 Verify `cargo test` passes for the core (unit tests and `tests/ffi_smoke.rs`) and that behavior is unchanged: every difficulty returns a legal move, medium/hard stay deterministic, easy keeps varying
- [x] 5.3 Verify the UniFFI surface is unchanged (`GameSession` functions, errors, and types are byte-identical in the `uniffi` bindings) and that both client builds succeed with no UI or protocol change
- [x] 5.4 Re-run the benchmark and report the before/after (think time per difficulty, allocations), confirming the gate outcomes from group 1

## Workflow follow-up

- Archive the change after review, and verify the archived result and the updated `rust-core` spec (legal-move consistency and CPU move time budget).
