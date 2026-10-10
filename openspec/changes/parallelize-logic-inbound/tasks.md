# Tasks

## 1. Configuration and scaffolding

- [x] 1.1 Add the inbound lane count (and the control-lane count) to the server config with a safe default, and verify unit tests: unset uses the default and zero/invalid is rejected at startup (`logic_lanes_defaults_to_one`, `logic_lanes_zero_is_rejected`, `logic_lanes_invalid_is_rejected`)
- [x] 1.2 Document the lane configuration and its safe default in `docs/tiered-architecture.md` and verify the documented run command works as written
- [x] 1.3 Restore graceful shutdown of the logic tier on SIGTERM (a refactor had turned the signal handler into a no-op), and verify the shutdown contract with `graceful_shutdown_releases_ownership_and_membership` plus the `ws-server` signal wiring

## 2. Sharded inbound lane

- [x] 2.1 Fan out a single broker reader to K bounded lanes keyed by `hash(room_code) % K` with one worker per lane, and verify per-room ordering under concurrent rooms (`per_room_order_preserved_across_lanes`) plus deterministic lane assignment (unit tests `room_lane_assignment_is_deterministic`, `rooms_spread_across_lanes`)
- [x] 2.2 Keep the reader routing-only so independent rooms on different lanes progress concurrently, and verify it with `per_room_order_preserved_across_lanes` (three rooms advanced in lockstep across four lanes)
- [x] 2.3 Drop the `Arc::get_mut` requirement so `run()` can be called on a shared `Arc` (needed to keep a handle for shutdown), taking the lane receivers once from a `Mutex<Option<...>>`, and verify the split suite still starts the tier
- [x] 2.4 Bound lane capacity (`LANE_CAPACITY`) so a full lane applies backpressure without silent drops, and verify `lane_capacity_is_bounded`
- [x] 2.5 Document the lane model, the preserved per-room ordering, and the single-reader backpressure limitation in `docs/tiered-architecture.md`, and verify the documented statements match the tests
- [x] 2.6 Forward a frame whose target logic node is known with a single publish, consulting the fabric node list only on the fallback path, and verify the routing fallback tests still pass (`stale_membership_node_is_skipped`, `transport_closes_a_client_when_no_logic_is_subscribed`)

## 3. Connection map partitioning

- [x] 3.1 Partition the shared inbound `connections` map (16 shards by `hash(connection_id)`) so the reader's per-message lane lookup and the lane workers do not serialize on one global lock, and verify `lanes_do_not_serialize_on_connection_map`
- [x] 3.2 Verify the existing split and rooms integration suites still pass (`cargo test --test split_e2e -- --ignored --test-threads=1` and `cargo test --test rooms`)

## 4. Control-plane isolation

- [x] 4.1 Verify create-before-follow-up ordering per connection through the connection-keyed control pool, and verify `connection_control_order_preserved` (a create-then-create yields one room and an `already_in_room` error)
- [x] 4.2 Verify control-plane work and room-scoped work use separate lane pools so a slow auth cannot occupy the worker that acks a move, and verify `control_and_move_messages_use_separate_pools`
- [x] 4.3 Verify the existing auth and rooms integration suites pass unchanged (`cargo test` → 16 auth + 24 rooms green)

## 5. Lease renewal hardening

- [x] 5.1 Renew owned-room ownership leases in a single pipelined round trip that reloads and retries on `NOSCRIPT`, and verify `renew_pipelined_renews_all_and_survives_script_flush`
- [x] 5.2 Assert at startup that the lease TTL safely exceeds the sweep interval plus worst-case sweep, and verify the fail-fast test (`lease_ttl_exceeds_renewal_sweep`)

## 6. Observability

- [x] 6.1 Expose per-lane queue depth and processing latency plus an inbound move-dispatch latency on the logic `/healthz`, and verify `lane_metrics_are_exposed`

## 7. Integration verification

- [x] 7.1 Re-run the load harness: the transport hot-path fix raised the per-node ceiling from ~950-1250 to ~2000-2700 concurrent games at move-ack p95 = 100 ms (peak ~23k moves/s at M=2000), with 0 errors and ~0 spurious re-claims. The sharded lanes preserve per-room ordering but do not add capacity (K=4 == K=1); the remaining levers are delegated to `balance-room-placement` and `reduce-fabric-contention`
- [x] 7.2 Verify `cargo test` passes for all server crates and no wire-protocol or client change was introduced: 166 lib + 16 auth + 8 clock + 7 integration + 3 persistence + 24 rooms + 2 split_config + 37 split_e2e all green, and `protocol.rs`/`core/`/`ios/`/`android/` are unchanged (git)
- [x] 7.3 Update `docs/tiered-architecture.md` with the achieved per-node capacity, the fabric hot path, the lane model, and the control-plane isolation, and verify the documented commands run as written

## Workflow follow-up

- Archive the change after review, and verify the archived result and the updated main specs (`logic-throughput` created, `room-routing` updated).
