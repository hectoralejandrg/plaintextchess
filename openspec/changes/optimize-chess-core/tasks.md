# Tasks

## 1. Benchmark baseline

- [ ] 1.1 Add a `criterion` benchmark in `core/benches/` covering `best_move` at difficulties 1-3 from a representative midgame FEN and a legal-move query loop, and verify `cargo bench` runs and prints a baseline
- [ ] 1.2 Record the baseline numbers (best_move time per difficulty, legal-move query time) in the design doc or a bench README so later changes can compare

## 2. Search evaluation without per-leaf allocation

- [ ] 2.1 Rewrite `search::evaluate` as two allocation-free passes over the board (material pass, then material + PST pass) and verify the existing search tests pass unchanged (`each_difficulty_returns_a_legal_move_from_start`, `promotion_capture_uses_full_uci`, `medium_and_hard_are_deterministic`, `hard_move_from_start_fits_time_budget`)
- [ ] 2.2 Verify the change removes the per-leaf allocation by checking the benchmark shows no regression and, if practical, that the allocation count per `best_move` drops (an allocation-counting test or a documented measurement)

## 3. Legal-move cache

- [ ] 3.1 Cache the position's legal moves in `BoardManager`, invalidated on every `play_move`, and expose both `get_valid_moves(square)` (filter from the cache) and a crate-level `legal_moves()` (the whole list); verify a query and a freshly generated set agree (`a_cached_answer_equals_a_freshly_computed_one`)
- [ ] 3.2 Verify cache invalidation: after `play_move`, a square's query reflects the new position, not a stale set (`a_move_invalidates_any_cached_moves`)
- [ ] 3.3 Verify an invalid or illegal move leaves the cache and position unchanged (`play_move` returns the error and later queries are unaffected)

## 4. Play path

- [ ] 4.1 Validate `play_move` against the cached legal set and apply the move in place where the pinned `shakmaty` version supports it (falling back to clone-then-play otherwise), and verify legal and illegal moves behave exactly as before (`play_move` results and errors unchanged)

## 5. Integration verification

- [ ] 5.1 Verify `cargo test` passes for the core (unit tests and `tests/ffi_smoke.rs`) and that behavior is unchanged: every difficulty returns a legal move, medium/hard stay deterministic, easy keeps varying
- [ ] 5.2 Verify the UniFFI surface is unchanged (`GameSession` functions, errors, and types are byte-identical in `uniffi` bindings) and that both client builds succeed with no UI or protocol change
- [ ] 5.3 Re-run the benchmark and report the before/after (best_move time and, where measured, allocations), confirming the search evaluation no longer allocates per leaf and queries do not regenerate the full legal set

## Workflow follow-up

- Archive the change after review, and verify the archived result and the updated `rust-core` spec (legal-move consistency requirement).
