# Tasks

## 1. Logic outbound publish

- [x] 1.1 Publish over a dedicated no-timeout `MultiplexedConnection` (lazy reconnect) instead of the per-command-timer `ConnectionManager`, and verify publishes still succeed and broker loss is still surfaced (`logic_survives_a_broker_restart`, `publish_retries_after_the_connection_dies`)
- [x] 1.2 Verify with the profiling harness that the timer-wheel/`__psynch_mutexwait` share of the logic hot path drops: `TimerEntry::init` fell to 5 samples and `__psynch_mutexwait` to ~650 (from the dominant ~21% chain), at unchanged throughput (~22.6k moves/s, p95 26 ms @ M=2000)

## 2. Transport affinity map

- [x] 2.1 Stop taking a global lock to update a connection's logic-node affinity on every frame - shard the map by `hash(connection_id)` or write only when the value changes - and verify routing is unchanged (`two_nodes_route_outbound_to_owner`, `cross_node_inbound_and_outbound`, `stale_membership_node_is_skipped`)
- [x] 2.2 Verify with the profiling harness that the affinity-map mutex no longer dominates the transport hot path: `__psynch_mutexwait` fell to ~460 samples with `__sendto` (socket writes) now the dominant non-idle cost

## 3. Integration verification

- [x] 3.1 Verify `cargo test` passes for the server crates and the split suite (`cargo test --test split_e2e -- --ignored --test-threads=1`) with no behavior change
- [x] 3.2 Re-run the load harness on one logic node: no throughput regression (≈22.6k moves/s, p95 26 ms @ M=2000, 0 errors); the gain is contention reduction (logic timer/mutex share down, transport mutex down), not a higher ceiling on this already-saturated host

## Workflow follow-up

- Archive the change after review, and verify the archived result and the updated `logic-throughput` and `transport-scale-out` specs.
