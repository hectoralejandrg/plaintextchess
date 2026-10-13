# Tasks

## 1. Confirm the problem (done)

- [x] 1.1 Reproduce QA's R6-D scenario (3 logic nodes, 3 Redis shards, 6000-
  game concurrent burst) with `server/examples/load_gen.rs`
- [x] 1.2 Confirm the single-lane default (`LOGIC_LANES=1`,
  `LOGIC_CONTROL_LANES=1`) causes ~37% setup failures, with `/healthz`
  showing per-message control-lane latency up to ~845ms under the burst

## 2. Find the right fix (not done — reverted a premature attempt)

- [x] 2.1 Tried raising both defaults to 16 (matching `OUTBOX_SHARDS`/
  `CONN_LOGIC_SHARDS`'s convention) — **reverted**. A single clean run
  measured 19.4% failures, worse than 8 lanes' 5.6% in an earlier single
  run. Repeated/same-host attempts to confirm either number were too
  confounded (process-reuse accumulation, then host-level port exhaustion
  from this session's own extensive testing) to trust. See `design.md` for
  the full account and what went wrong with each attempt.
- [ ] 2.2 Set up a clean, dedicated measurement environment (fresh host or
  at minimum a host with nothing else running, full process restarts
  between replicas, real drain-time checks) — see `design.md`'s "what a
  trustworthy measurement needs"
- [ ] 2.3 Run >= 3 replicas each for a real candidate set (e.g. 1, 4, 8,
  16, 32), using the exact R6-D repro scenario, recording per-lane
  `avg_us`/`max_us` from `/healthz` alongside the failure rate for each
  - [ ] 2.3a If failures still occur with the per-lane queue clearly not
    the bottleneck (low `avg_us`), check whether the control-shard retry
    fix (`fix-multi-node-rating-consistency`) is itself adequate at that
    concurrency level, or needs its own retry/backoff tuning
- [ ] 2.4 Pick a default (or an alternative mechanism — e.g. scale lanes
  with detected CPU count, or leave 1 as the default with a startup warning
  for `TIER=logic`/`combined` instead of a blanket default change) based on
  that data, not on a single noisy run

## 3. Ship

- [ ] 3.1 Implement the chosen default/mechanism in
  `server/src/infrastructure/config.rs`
- [ ] 3.2 Full regression: `cargo test --lib --tests` and
  `split_e2e --ignored --test-threads=1` (x3, isolated — nothing else
  running on the host at the same time, learned the hard way this round)
- [ ] 3.3 Re-run the R6-D repro scenario to confirm the chosen value
  actually resolves the setup-failure rate at the scale QA originally found
  it

## Workflow follow-up

- Do not archive until section 2 produces a trustworthy number and section
  3 ships it. This change intentionally stops at "investigated, root cause
  confirmed, fix deferred pending real data" rather than shipping an
  unverified default.
