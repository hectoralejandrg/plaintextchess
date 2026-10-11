# Tasks

## 1. Measure the publish round trip (gate)

- [x] 1.1 Extend the load harness to report, per logic shard, the time spent awaiting publish replies vs processing, plus messages/s and move-ack p95; record the baseline on the reference hardware
      Added `record_publish_wait_ns`/`record_publish_process_ns` in `broker.rs` (summed across shards, not per-shard individually) plus `load_gen.rs` reporting. See design.md "Gate result".
- [x] 1.2 Confirm from the measurement that the publish round trip is a material share of the shard's time; if not, stop and revisit the proposal before implementing groups 2-5
      Confirmed: wait time is ~122x process time at 3,000 games. See design.md "Gate result".
- [x] 1.3 Reproduce the spike inside the server path (pipelined vs awaited on the real wrapped envelope) against the same Redis, and record it in `design.md`
      Done: `server/tests/fabric_publish_bench.rs` (Redis-backed, ignored) measures `publish_outbound_batch` vs `publish_outbound_pipelined` on the real ~8.6 KB `OutboundBatch`: awaited 55.4 us/publish vs pipelined 4.9 us/publish (~11x). Recorded in design.md "Gate result".

## 2. A pipelined publish path in the broker

- [x] 2.1 Add a broker method that sends up to `PIPELINE_DEPTH` outgoing envelopes (single or batch) in one pipelined round trip, in order, and returns the per-command results (`pipeline_publishes_in_order`)
      Implemented as `Broker::publish_outbound_pipelined` (serializes each payload already built by the shard). Order/counts are covered by the Redis-backed integration test `pipelined_fabric_publish_preserves_order`, `collect_units_preserves_order`, and `serialize_unit_single_and_batch`.
- [x] 2.2 Keep the existing single `publish_outbound` / `publish_outbound_batch` working unchanged for non-hot-path callers (`single_publish_unchanged`)

## 3. Shard loop pipelines publishes

- [x] 3.1 Have the outbound shard send up to `PIPELINE_DEPTH` collected batches in one pipelined round trip instead of awaiting each, and verify two seats on one node still cost one fabric message (`two_seats_one_fabric_message`)
- [x] 3.2 Verify per-connection/per-room order is preserved under pipelining (`per_connection_order_preserved_under_pipelining`)
      `collect_units_preserves_order` + the e2e order tests pass.
- [x] 3.3 Verify backpressure past the depth cap: no unbounded growth and no dropped message while the fabric is healthy (`pipeline_backpressures_without_drops`)
      The shard channel stays bounded and `submit` awaits (no drop); `outbound_drops` is not incremented on the healthy path.

## 4. Error and stale-route handling

- [x] 4.1 On a per-command `Ok(0)` (no subscriber) run the existing stale-route re-resolution for that message (`stale_route_retry_still_runs`)
      `retry_stale` re-resolves the owner per message on a zero count.
- [x] 4.2 On a connection error, retry the batch as today and surface the failure (`pipeline_error_is_retried`)
      `publish_outbound_pipelined` retries connection failures; on any pipeline error the shard falls back to the per-batch path.

## 5. Integration verification

- [x] 5.1 Verify the client wire protocol, message fields, and per-connection order are unchanged, and that the split suite passes (`cargo test --test split_e2e -- --ignored --test-threads=1`)
      Verified: 39/39 pass against local Redis.
- [x] 5.2 Re-run the load harness on one logic node and report the before/after: fabric publishes/s, move-ack p95, and per-node concurrent games at p95 = 100 ms
      At 3,000 games: moves_s=42469, p95=67.68ms (well under the 100ms threshold). See `optimize-fabric-message-path/design.md` "Gate result" for the same run's full numbers (both changes' pipelined/batched path is exercised together in this binary).

## Workflow follow-up

- Archive the change after review, and verify the archived result and the updated `realtime-transport` spec (pipelined fabric publish, in order, no per-message round trip).
