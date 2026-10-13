# QA performance findings

Performance/quality findings from QA rounds on the split realtime architecture.
Each round records the host, the head under test, the method and its caveats,
the verdicts, and the open findings. Companion to `qa-client-e2e-findings.md`
(which covers client end-to-end behaviour).

## Reading these numbers

- The **per-node ceiling is a property of the host** (CPU), not of the fabric:
  the transport's per-move egress write is the dominator (~50-60 us/move). Redis
  in these runs sits near 0.3 of a core.
- **All nodes were co-located on one host** in every round so far. That caps the
  aggregate at the host's ~`50-58k moves/s` regardless of how many logic /
  transport / Redis processes run. **Horizontal scaling cannot be demonstrated
  while every node shares one machine** - it needs nodes on separate hosts.
- Run-to-run variance is high (up to ±50% between replicas with identical load);
  **>=3 replicas** are required for a factor to be interpretable.

---

## Round 6 (2026-10-12)

- Head: `e234d8e` (working tree dirty: 10 modified files + an untracked
  `fix-multi-node-rating-consistency` change; no repo files modified by QA).
- Host: Apple M2 Max, 12 cores, macOS 26.6.2 (not idle; `caffeinate -is`).
- Deviations: own Redis on 16379/16380/16381 (6379 untouched); split-e2e ports
  moved to 18910+/18920+ (see R6-E); a temporary `LOADGEN_PREFIX` patch to
  `load_gen.rs` (restored, md5 identical).

### Verdicts

| Section | Verdict |
|---|---|
| A1 gate | PASS - 257 lib/integration + 3x 48/48 split_e2e, 0 warnings |
| A2 logic RTO | PASS with reserve - 0.46-3.4 s (was 10-18 s); 1 of 4 above 2-3 s |
| A3 transport RTO | PASS - keys back in <=40 ms; exactly 1 lapse per node; SIGSTOP does not disconnect |
| A4 prior fixes | PASS |
| B baseline by mode | PASS - combined 58.7k > all 57.5k > split 47.6k moves/s |
| C scaling | **FAIL** - objective >1.5x not met; 3 shards = 0.85x vs 1 shard (but see caveat) |
| D stability | PASS - 12 runs, 0 failures, 0 panics, bounded channel depth |
| E cleanup | PASS |

### Key numbers

- Modes (saturating load, 2000 games, pace 25 ms): `combined` 58,700 moves/s
  (p95 44 ms), `all` 57,484 (p95 40 ms), `split` 47,615 (p95 63 ms, ~2x p99).
  **Leaving the process (the fabric crossing) costs ~19% throughput.**
- Scaling (weak, 2000 games/node): `1 node/1 shard` ~43.6k; `3 nodes/1 shard`
  ~55.2k (1.27x); `3 nodes/3 shards` ~47.1k (1.08x) - and **3 shards were slower
  than 1** (0.85x), within a ±33% replica spread.
- Control keyspace distribution with 3 shards: shard 0 held all 6 keys
  (`ptc.logic_node.*`, `ptc.node.*`); shards 2-3 held 0. Only pub/sub channels
  spread; **the control plane stays on one instance**.

### Findings

- **R6-A (MEDIUM) - Fabric sharding shows no measurable throughput gain; the
  >1.5x objective is not met.** Factor 1.08x; 3 shards 0.85x. Root evidence:
  the control keyspace is centralized on shard 0, and the split itself already
  costs ~19% before any scaling.
  **Caveat (added on review):** the test ran every node on one host, so the host
  (~50-58k moves/s) is the shared cap; a conclusion about sharding/horizontal
  scaling is **not reachable from a single host** and needs separate hosts.
- **R6-B (MEDIUM) - logic-side RTO exceeds 2-3 s in 1 of 4 outages** (2 s outage
  -> 3448 ms upper bound; others 0.46/1.5/2.2 s).
- **R6-C (INFO) - the adaptive lease backoff is masked by the ConnectionManager.**
  Only 1-2 renewal warnings per outage regardless of duration (a working loop
  backoff would emit ~8-9 warnings in 15 s). The recovery is the underlying
  socket reconnect, not the loop's backoff formula; hence the non-monotonic RTO.
- **R6-D (MEDIUM) - intermittent setup failures (0-8%) in 3-node runs, even with
  unique player_ids** (plies=0, they die in seating). Consistent with a seating /
  single-control-lane bottleneck.
- **R6-E (LOW) - `server/tests/split_e2e.rs` hardcodes ports (18765..18873)**;
  a developer stack on those ports makes the gate fail with `AddrInUse`. Prefer
  env-parameterised ports.
- **R6-F (INFO) - high replica variance** (baseline varied -38%; scaling ±33%);
  >=3 replicas required.

### Recommendations

1. Do not conclude on fabric sharding from single-host runs. Re-run the scaling
   matrix with logic and transport on **separate hosts** (Redis shards can stay
   local to their logic node).
2. Sharding a non-bottleneck does not help; if fabric work continues, address the
   **control-plane centralization** (control shard) and the **split cost** first.
3. Investigate R6-D (seating) as a separate, cheaper win.
4. Treat R6-C as a simplification opportunity: the adaptive backoff may be dead
   code if the ConnectionManager always recovers first.

---

## Monolith registry-lock profile (2026-10-13) — not split-tier

Profiles the *monolith's* (`TIER=all`) setup ceiling, adjacent to R6-D.

- **Question** (`shard-room-registry-locks`): are the three process-global
  `Mutex` registries (`rooms` / `players` / `account_rooms`) the cause of the
  setup failures measured at ~10k concurrent room creations (baseline 35.8 %
  on a 2 vCPU / 2GB container)?
- **Method**: temporary wait+hold nanosecond counters on every `.lock()` in
  `check_room_limit`/`unique_code`/`create_room_internal`'s insert/
  `track_identity`, exposed at `/healthz` as `registry_lock_ns`. The 2-vCPU
  mechanism (one worker thread per core) was reproduced with
  `TOKIO_WORKER_THREADS=2` on the M2 Max host — *not* a 2-CPU cgroup, so this
  is a mechanism-faithful, not resource-identical, repro. `load_gen.rs
  --games 10000 --pace-ms 8000 --ramp-ms 1000`; `DATABASE_URL=sqlite:/tmp/...`.
- **Result**: 10,000 games → 7,531 ok / **2,469 failed (24.7 %)** — a real
  breaking point. Total lock-wait over the entire run: **8.82 ms**
  (`rooms` 2.32, `players` 6.50, `account_rooms` 0 — the guest-only load never
  exercises the account map). That is **<=0.03 %** of the >=30 s window in
  which the failed games timed out (~0.1 µs per acquisition: uncontended).
  Meanwhile `GET /healthz` was unreachable (`curl` failed) during the burst,
  and the move phase ran at only ~940 moves/s with ack p95 = 493 ms.
- **Verdict**: **refuted.** The registry mutexes are not the bottleneck;
  sharding them would not move the failure rate. The evidence points at
  event-loop / scheduler starvation in the accept/setup path under raw task +
  connection count (same family as R6-D). The instrumentation was reverted and
  `shard-room-registry-locks` is closed with this negative result.
