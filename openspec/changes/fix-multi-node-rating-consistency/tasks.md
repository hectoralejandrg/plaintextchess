# Tasks

## 1. Cross-node rating mirror (implemented, verified)

- [x] 1.1 `broker.rs`: `pub mod rating` (Redis hash, last-write-wins guard
  via Lua comparing `updated_at_ms`), `impl RatingMirror for Broker`
- [x] 1.2 `ports.rs`: `Ratings::seed`, `RatingMirror` trait,
  `RoomServices.rating_mirror`
- [x] 1.3 `recorder.rs`: `NoopRatingMirror` (monolith / single-logic-node)
- [x] 1.4 `server.rs`: `services()` wires `rating_mirror` from `self.broker`
  or `NoopRatingMirror`
- [x] 1.5 `room_actor.rs`: write hook in `apply_terminal_ratings` (both
  players, concurrent via `tokio::join!`), read hook
  `refresh_rating_from_mirror` at seat assignment (creator and joiner)
- [x] 1.6 `tests/split_e2e.rs`: `rating_survives_cross_node_room_placement`
  — passed against real Redis (1500 → 1599.52 → 1641.43, carried across a
  logic-node switch)
- [x] 1.7 `cargo test --lib --tests` and `split_e2e --ignored` green with
  real Redis

## 2. Bounded control-shard command latency (implemented, verified)

- [x] 2.1 `broker.rs`: `COMMAND_RESPONSE_TIMEOUT = 3s`,
  `ConnectionManager::new_with_config` in `Broker::connect`
- [x] 2.2 `room_actor.rs`: both players' rating mirrors run concurrently
  (`tokio::join!`), not sequentially, so the timeout is paid at most once
- [x] 2.3 QA re-verification: Redis `SIGSTOP`, resign → both players
  received the terminal result in ~3.0s; mirror timeouts for both players
  fired ~112μs apart (confirms concurrency); happy-path latency unaffected
  (26ms); `CreateRoom`/`Register` with Redis killed reject immediately
  (0ms); full suite green

## 3. Inbound publish path health (implemented)

- [x] 3.1 `transport_handler.rs`: `publish_healthy: Vec<AtomicBool>`,
  `spawn_publish_health_check` — periodic (2s) bounded (3s) `PING` probe per
  shard via `Broker::ping_publish_shard`, independent of `shard_healthy`
  (which only reflects the outbound subscription)
- [x] 3.2 `handle_transport_text` checks `publish_shard_healthy` before
  attempting a publish; fails fast with `Outbound::BrokerUnavailable`
  instead of hanging
- [x] 3.3 Wired into both `run_transport` and `run_combined` in
  `ws-server.rs`, alongside the existing `spawn_dispatcher`/`spawn_liveness`/
  `spawn_logic_watch` calls
- [x] 3.4 Re-verified against the exact QA scenario: `CreateRoom` sent to a
  real transport+logic pair with Redis `SIGSTOP`'d closed with
  `1011 broker unavailable` in ~3007ms (bounded by the probe timeout),
  down from QA's >10s unbounded hang. `load_gen` sanity check on
  `TIER=combined` (300 games) after the change: p50=6.03ms/p95=8.34ms/
  p99=8.90ms — consistent with this session's earlier combined-mode
  baselines, no regression on the healthy path from the periodic probe

## 4. Lease-flapping / stale-reaper fix (confirmed root cause, fixed)

