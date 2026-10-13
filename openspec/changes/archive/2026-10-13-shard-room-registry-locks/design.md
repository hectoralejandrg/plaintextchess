# Design

## Context

See `proposal.md` - Why. `App` (`server/src/infrastructure/server.rs:95+`)
holds three process-global `std::sync::Mutex`-guarded maps used by every
`CreateRoom`/`JoinRoom` on the monolith (`TIER=all`):

```rust
rooms: Mutex<HashMap<String, mpsc::UnboundedSender<RoomMsg>>>,   // line 99
players: Mutex<HashMap<String, String>>,                          // line 102
account_rooms: Mutex<HashMap<String, String>>,                    // line 107
```

Every call site that locks them:

| Lock | Call sites |
|---|---|
| `rooms` | `remove_room` (129), `room_codes` (335), `room_exists` (339), `join_room` (570), `unique_code` (756), `create_room_internal`'s insert (536) |
| `players` | `untrack_if` (145), `check_room_limit` (706), `track_identity` (739) |
| `account_rooms` | `untrack_if` (151), `check_room_limit` (716), `track_identity` (743) |

A single `CreateRoom` call acquires `players` once (`check_room_limit`),
`rooms` once (`unique_code`), `rooms` again (insert), and `players` again
(`track_identity`) — four lock acquisitions on two global mutexes, every
time, regardless of how many rooms already exist or how many CPUs are
available to serve other concurrent callers.

This codebase already has an established, working pattern for exactly this
problem: `CONN_LOGIC_SHARDS`/`OUTBOX_SHARDS` (`transport_handler.rs`) are
16-way sharded `Vec<Mutex<HashMap<...>>>`, indexed via the shared
`shard_index(key, shard_count)` helper (`transport.rs:125`), specifically
because a single global lock on a per-connection map was a measured
contention point when transport/logic were split into their own tiers.
The monolith's own room/player registries never got the same treatment.

## Goals / Non-Goals

**Goals:**
- Confirm, with a real profile (not just code-reading), that these locks
  are an actual contributor to the measured setup-failure rate under
  burst load — not just a plausible-looking culprit.
- If confirmed, shard the three maps using the exact same pattern already
  established for `CONN_LOGIC_SHARDS`/`OUTBOX_SHARDS`, so a burst of
  concurrent `CreateRoom`s spreads across N independent locks.
- Preserve every existing guarantee exactly: room code uniqueness,
  one-room-per-device, one-room-per-account, idempotent `untrack_if`.

**Non-Goals:**
- Changing the split tiers (`TIER=logic`/`combined`) — they don't use
  `App.rooms`/`players`/`account_rooms` the same way (room ownership is
  Redis-backed there via `Broker::claim_room`, already fixed separately in
  `fix-multi-node-rating-consistency`). This proposal is specifically about
  the monolith's in-memory registries.
- Resolving `tune-logic-lane-defaults` — a related but distinct, separate
  open investigation about a different tier's configuration.
- Any wire-protocol or client-visible change.

## Plan

### Step 1 - Confirm with a profile, not just code-reading

