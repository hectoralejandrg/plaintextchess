# Tasks

## 1. Logic outbound publish

- [ ] 1.1 Remove the per-message timer from the outbound publish path (configure the Redis connection so it does not arm a per-command response timeout, or publish over a dedicated no-timeout path), and verify publishes still succeed and broker loss is still surfaced (`logic_survives_a_broker_restart`, `publish_retries_after_the_connection_dies`)
- [ ] 1.2 Verify with the profiling harness that the timer-wheel/`__psynch_mutexwait` share of the logic hot path drops and that sustained per-node throughput rises (`bucket: logic, category: contention-mutex` decreases)

## 2. Transport affinity map

- [ ] 2.1 Stop taking a global lock to update a connection's logic-node affinity on every frame - shard the map by `hash(connection_id)` or write only when the value changes - and verify routing is unchanged (`two_nodes_route_outbound_to_owner`, `cross_node_inbound_and_outbound`, `stale_membership_node_is_skipped`)
- [ ] 2.2 Verify with the profiling harness that the affinity-map mutex no longer dominates the transport hot path (`bucket: transport, leaf: set_conn_logic_node -> Mutex::lock` decreases)

## 3. Integration verification

- [ ] 3.1 Verify `cargo test` passes for the server crates and the split suite (`cargo test --test split_e2e -- --ignored --test-threads=1`) with no behavior change
- [ ] 3.2 Re-run the load harness on one logic node and report the before/after: per-node concurrent games at move-ack p95 = 100 ms and sustained moves/s, plus the profiling shares before/after

## Workflow follow-up

- Archive the change after review, and verify the archived result and the updated `logic-throughput` and `transport-scale-out` specs.
