# Tasks

## 1. Benchmark and measurement (gate)

- [ ] 1.1 Add a `criterion` benchmark in `core/benches/` covering `best_move` at difficulties 1-3 from representative midgame positions and a legal-move query loop, and verify `cargo bench` runs and prints a baseline
- [ ] 1.2 Measure allocations per `best_move` (an allocation-counting harness) and think time per difficulty, and record the baseline in `design.md` or a bench README
- [ ] 1.3 Verify how the iOS/Android apps call `get_valid_moves` (per square/tap/frame vs once per selection) and record the finding
- [ ] 1.4 Confirm which candidate cost (per-leaf allocation, repeated legal-move generation) is material, and gate: if neither is, stop and revisit the proposal before implementing groups 2-5

## 2. Search evaluation without per-leaf allocation

- [ ] 2.1 Rewrite `search::evaluate` as two allocation-free passes over the board (material pass, then material + PST pass) and verify the existing search tests pass unchanged (`each_difficulty_returns_a_legal_move_from_start`, `promotion_capture_uses_full_uci`, `medium_and_hard_are_deterministic`, `hard_move_from_start_fits_time_budget`)
- [ ] 2.2 Add an equivalence test asserting the new evaluation returns the same scores as the previous implementation on fixed positions (start, middlegame, endgame), so engine strength cannot drift
- [ ] 2.3 Verify the per-leaf allocation is gone: the allocation count per `best_move` drops (from the harness in 1.2) and the benchmark shows no regression

## 3. Legal-move cache, invalidated by every board change

- [ ] 3.1 Cache the position's legal moves in `BoardManager`, invalidated by every board-changing operation (`play_move` and any session restore), and expose `get_valid_moves(square)` (filter from the cache) plus a crate-level `legal_moves()`; verify a cached answer equals a freshly generated one (`a_cached_answer_equals_a_freshly_computed_one`)
- [ ] 3.2 Verify a played move invalidates the cache (`a_move_invalidates_any_cached_moves`)
- [ ] 3.3 Verify a restored session invalidates the cache (`a_restored_session_invalidates_any_cached_moves`)
- [ ] 3.4 Verify an invalid or illegal move leaves the cache and position unchanged
- [ ] 3.5 If 1.3 shows the apps query `get_valid_moves` only once per selection, drop this group and say so (the cache is not worth its state)

## 4. Play path

- [ ] 4.1 Validate `play_move` against the cached legal set and apply the move in place where the pinned `shakmaty` version supports it (falling back to clone-then-play otherwise), and verify legal and illegal moves behave exactly as before (`play_move` results and errors unchanged)

## 5. Time budget and integration verification

- [ ] 5.1 Document a CPU move time budget per difficulty on the reference hardware and assert the hardest move stays within it (`cpu_move_fits_documented_budget`)
- [ ] 5.2 Verify `cargo test` passes for the core (unit tests and `tests/ffi_smoke.rs`) and that behavior is unchanged: every difficulty returns a legal move, medium/hard stay deterministic, easy keeps varying
- [ ] 5.3 Verify the UniFFI surface is unchanged (`GameSession` functions, errors, and types are byte-identical in the `uniffi` bindings) and that both client builds succeed with no UI or protocol change
- [ ] 5.4 Re-run the benchmark and report the before/after (think time per difficulty, allocations), confirming the gate outcomes from group 1

## Workflow follow-up

- Archive the change after review, and verify the archived result and the updated `rust-core` spec (legal-move consistency and CPU move time budget).