Before sharding anything, measure actual lock-wait time during the exact
burst scenario that found this (the capacity test: `TIER=all`, 2 vCPU/2GB
container, `load_gen.rs --games 10000 --pace-ms 8000 --ramp-ms 1000`,
which measured 35.8% setup failures). Options, cheapest first:
- Add temporary `Instant::now()`-based timing around each `.lock()` call
  in the four hot functions, logging/aggregating wait time (same spirit as
  this codebase's existing `record_publish_wait_ns`/`record_publish_process_ns`
  pattern in `broker.rs`, which already distinguishes "time waiting for a
  shared resource" from "time doing the work").
- Or: `cargo flamegraph`/`perf` on the monolith process during the burst,
  looking for time spent in `std::sync::Mutex` internals (futex wait) on
  the worker threads.

If the profile does **not** show meaningful lock-wait time, stop here —
the hypothesis was wrong, and the real cause (tokio scheduler saturation
from raw task count, Argon2 cost for a different auth path, something
else) needs its own investigation rather than a sharding fix that wouldn't
help.

### Step 2 - If confirmed: shard the three maps

Reuse `shard_index` (`transport.rs:125`) and the same shard-count
convention (16, matching `CONN_LOGIC_SHARDS`/`OUTBOX_SHARDS`) rather than
picking a new number. Shape (illustrative, not final):

```rust
rooms: Vec<Mutex<HashMap<String, mpsc::UnboundedSender<RoomMsg>>>>,      // sharded by room code
players: Vec<Mutex<HashMap<String, String>>>,                            // sharded by device_id
account_rooms: Vec<Mutex<HashMap<String, String>>>,                      // sharded by account_id
```

Every call site above moves from `self.rooms.lock()` to
`self.rooms[shard_index(key, N)].lock()`, where `key` is the room code
(for `rooms`) or device/account id (for `players`/`account_rooms`) — the
same key each map is already indexed by today, so sharding only changes
*which lock* guards a given key, not any lookup semantics.

**One genuine wrinkle**: `room_codes()` (line 335, used by the logic
tier's `shutdown()` to release every room it owns) and `room_exists`
(line 339) iterate the *whole* map today — sharded, these become "iterate
all N shards" instead of "lock the one map," the same shape
`membership::all_nodes`/the transport's per-shard iteration already use
elsewhere in this codebase. Not a correctness problem, just something to
implement consistently with that existing pattern, not improvise anew.

## Risks / Trade-offs

- [Sharding the wrong thing] - mitigated by Step 1's profile-first gate,
  the same discipline `reduce-fabric-contention` used and this session's
  own `tune-logic-lane-defaults` investigation learned the hard way
  (a plausible-looking fix that wasn't verified made things worse once).
- [More complexity for a monolith that's supposed to be the simple path] -
  real, but the alternative (an unfixed CPU ceiling around ~7,500-8,500
  concurrent room creations on a 2-vCPU box) is a worse trade-off if the
  profile confirms the hypothesis; `TIER=all`'s whole value proposition is
  being the simple *deployment* path, not necessarily the simplest
  *internal* implementation.
- [Sharded iteration (`room_codes`) is slower per-call than one lock] -
  negligible: that path runs once at shutdown, not on the request hot path.

## Open Questions

- Does the profile actually confirm lock contention, or point somewhere
  else entirely (tokio scheduler, Argon2, something in the WebSocket
  accept path)? This gates whether step 2 happens at all.
- Should `tokio::sync::Mutex` (async-aware, doesn't block the OS thread)
  be considered as an alternative/complement to sharding for these maps,
  given the critical sections are all short, synchronous HashMap
  operations where a `std::sync::Mutex` is normally the *right* choice
  (per Rust's own guidance against using async mutexes for short critical
  sections) — sharding a `std::sync::Mutex` is likely still the better fit
  here, but worth confirming against the profile data rather than assuming.

## Outcome (2026-10-13) — gate 1.3: hypothesis REFUTED

**Method.** Temporary `Instant::now()` instrumentation (the same
"wait vs. work" shape as `record_publish_wait_ns`/`record_publish_process_ns`)
was added around every `.lock()` in the four hot functions
(`check_room_limit`, `unique_code`, `create_room_internal`'s insert,
`track_identity`): per-map `wait` ns (blocked on the mutex) and `hold` ns
(inside the critical section), exposed on the monolith's `GET /healthz` under
`registry_lock_ns`. The 2-vCPU mechanism the proposal names — only as many
worker threads as cores — was reproduced with `TOKIO_WORKER_THREADS=2`
(Tokio's documented way to pin the multi-thread runtime's worker count), on
the M2 Max dev host, `TIER=all`, `DATABASE_URL=sqlite:/tmp/...`, using the
same `load_gen.rs` client.

**Runs.**

| games | pace | ok | failed | rooms wait | players wait | account wait | total wait | total hold |
|---|---|---|---|---|---|---|---|---|
| 500 | 60 ms | 500 | 0 | 0.06 ms | 0.20 ms | 0 | **0.27 ms** | 0.97 ms |
| 10,000 | 8000 ms | 7,531 | **2,469 (24.7 %)** | 2.32 ms | 6.50 ms | 0 | **8.82 ms** | 5.19 ms |

The 10,000-game run is a genuine breaking point (the proposal's baseline was
35.8 % setup failures; 24.7 % here, lower because the host was not CPU-capped
at 2 vCPU as the container was). During and just after the setup burst the
monolith could not even answer `GET /healthz` (`curl` failed → the load
generator recorded `null`), and the move phase ran at only ~940 moves/s with
ack p50 = 64.7 ms / p95 = 493.5 ms / p99 = 632.4 ms.

**Verdict.** Total time blocked on the three registry mutexes across the
entire 10,000-game run was **8.82 ms** — about **0.1 µs per acquisition** and
**<=0.03 %** of the >=30 s window in which the 2,469 games timed out. The maps
are effectively uncontended; sharding them (section 2) would not move the
failure rate. Gate 1.3 says: stop here. The evidence instead points at the
alternative this design already named — **event-loop / scheduler starvation
under raw task + connection count** in the accept/setup path (unreachable
`/healthz`, high ack latency at low throughput), consistent with QA's R6-D
"seating bottleneck" finding.

**Caveats.** `load_gen.rs` seats guests only, so `account_rooms` was never
exercised (0 ns); `rooms` and `players` were profiled. `account_rooms` has the
same shape and strictly less traffic, so it is not expected to differ, but this
profile did not measure it.

**Disposition.** The instrumentation was profiling scaffolding and has been
reverted (the create/join hot path is back to plain `Mutex` locks). Sections
2-3 are intentionally not implemented. A future investigation of this ceiling
should target the scheduler/accept path (worker-thread count, accept backlog,
per-connection task cost), not these maps.
