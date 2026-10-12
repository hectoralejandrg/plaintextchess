# Tasks

## 1. Shard the fabric (implemented)

- [x] 1.1 Config accepts `REDIS_URLS` (comma-separated) with `REDIS_URL` as a 1-shard fallback; a non-`all` tier refuses to start without a URL (`config.rs` tests: unset/singular/plural)
- [x] 1.2 The broker connects to N shards (per-shard client/manager/publish connection), exposes `n_shards`/`shard_for`, and a control shard for low-volume lifecycle work (`broker.rs`)
- [x] 1.3 Publish paths carry the shard; `publish_outbound` routes by connection key, and retry/reconnect is scoped per shard (`publish_with_retry(shard, ...)`)
- [x] 1.4 The outbound batcher spawns one `BatchShard` per Redis shard, each bound to that shard (`logic_handler.rs`); the `run_*` functions wire `Broker::connect(&config.redis_urls, ...)`

## 2. Measure the shard payoff (gate) — in progress

- [x] 2.1 Run the load harness at 1 shard vs N shards on the same host (same games/pace), reporting move-ack p95 and per-node concurrent games at p95 = 100 ms; record the baseline.
      Done (QA round 6; see `docs/support/qa-performance-findings.md`): 1 node/1 shard ~43.6k moves/s; 3 nodes/1 shard ~55.2k (1.27x); 3 nodes/3 shards ~47.1k (1.08x). 3 shards were 0.85x vs 1, within a ±33% replica spread.
- [x] 2.2 Confirm from the measurement whether sharding lifts the ceiling/scaling; if not, stop and revisit (the socket-write floor may still bind).
      Result: **no measurable gain**, and the control keyspace (node leases/registry) remains entirely on shard 0. Revisit triggered (2.3).
- [ ] 2.3 Re-measure across SEPARATE HOSTS. The round-6 matrix co-locates every node on one host, so the host itself (~50-58k moves/s) is the shared cap and horizontal scaling cannot be demonstrated. Put logic/transport on distinct hosts and repeat before concluding on sharding.

## 3. Integration verification

- [ ] 3.1 Run the split e2e suite against a multi-shard configuration (`REDIS_URLS` with ≥2 instances)
- [ ] 3.2 Verify per-room/per-connection order is preserved across shards (`order_preserved_across_shards`)

## Workflow follow-up

- Archive the change after review, and verify the archived result and the updated `realtime-transport` spec (sharded fabric, order preserved per room).
