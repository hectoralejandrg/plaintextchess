# Design

## Context

See `proposal.md` - Why. `LogicState` (`server/src/infrastructure/
logic_handler.rs`) routes room-scoped messages across `room_lane_count`
workers (`hash(room_code) % K`) and control-plane messages (`CreateRoom`,
`JoinRoom`, `Leave`, `Resign`, auth) across `control_lane_count` workers
(`hash(connection_id) % C`), both configurable via `LOGIC_LANES`/
`LOGIC_CONTROL_LANES` env vars, both defaulting to 1
(`server/src/infrastructure/config.rs`). `TIER=all` (the monolith) does not
go through `LogicState`/lanes at all — this is purely a `TIER=logic`/
`TIER=combined` concern.

## Goals / Non-Goals

**Goals:**
- Confirm (done) whether single-lane serialization is a real contributor to
  setup failures under multi-node burst load.
- Determine, with a measurement that can actually be trusted, what lane
  count (or what alternative mechanism) fixes it without shifting the
  bottleneck somewhere worse (the shared control-shard Redis connection).

**Non-Goals:**
- Changing the monolith (`TIER=all`) — not affected, not in scope.
- Re-litigating `fix-multi-node-rating-consistency`'s `claim_room`/
  `get_room_owner` retry fix — that fix is real, confirmed, and orthogonal;
  this investigation is about a *second*, independent contributor to the
  same symptom (QA's R6-D).

## What was tried and what it showed

| Config | Setup failure rate | Note |
|---|---|---|
| 1 lane (today's default) | ~37% | Clean single run, 3 real logic nodes, 6000-game burst |
| 8 lanes | ~5.6% | Clean single run, same scenario |
| 16 lanes | ~19.4% | Clean single run, same scenario — **worse than 8** |
| 16 lanes, repeated | 19%, then 95%, 99%, 86% (progressively worse) | Same long-running processes reused across repeats with no drain time — confounded by accumulating in-flight state, not a lane-count effect. Methodology error, not a finding. |
| 8 lanes, 3 fresh replicas (full process restart each) | 37.9%, 100% (0/6000), 84.3% | Host too contaminated by this session's own extensive prior load-testing (ephemeral port/TIME_WAIT exhaustion) to produce trustworthy numbers at all. Abandoned. |

**Conclusion so far**: the single-lane default is a confirmed, real
problem (37% failure is unambiguous and reproduces cleanly). The *fix* is
not confirmed — 8 looked better than 1 and 16 in isolated single runs, but
single runs are exactly what `docs/support/qa-performance-findings.md`
already warned aren't interpretable (±33-50% replica variance observed
elsewhere in this same investigation). Nothing here should be read as "8 is
the right number" — it's the only value that happened to look good once,
on a host that was later shown to produce unreliable results under repeated
load.

## What a trustworthy measurement needs

1. **Fresh process state per replica.** Kill and fully restart every Redis
   shard, logic node, and transport node between replicas — don't reuse
   long-running processes across repeated bursts (confirmed cause of the
   "progressively worse" false signal above).
2. **Real drain time between replicas**, not a fixed short sleep — poll for
   the previous replica's ports/processes to actually be gone (`lsof`/
   `ps`) before starting the next, since a few seconds was not enough on a
   loaded macOS host.
3. **A host not already saturated by other testing.** This investigation
   ran on a machine that had, by this point, run dozens of ad-hoc Redis/
   server instances over several hours in the same session — a measurement
   this sensitive to timing needs a clean host, ideally the same dedicated
   environment `shard-fabric` task 2.3 already needs for its own
   single-host-confound problem.
4. **>= 3 replicas per candidate value**, matching QA's own stated bar
   (`docs/support/qa-performance-findings.md`: "Run-to-run variance is
   high... >=3 replicas are required for a factor to be interpretable").
5. **Instrument where the time actually goes** before picking a number:
   `control_lane_metrics`/`room_lane_metrics` in `/healthz` already report
   per-lane `avg_us`/`max_us`/`processed` — use these to see whether a
   candidate lane count actually relieves the *local* queue (it should) or
   just moves the wait onto the control-shard round trip (watch
   `fabric_publish_shard_time`/any future control-shard latency metric) —
   that is the mechanism that would explain 16 losing to 8.

## Risks / Trade-offs

- [More lanes = more concurrent load on the shared control shard] - the
  leading hypothesis for why 16 lost to 8; not yet confirmed, needs the
  instrumentation above before trusting any specific number.
- [Picking a number from noisy data] - already happened once in this
  investigation (the reverted attempt to default to 16); the fix in
  `tasks.md` is explicitly "measure properly first," not "pick a number
  and ship it."

## Open Questions

- What is the actual right default (if a static one even makes sense), or
  should lane count scale with the number of live logic/transport nodes
  somehow instead of being a fixed constant?
- Is there a point where control-shard contention (not local lane
  serialization) becomes the dominant cost regardless of lane count, and if
  so, does that argue for sharding the control plane after all (the same
  open question `shard-fabric` left unresolved)?
