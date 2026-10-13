# Proposal

## Why

A capacity test (`TIER=all`, the monolith, 2 vCPU / 2GB container, human
game pace) found the server's real ceiling is CPU, not memory or
connections: memory stayed flat at ~7.5MB through every load level tested,
including a 20,000-game burst, while the game-setup failure rate rose
sharply between 7,500 games (0% failures) and 10,000+ (35-67% failures).
Every failure had `plies=0` — the game never started; this is a setup-time
problem, not a move-processing one.

Tracing `CreateRoom`'s actual code path (`App::create_room_internal`,
`server/src/infrastructure/server.rs:516-553`) found a concrete, specific
cause: **every single `CreateRoom`/`JoinRoom` call acquires four separate
process-global `std::sync::Mutex` locks** (not a sharded structure like
the fabric/connection maps elsewhere in this codebase):

1. `check_room_limit` locks `self.players` (line 706) to check the
   one-active-game-per-device rule.
2. `unique_code` locks `self.rooms` (line 756) once per draw to check the
   candidate code isn't taken (collision odds are ~0.0015% at 20,000
   concurrent rooms given the 33^6 code space — this lock is drawn once
   per call in practice, not a retry storm).
3. `create_room_internal` locks `self.rooms` again (line 536) to insert
   the new room.
4. `track_identity` locks `self.players` again (line 739) to record the
   device's active room.

`std::sync::Mutex::lock()` **blocks the calling OS thread** — under a
burst of thousands of concurrent `CreateRoom` calls on a tokio runtime
with only as many worker threads as CPU cores (2, in the tested
container), every thread contending on the same lock stalls the runtime's
ability to make progress on *other* tasks while it holds/waits for that
lock. This is a strong, code-level explanation for a CPU-bound setup
bottleneck that has nothing to do with per-move processing, memory, or
network — consistent with every symptom measured (flat memory, no fd
pressure, failures concentrated in setup, not gameplay).

This mirrors a pattern this codebase has already solved elsewhere
(`CONN_LOGIC_SHARDS`/`OUTBOX_SHARDS` = 16-way sharded maps instead of one
global lock, in the split-tier code) — but the monolith's own room/player
registries were never given the same treatment, likely because the
monolith predates the concurrency work done on the split tiers.

## What Changes

This proposal is **investigate-then-fix**, matching this codebase's own
established convention (`reduce-fabric-contention`'s design explicitly
required "profiling report pinned two per-message shared resources" before
any code changed):

1. Confirm the hypothesis with a real profile (not just the code-reading
   evidence above) — measure actual lock-wait time during a burst, not
   just identify plausible contention points.
2. If confirmed, shard `rooms`/`players`/`account_rooms` the same way
   `CONN_LOGIC_SHARDS`/`OUTBOX_SHARDS` already are elsewhere in this
   codebase (hash-based sharding, same shard count convention), so a
   burst of concurrent `CreateRoom`s spreads across N locks instead of
   serializing on one.

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

(none — this is an internal concurrency/performance change; room
creation's observable behavior, guarantees, and wire protocol are
unchanged. No spec delta: `skip_specs: true`.)

## Impact

- **Server code**: `server/src/infrastructure/server.rs` (`App`'s
  `rooms`/`players`/`account_rooms` fields and every function that locks
  them: `create_room_internal`, `join_room`, `check_room_limit`,
  `track_identity`, `unique_code`, plus any room-removal/cleanup path that
  also touches these maps).
- **Clients**: none.
- **Tests**: existing `tests/rooms.rs`/`tests/persistence.rs` coverage of
  room creation, join, and the one-room-per-device rule must stay green
  unchanged (sharding must not alter any of those observable guarantees);
  a new concurrency-focused test or load-based verification is likely
  needed to actually demonstrate the fix, since the existing unit tests
  are correctness tests, not contention tests.
- **Related work**: this is the monolith (`TIER=all`) analog of the
  concurrency-sharding work already done for the split tiers
  (`CONN_LOGIC_SHARDS`/`OUTBOX_SHARDS` in `transport_handler.rs`/
  `logic_handler.rs`). It is a different, narrower problem than
  `tune-logic-lane-defaults` (which is about `TIER=logic`/`combined`'s
  lane configuration) — the monolith does not use the lane system at all,
  so that investigation's findings don't transfer here directly, though
  the underlying intuition (global serialization points don't scale) is
  the same lesson.