- [x] 4.1 Root cause confirmed: two independent bugs (TOCTOU in
  `liveness::renew`/`membership::renew`; `MAX_SUBSCRIBE_FAILURES` budget
  far shorter than a real outage) — see `design.md` D4. Response
  misattribution (QA's original hypothesis) investigated and refuted by
  reading `redis-rs` source directly.
- [x] 4.2 `broker.rs`: both `renew` functions now use a single atomic Lua
  script (`EXISTS` + `SET EX` in one round trip), same pattern as
  `ownership::RENEW_SCRIPT`/`rating::STORE_SCRIPT`
- [x] 4.2b `logic_handler.rs`: `MAX_SUBSCRIBE_FAILURES` raised from 10 to
  150 (~5 minutes at the existing backoff shape)
- [x] 4.2c Bug C (found while verifying, not in the original report):
  `transport_handler.rs`'s `spawn_logic_watch` had no debounce before
  acting on a single `logic_node_alive() == false`, unlike the symmetric
  watcher on the other side. Added `LOGIC_NODE_MISS_THRESHOLD = 2`,
  mirroring `logic_handler.rs`'s existing `NODE_MISS_THRESHOLD` pattern.
- [x] 4.3 New test `survives_a_full_outage_past_the_lease_ttl`
  (`tests/split_e2e.rs`): real `ws-server` logic AND transport subprocesses
  against a real, separately spawned `redis-server`, outage held 35s (past
  the 30s TTL) — passed: neither tier exited early, both re-registered
  after recovery, transport's own warn log shows at most one "lease had
  lapsed" line for the whole outage (not a repeated flap)
- [x] 4.4 The 11-minute scenario doesn't need manual re-running: the new
  automated test exercises the same mechanism (a real expired key after a
  real outage) at a bounded, CI-appropriate duration; the root cause (not a
  duration-dependent effect) is what was fixed

## 5. Test-harness correctness (test-only, no spec delta)

- [x] 5.1 `LogicState`: internal background tasks (lane workers, lease
  renewal, room-ownership keep-alive, liveness watcher) now spawn into a
  tracked `JoinSet`; `abort_background_tasks()` stops all of them
- [x] 5.1b `TransportState`: same treatment for `spawn_dispatcher`'s
  per-shard readers, `spawn_logic_watch`, and the new
  `spawn_publish_health_check` (previously unabortable in principle)
- [x] 5.2 `tests/split_e2e.rs`: `Harness`/`CombinedHarness`/
  `MultiLogicHarness::drop` now call `abort_background_tasks()` (and
  `stop_liveness()`) for every logic/transport instance they created, not
  just the outer `run()` task handle
- [x] 5.3 Renumbered the four duplicate `PORT_*` constants
  (`PORT_RECLAIM`/`PORT_OWNERLOSS_1`/`PORT_OWNERLOSS_2`/
  `PORT_LOST_MEMBERSHIP` → 18867/18869/18871/18873) to unique values;
  confirmed no other duplicates exist across all `PORT_*` constants
- [x] 5.4 Ran `split_e2e --ignored --test-threads=1` repeatedly: improved
  from QA's 42/47 → 46/47 → 47/47 to 5-for-5 consecutive clean 48/48 runs
  after the Bug C fix (one isolated flake, `same_node_pair_lease_loss_
  keeps_the_room`, seen once before that fix and traced directly to it)

## 6. Second QA round: 3 bugs found in this change's own fixes (fixed)

- [x] 6.1 `MAX_SUBSCRIBE_FAILURES` 150→600 (~20 min, real margin past both
  the TTL and the ~11 minute incident) — QA's own "wait ≥5 min" test had
  found the old 150-budget dying at exactly 5 minutes, no margin
- [x] 6.2 `broker.rs`: `ping_publish_shard` now drops the cached connection
  on a failed probe (same pattern `publish_with_retry` already used) —
  re-verified: `CreateRoom` after a Redis restart, with no client
  connecting first, now succeeds in 17ms (previously hung until some
  client's own publish happened to repair the connection)
- [x] 6.3 `transport_handler.rs`: `PUBLISH_HEALTH_CHECK_INTERVAL`/
  `PUBLISH_HEALTH_PROBE_TIMEOUT` 2s/3s → 500ms/1.5s, shrinking (not
  eliminating — architecturally can't without reintroducing the D1 cost)
  the detection-window race QA found. Re-verified directly: the exact race
  (stop Redis and send in the same tick) still hangs, confirmed as an
  accepted trade-off — but resolves in ~211ms once Redis actually responds
  again, not indefinitely
- [x] 6.4 3.3 (close reason ambiguity) deferred — needs a wire-protocol
  change for a low-severity issue, documented in `design.md` D5
- [x] 6.5 Full regression: `cargo test --lib --tests` green;
  `split_e2e --ignored --test-threads=1` run 3x after these fixes,
  confirmed still deterministic (48/48 every time)

## 7. Third QA round: real recovery-time bottleneck found and fixed

- [x] 7.1 QA's machine-readable re-verification confirmed all 3 section-6
  fixes with direct re-measurement (self-heal 6/6, budget death at
  1198-1200s matching the ~20min target, race window 1588-1862ms matching
  the configured 500ms/1.5s exactly) — all PASS, every claimed number held up
- [x] 7.2 GAP-QA-R5-A (severity MEDIA, found via attribution, not a symptom
  guess): `logic_handler.rs`'s `ptc.logic_node.*` lease-renewal loop used a
  fixed 10s `tokio::time::interval` with no backoff on failure, so recovery
  after a real Redis restart took 10-18s (dominated by this, not by the
  publish-health probe or the control-shard timeout). Fixed: `sleep`-based
  loop, success → steady 10s, failure → `reconnect_backoff` (200ms→2s cap),
  same pattern already used by the subscribe-retry loop in the same file
- [x] 7.3 Applied the identical fix proactively to the symmetric
  `transport_handler.rs::spawn_liveness` loop (`ptc.node.*`) — same bug
  shape, not proven to matter for this specific symptom but left unfixed
  would be a known, identical, untreated twin
- [x] 7.4 OBS-QA-R5-B (severity INFO) documented as a deliberately deferred
  observability gap, not implemented
- [x] 7.5 Re-verified directly: real Redis kill + 4s outage + restart,
  `create_room` probed repeatedly — recovered in 1247ms (down from 10-18s)
- [x] 7.6 Full regression: `cargo build --lib`/`cargo test --lib --tests`
  green; `split_e2e --ignored --test-threads=1` run 3x, still deterministic

## 8. Fourth QA round (R6): general performance + a confirmed setup-race fix

- [x] 8.1 D5/D6 fixes re-confirmed (reconnect budget, probe self-heal,
  detection window, lease backoff) — all PASS, see `design.md` D7 for the
  nuance QA found in how the lease-backoff fix actually works
- [x] 8.2 GAP-QA-R6-D fixed: `broker.rs`'s `claim_room`/`get_room_owner`
  now retry (reusing `PUBLISH_ATTEMPTS`/`PUBLISH_RETRY_DELAY`) on a
  control-shard timeout under burst load, with `claim_room`'s retry
  correctly handling the "our own earlier claim actually landed" case
  before reporting failure
- [x] 8.3 GAP-QA-R6-E fixed: port-range warning comment in
  `tests/split_e2e.rs`
- [x] 8.4 GAP-QA-R6-A (fabric sharding inconclusive) and OBS-QA-R6-F (high
  variance) are out of this change's scope — tracked in
  `shard-fabric/tasks.md` (2.3) and `docs/support/qa-performance-findings.md`
  respectively, not duplicated here
- [x] 8.5 Full regression: `cargo build --lib`/`cargo test --lib --tests`
  green; `split_e2e --ignored --test-threads=1` run 3x after the retry fix
- [ ] 8.6 Re-run QA's 3-node/3-shard scaling load (2000 games/node, pace
  25ms) to confirm the 0-8% setup-failure rate drops to ~0% — the real
  confirmation at the scale that found the bug, not just the local gate

## Gate before calling multi-logic-node operation production-ready

- [x] All of sections 1-8 complete and verified except 8.6 (pending a QA
  re-run at load; the fix itself is implemented, tested for regressions,
  and traced to the exact confirmed mechanism)
- [ ] Archive this change and update the `server`/`realtime-transport`
  living specs

## Workflow follow-up

- All QA findings across both rounds (rating consistency, bounded
  control-shard latency, inbound publish health + its own 2 implementation
  bugs, the lease-flapping bugs + the budget-margin bug, and test-harness
  determinism) are implemented and verified, each with direct re-measurement
  against the actual behavior, not just code review. Ready to archive once
  the user reviews; nothing is blocking. 3.3 (close reason ambiguity) is a
  known, documented, deliberately deferred low-severity gap, not an
  oversight.
