# Proposal

## Why

QA's round-6 performance testing (`docs/support/qa-performance-findings.md`,
finding R6-D) found 0-8% of games failing to even seat (`plies=0`) under a
multi-node burst load (3 logic nodes, thousands of concurrent `CreateRoom`s).
Investigating that finding (see `fix-multi-node-rating-consistency`'s
`design.md` D7) traced a confirmed, real contributing cause to
`COMMAND_RESPONSE_TIMEOUT` on the control-shard connection (fixed there, by
retrying `claim_room`/`get_room_owner`) — but reproducing the exact scenario
surfaced a second, independent contributing factor that fix does not touch:
`LOGIC_LANES`/`LOGIC_CONTROL_LANES` default to 1, meaning every
`CreateRoom`/`JoinRoom` for a `TIER=logic`/`TIER=combined` process
serializes through a single worker. A reproduction of the exact QA scenario
measured ~37% setup failures at this default, caused by that one lane's
queue backing up (observed per-message latency up to ~845ms) under a burst
of thousands of concurrent room creations.

## What this proposal is (and isn't)

This is **not** a completed fix — it's an investigation that found a real,
confirmed problem (the single-lane serialization bottleneck) but could not
responsibly conclude what the right fix is, because the obvious one (raise
the default) did not hold up:

- A single clean comparison run suggested 8 lanes cut failures from 37% to
  5.6%.
- A more careful re-test of 16 lanes (to match this codebase's existing
  sharding-width convention, `OUTBOX_SHARDS`/`CONN_LOGIC_SHARDS` = 16)
  measured 19.4% failures on a clean run — **worse** than 8, not better.
  Plausible mechanism (not yet confirmed): more local lanes means more
  concurrent requests reach the *shared* control-shard Redis connection at
  once, shifting load there instead of relieving it.
- A third attempt to pin this down with repeated same-host replicas was too
  contaminated by the test machine's own accumulated state (TIME_WAIT
  socket exhaustion from extensive same-session load testing; one replica
  measured 0/6000 successes, an obvious setup artifact) to trust at all.

Raising `DEFAULT_LOGIC_LANES`/`DEFAULT_LOGIC_CONTROL_LANES` was reverted
back to 1 (the known-safe value) rather than ship a change whose own
verification contradicted itself. See `design.md` for what a trustworthy
measurement would need.

## What Changes

Nothing in server behavior, yet. This proposal exists to:
- Record the confirmed problem (single-lane serialization under multi-node
  burst load) so it isn't lost or rediscovered from scratch.
- Record why the obvious fix (raise the default) isn't a safe default
  change without better data, so a future attempt doesn't repeat the same
  noisy, same-host, back-to-back-replica mistakes.
- Define what a trustworthy measurement looks like (see `tasks.md`).

## Capabilities

### New Capabilities

(none yet — no behavior changed)

### Modified Capabilities

(none yet)

## Impact

- **Server code**: none in this change. The eventual fix (whatever value,
  or whatever mechanism — a static default change, an adaptive scheme, or
  a startup warning instead) touches `server/src/infrastructure/config.rs`
  (`DEFAULT_LOGIC_LANES`/`DEFAULT_LOGIC_CONTROL_LANES`).
- **Clients**: none — purely a server-side tier-configuration concern.
- **Related work**: `shard-fabric`'s own pending task 2.3 (re-measure
  scaling across separate hosts) has the same "single host confounds every
  conclusion" problem this finding ran into; a dedicated measurement
  environment would likely serve both investigations at once.
