# Proposal

## Why

`shard-fabric` and the room-ownership/routing work before it made it possible
to run more than one logic-tier node, each owning disjoint rooms (atomic
`SET NX` claim, round-robin placement, routing by `get_room_owner`). But each
logic node keeps its own independent SQLite file (design D7 of
`split-realtime-architecture`, to avoid `SQLITE_BUSY` from concurrent
writers). A player's rating is keyed by `device_id`, not by room — so once
rooms are placed round-robin across nodes, the same player's consecutive
games can be owned by different logic nodes, and the node that never saw
their previous game shows them the default 1500 instead of their real
rating. This is a silent correctness bug (wrong rating, not a crash), and it
is the one remaining blocker to safely running more than one logic node in
production.

QA verified the fix end-to-end (rating survives a cross-node room
placement, confirmed against real Redis) and, while testing it under
adversarial conditions (Redis stalled, Redis killed, a full outage +
restart), found three further gaps in how the control shard and the
inbound publish path behave when Redis misbehaves — one of them
(`node lease had lapsed while alive` flapping for ~11 minutes after a full
Redis restart, confirmed to delete at least one *live* node's own lease
key) is a reliability bug independent of rating, serious enough to block
calling multi-logic-node operation production-ready.

## What Changes

- **Cross-node rating mirror**: a Redis hash per device
  (`ptc.rating.<device_id>`), written (last-write-wins, guarded by
  `updated_at_ms`) when a game ends, and read whenever a device is seated
  in a new room — so the rating a player sees never depends on which logic
  node happens to own their current game.
- **Bounded control-shard command latency**: `redis-rs`'s `ConnectionManager`
  defaults to `response_timeout = Duration::MAX` (confirmed by QA and by
  reading `redis-rs`'s source) — a stalled (not dead) Redis could hang any
  control-shard command (ownership claim, lease renewal, session mirror,
  rating mirror) indefinitely. `reduce-fabric-contention`'s design already
  flagged this as an accepted risk with an explicit escape hatch ("a coarse
  timeout is kept if needed") for the separate no-timeout hot publish
  connection; this change exercises that escape hatch for the
  `ConnectionManager` the control shard uses.
- **Inbound publish path health** (hot path, cannot take a per-command
  timeout without reintroducing the per-message timer cost
  `reduce-fabric-contention` eliminated): a periodic, bounded health probe
  per shard's publish connection, reusing the existing `shard_healthy`
  pattern (today only driven by the outbound subscription), so a stalled
  publish connection is detected and new inbound frames (including
  `CreateRoom`) fail fast instead of hanging, without adding any per-move
  overhead on the healthy path.
- **Lease-flapping / stale-reaper fix** (root cause confirmed, fixed — see
  `design.md` D4): a TOCTOU race in `liveness::renew`/`membership::renew`
  (fixed with an atomic Lua script), a subscribe-retry budget far shorter
  than a real outage that caused a node to self-unregister and crash-exit
  mid-outage (budget raised from ~17s to ~5 minutes), and a third bug found
  while verifying the first two: a missing debounce in the transport's
  watch of logic-node membership, causing a real, reproduced test flake.
- **Test-harness correctness** (not a product requirement, listed for
  completeness): `tests/split_e2e.rs`'s multi-logic harness leaks five
  `tokio::spawn`ed background tasks per logic node (lease renewal, room
  ownership keep-alive, lane workers, liveness watcher) that outlive the
  test and keep rewriting Redis for their TTL window into the next test,
  plus four literal duplicate `PORT_*` constants causing intermittent
  `TcpListener::bind` races — together explaining the suite's run-to-run
  flakiness (42/47 → 46/47 → 47/47 across three identical runs).

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `server`: the per-player rating requirement extends to multi-logic-node
  deployments — a device's rating must be consistent regardless of which
  logic node currently owns its game.
- `realtime-transport`: control-shard commands and the inbound publish path
  must both be bounded against an unresponsive (not just a dead) Redis,
  by two different mechanisms suited to their call frequency.

## Impact

- **Server code**: `infrastructure/broker.rs` (rating mirror module,
  `ConnectionManagerConfig` timeout, publish-path health probe),
  `application/ports.rs` (`Ratings::seed`, `RatingMirror` trait),
  `infrastructure/ratings.rs`, `infrastructure/recorder.rs`
  (`NoopRatingMirror`), `infrastructure/server.rs` (wiring),
  `application/room_actor.rs` (mirror write/read hooks), `infrastructure/
  logic_handler.rs` (background-task lifecycle, pending finding (b)'s fix).
- **Tests**: `tests/split_e2e.rs` (new `rating_survives_cross_node_room_
  placement` test, background-task cleanup, de-duplicated ports).
- **Clients (iOS/Android)**: no change — this is entirely server-internal.
- **Specs**: modified `server`, modified `realtime-transport`.
- **Non-goals**: account/profile consistency across logic nodes (sessions
  are already mirrored and solve the highest-risk case; account creation/
  profile data is a separate, lower-priority gap, not addressed here).
