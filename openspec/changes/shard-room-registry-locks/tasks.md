# Tasks

## 1. Confirm the hypothesis (gate — do not skip)

- [x] 1.1 Reproduce the capacity test's breaking point (`TIER=all`, 2 vCPU /
  2GB container, `load_gen.rs --games 10000 --pace-ms 8000 --ramp-ms 1000`,
  previously measured 35.8% setup failures) with lock-wait instrumentation
  added to `check_room_limit`/`unique_code`/`create_room_internal`'s
  insert/`track_identity`
- [x] 1.2 Record actual time spent waiting on `rooms`/`players`/
  `account_rooms` locks during the burst vs. time spent doing the actual
  work inside the critical section
- [x] 1.3 Decide: if lock-wait time is a small fraction of the failure
  window, stop here and investigate the real cause instead (tokio
  scheduler saturation, something else in the accept/setup path) — do not
  proceed to section 2 on an unconfirmed hypothesis
  → **Refuted.** Lock-wait was 8.82 ms total against a >=30 s failure
  window (<=0.03 %). Instrumentation reverted; section 2 intentionally not
  done. See `design.md` "Outcome".

## 2. Shard the three maps (NOT DONE — gate 1.3 refuted the hypothesis)

- [ ] 2.1 `rooms`, `players`, `account_rooms` become `Vec<Mutex<HashMap<...>>>`,
  16-way (matching `CONN_LOGIC_SHARDS`/`OUTBOX_SHARDS`'s existing
  convention), indexed via the existing `shard_index` helper
  (`transport.rs:125`) — no new sharding mechanism invented
- [ ] 2.2 Update every call site: `remove_room`, `untrack_if`,
  `check_room_limit`, `track_identity`, `unique_code`,
  `create_room_internal`'s insert, `join_room`'s lookup
- [ ] 2.3 `room_codes()`/`room_exists()` iterate all shards (same shape as
  existing per-shard iteration patterns elsewhere in this codebase, e.g.
  `membership::all_nodes` or the transport's shard loops) — not a new
  pattern improvised for this case
- [ ] 2.4 Confirm every existing guarantee still holds byte-for-byte: room
  code uniqueness, one-room-per-device, one-room-per-account, idempotent
  `untrack_if` on a stale/already-removed entry

## 3. Verify (NOT DONE — only applies if section 2 ships)

- [ ] 3.1 `cargo test --lib --tests` green, no behavior change to any
  existing room/registry test (`tests/rooms.rs`, `tests/persistence.rs`,
  the server unit tests already covering the one-room-per-device/account
  rules)
- [ ] 3.2 Re-run the exact capacity test that found this
  (`TIER=all`, 2 vCPU/2GB, the 7,500/8,500/10,000-game levels) and confirm
  the failure rate at 10,000 games drops meaningfully from the measured
  35.8% baseline — the real confirmation, not just "the code compiles and
  unit tests pass"
- [ ] 3.3 Re-profile (same method as 1.1) to confirm lock-wait time
  actually dropped, not just that the failure count did (rules out a
  different, coincidental cause)

## Workflow follow-up

- Gate 1.3 **refuted** the hypothesis (2026-10-13): total registry lock-wait
  during the 10,000-game breaking-point burst was 8.82 ms against a >=30 s
  failure window (`design.md` "Outcome"). Sections 2-3 are intentionally
  not done, the temporary instrumentation was reverted, and this change is
  closed with the negative result documented — not silently abandoned.
