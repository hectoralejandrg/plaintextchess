# Design

## Context

See `proposal.md` - Why. `shard-fabric` and the earlier room-ownership work
made multi-logic-node operation *possible* (disjoint room ownership via
atomic `SET NX`, routing via `get_room_owner`, both proven by
`rooms_distributed_across_logic_nodes` / `two_nodes_route_outbound_to_owner`
/ `cross_logic_routing_with_multiple_logic_nodes`). What remained unsafe was
rating correctness: each logic node's SQLite is independent (D7 of
`split-realtime-architecture`), and a `device_id`'s rating has no concept of
"home node" the way a room does. QA's adversarial testing then surfaced that
the control shard and the inbound publish path have no bound on Redis
unresponsiveness beyond "dead" (connection refused/reset) — `reduce-fabric-
contention`'s D1 explicitly accepted this risk for the hot publish
connection with the note "a coarse timeout is kept if needed," which is
exactly the situation this change is in.

## Goals / Non-Goals

**Goals:**
- A device's rating is the same regardless of which logic node currently
  owns its game.
- Every control-shard command (ownership, registry, leases, sessions,
  rating) fails within a bounded time when Redis is unresponsive, instead
  of hanging indefinitely.
- The inbound hot publish path gets the same bound without paying a
  per-message timer cost (the exact cost `reduce-fabric-contention`
  eliminated).
- A live node's own liveness lease is never mistaken for lapsed, and no
  reaping logic deletes a live node's registration.
- The `split_e2e` suite is deterministic across repeated runs.

**Non-Goals:**
- Account/profile consistency across logic nodes (sessions already solve
  the highest-risk case via `mirror_session`; account creation/profile
  data is lower-frequency, lower-impact, and deferred).
- Any change to the client-visible wire protocol.
- Redis HA/clustering (this change bounds *this server's* behavior when
  Redis misbehaves; it does not change how Redis itself is deployed).

## Decisions

### D1 - Cross-node rating mirror, Redis hash + last-write-wins

A Redis hash per device, `ptc.rating.<device_id>` → `{rating,
rating_deviation, volatility, updated_at_ms}`, on the control shard (same
place as ownership/registry/leases/sessions — write volume is at most twice
per finished game, the same order of magnitude as session issuance, already
accepted as cheap for that shard). A plain hash, not JSON, so the
last-write-wins guard (a small Lua script comparing `updated_at_ms`) never
needs to parse a payload. No TTL: unlike a session, a rating never expires.

Write: in `apply_terminal_ratings`, both players' post-game states are
mirrored concurrently (`tokio::join!`, not sequential awaits — see D2 for
why sequential would double the worst-case stall). A mirror failure is
logged and swallowed, same blast radius as a `GameRecorder` commit failure
(SQLite stays the durable history; Redis is the cross-node freshness
signal, not the source of truth — losing it degrades to today's bug, not to
data loss).

Read: a device's rating is refreshed from the mirror the moment it is
seated in a new room (`refresh_rating_from_mirror`, both the creator and the
joiner paths), trusting Redis whenever it has an entry (the only way the
local value could be fresher is if its own mirror write failed, rarer than
the staleness bug this exists to fix). Rejected alternative: making
`Ratings::rating_of`/`rating_state` async and reading through on every
access — ripples a sync trait used throughout the actor into an async one,
a much bigger diff for a value that changes at most twice per game.

### D2 - Bounded control-shard command latency

`redis-rs`'s `ConnectionManager` (used for everything on the control shard
via `Broker::conn()`) defaults `response_timeout` to `Duration::MAX` —
confirmed by reading `redis-0.26.1/src/aio/connection_manager.rs`. QA
demonstrated this with a stalled (`SIGSTOP`) Redis: terminal game results
hung until the stall ended. `Broker::connect` now builds the manager via
`ConnectionManager::new_with_config(client, ConnectionManagerConfig::new()
.set_response_timeout(COMMAND_RESPONSE_TIMEOUT))`, `COMMAND_RESPONSE_TIMEOUT
= 3s`. This bounds *every* control-shard command (not just rating) in one
place, matching `reduce-fabric-contention`'s own escape hatch for exactly
this risk. `connection_timeout` is left at its default (unbounded) —
tightening it governs the initial connection attempt, a different concern
not implicated by QA's finding, and over-tightening it risks failing to
connect to a Redis that is merely slow to accept new connections at
startup.

Verified (QA): both players' mirror timeouts fire within ~112μs of each
other (confirming D1's `tokio::join!` concurrency, not ~6s of sequential
timeouts), the happy path is unaffected (26ms, no timeout paid), and the
rest of the control shard (leases, `CreateRoom`, `Register`) is bounded the
same way.

### D3 - Inbound publish path health (periodic probe, not a per-command timeout)

The inbound publish connection (`ShardConn.publish_conn`, a plain
`MultiplexedConnection` by design D1 of `reduce-fabric-contention`
specifically *because* it must not arm a timer per command — every move
publishes through it) cannot take the same fix as D2 without reintroducing
the ~25%-of-active-time cost that change measured and eliminated. The
transport is also type-blind at this layer (`InboundPayload::ClientFrame`
carries raw, undecoded text — `CreateRoom` and `Move` are indistinguishable
until the logic tier decodes them), so scoping a timeout to "just
`CreateRoom`" is not possible without decoding client frames at the
transport, a materially bigger change.

Instead: extend the existing `shard_healthy: Vec<AtomicBool>` mechanism
(today only driven by the outbound subscription's reconnect loop,
`transport_handler.rs`) to also reflect the inbound publish connection's
health, via a periodic probe (a cheap command like `PING`, on an interval of
a few seconds, each probe wrapped in its own bounded timeout — arming one
timer per interval tick is negligible, unlike one per message). Consecutive
probe failures mark that shard's publish path unhealthy; `publish_inbound_to`
and its transport call sites check the flag before attempting a publish and
fail fast (same "broker unavailable" signal already used for outbound
health loss) instead of hanging on a stalled socket. This bounds a stalled-
Redis `CreateRoom` to the probe interval instead of indefinitely, at zero
added cost on the steady-state healthy path.

### D4 - Lease-flapping / stale-reaper root cause (confirmed, fixed)

Two independent bugs, not one (QA's single incident report conflated them):

**Bug A — TOCTOU in `liveness::renew`/`membership::renew`.** Both did
`EXISTS` then `SET EX` as two separate round trips (already flagged as
`BUG-R1-F2` for the sibling `membership::renew`, never fixed there either).
After a full Redis restart, this is architecturally a single-fire false
positive per node (TTL=30s is 3x the 10s renewal cadence, so only the very
first post-restart tick can race), not a repeating flap — QA's "every
10-20s for 11 minutes" is the aggregate of several independently-timed
nodes (`c1`/`c2`/the transport) each hitting their own one-shot race on
their own staggered reconnect schedule. Fixed by collapsing both into one
atomic Lua script (`local existed = redis.call("EXISTS", KEYS[1]);
redis.call("SET", KEYS[1], ARGV[1], "EX", ARGV[2]); return existed`),
the same pattern already used by `ownership::RENEW_SCRIPT` and
`rating::STORE_SCRIPT`. Response misattribution on the shared
`MultiplexedConnection` (QA's original hypothesis) was investigated and
**refuted** by reading `redis-0.26.1`'s source directly: responses are
matched strictly FIFO per connection, and `reconnect()` swaps in an
entirely fresh connection + queue on loss, so no response from a dead
connection can land on a new one.

**Bug B — the subscribe-retry budget was far shorter than a real outage.**
`MAX_SUBSCRIBE_FAILURES=10` with the existing capped exponential backoff
exhausts in ~17 seconds, after which a logic node calls
`unregister_logic_node()` **on itself** and exits non-zero. In a supervised
deployment (`restart: unless-stopped`), the container respawns as a
brand-new process with a brand-new node id — which, from the outside,
looks exactly like "a live node's key got deleted while it was still
alive" (QA's "11 DELs including c2, a node confirmed still alive"): the
DEL was real and self-inflicted, immediately followed by a fresh
re-registration under a different identity. Fixed by raising
`MAX_SUBSCRIBE_FAILURES` to 150 (≈5 minutes at this backoff shape,
comfortably past the 30s TTL by over an order of magnitude), so a node
rides out a realistic outage instead of abandoning its own identity
mid-recovery.

**Bug C — found while verifying A and B, not in the original QA report:**
`spawn_logic_watch` (transport side, watches logic node membership) acted
on a single `logic_node_alive() == false` with no debounce, unlike the
symmetric watcher on the other side (`logic_handler.rs`'s watch of
transport nodes, which already requires `NODE_MISS_THRESHOLD=2`
consecutive misses). Surfaced as a real, reproducible flake in
`same_node_pair_lease_loss_keeps_the_room` under test-suite load (a
transient `EXISTS` miss closed connections with "logic node was lost"
instead of the lease-loss reason the test actually triggered). Fixed by
adding the same 2-consecutive-miss debounce (`LOGIC_NODE_MISS_THRESHOLD`),
mirroring the existing pattern exactly rather than inventing a new one.

### D5 - Test-harness background-task lifecycle

`LogicState::run()` spawns five long-lived `tokio::spawn` tasks (room lane
workers, control lane workers, the lease-renewal loop, the room-ownership
keep-alive loop, the transport-liveness watcher) and never retains their
handles — fine for a real process (the whole process exits together), but
in `tests/split_e2e.rs`, where many `LogicState`s are created and torn down
within one test binary, only the outer task running `run()` itself gets
`.abort()`-ed; these five keep running and keep renewing/re-writing
`ptc.logic_node.*`/`ptc.room_owner.*` in the *shared* test Redis for their
TTL window (30-180s) into the next test. Combined with four literal
duplicate `PORT_*` constants (`PORT_RECLAIM`/`PORT_ABANDON_1` both 18827,
and three more pairs), this fully explains the run-to-run flakiness QA
measured (42/47 → 46/47 → 47/47) without needing to touch any test's
actual assertions or timeouts.

Fix: collect `LogicState::run()`'s internal spawns into a `tokio::task::
JoinSet` (or an equivalent tracked `Vec<JoinHandle<_>>` mirroring
`TransportState::liveness_task`'s existing pattern) so the test harness can
abort all of them on `Drop`, and renumber the four duplicate port constants
to unique values. This is test-only infrastructure, not a product
requirement — no spec delta, tracked in `tasks.md` only.

## Risks / Trade-offs

- [Redis mirror and local SQLite can disagree] -> deliberate: SQLite is
  durable history, Redis is a freshness signal; losing Redis degrades to
  today's bug (default rating until the next game), not to data loss.
- [3s control-shard timeout is itself a guess] -> chosen to be well above
  normal command latency and far below "indefinite"; revisit if real
  deployments show it's too tight or too loose.
- [Publish-path health probe adds a background task per shard] -> bounded,
  periodic, and off the message path entirely; matches the cost profile of
  the liveness/lease-renewal loops already running.
- [D4 unresolved at time of writing] -> this change should not be
  considered complete, or multi-logic-node operation called safe for
  production, until D4 lands; tracked explicitly in `tasks.md` as a gate.

## D5 - A second QA round found 3 real bugs in this change's own implementation

A re-verification of D2/D3/D4 (not of the original rating-mirror work, which
was unaffected) found three problems in the fixes themselves, confirmed
against the actual HEAD at the time (`e234d8e`):

**3.1 — `MAX_SUBSCRIBE_FAILURES`'s "~5 minute" budget had zero margin.**
QA followed this file's own "wait at least 5 minutes" guidance and the node
died at exactly that boundary (296.6s-300.5s across runs) — a budget that
*just barely* meets a target survives *less than* it. Raised from 150 to
600 (~20 minutes), with real margin past both the 30s TTL and the ~11
minute incident that motivated this work.

**3.2 — the publish-health probe never repaired the connection it tests.**
`ping_publish_shard` reused `publish_connection`'s cached
`MultiplexedConnection` but, unlike `publish_with_retry` (which drops the
cache on a publish error, forcing a reconnect), never invalidated a broken
cache on a failed ping. QA observed the probe reporting unhealthy
indefinitely after a real Redis restart — recovery only happened once a
real client's publish attempt hit `publish_with_retry`'s own repair path.
Fixed by dropping the cached connection on a failed probe too, the exact
same one-line pattern `publish_with_retry` already used. Re-verified: a
`CreateRoom` sent after Redis recovered, with **no client connecting in
between**, now succeeds in 17ms — the probe self-heals within one tick
(confirmed via transport logs: 4 consecutive "unhealthy" warnings at the
new 500ms cadence, then silence, then a fast `CreateRoom`).

**3.4 — the detection-window race was real and reproduces reliably.**
A message sent in the gap between a stall starting and the next probe tick
still rides the old, now-stuck connection (no per-command timeout by
design — see D3). QA measured this hanging ≥90s (their test's own patience
limit, not an organic resolution). This cannot be closed without paying
the per-message timer cost D1 of `reduce-fabric-contention` eliminated, and
the transport cannot distinguish `CreateRoom` from `Move` before
publishing (it forwards the raw, undecoded frame) to scope a timeout
narrowly. Mitigated by shrinking the window: `PUBLISH_HEALTH_CHECK_INTERVAL`
2s→500ms, `PUBLISH_HEALTH_PROBE_TIMEOUT` 3s→1.5s (worst case ~5s → ~2s).
Re-verified directly: triggering the exact race (stop Redis and send the
message in the same event-loop tick) still hangs — confirmed as an
accepted, documented trade-off, not a silently-missed bug — but resolves in
~211ms once Redis actually responds again, i.e. the hang is bounded by
"however long Redis is actually unresponsive," not "forever," except under
a synthetic stall (`SIGSTOP`) that produces neither a response nor a
connection-level error.

**3.3 — close reason ambiguity (deferred).** A client can get `1011 broker
unavailable` for two different causes (pre-emptive fail-fast vs. a
publish retry exhausting its attempts) with the same wire-visible reason.
Fixing this well needs a new error code, a wire-protocol change requiring
coordinated updates to the iOS/Android decoders (per `wire_form_is_stable`'s
contract) for a low-severity log-clarity issue. Deferred; not blocking.

## D6 - A third QA round found the real end-to-end recovery bottleneck

A machine-readable, highly rigorous re-verification of D5's three fixes
(reconnect budget, probe self-heal, detection-window shrink) confirmed all
three with direct measurement (self-heal: 6/6 restarts, 0 lingering
unhealthy warnings, 34-242ms detection latency; budget: measured death at
1198.2s/1199.7s vs. a declared ~1200s target, survives 18 minutes with
margin; race window: measured 1588-1862ms detection, boundary at 1500-2000ms
matching the configured values exactly, resolves in 210-229ms once Redis
responds). All three: **PASS**, each previously-claimed number matched to
within measurement noise.

But it also did something I hadn't: attribution. It measured `create_room`
success *simultaneously* with the actual Redis keys (`ptc.logic_node.*`,
`ptc.node.*`) across a real, full Redis restart, and found end-to-end
service recovery took 10-18s — not "tens of ms" as I'd assumed — and proved
the bottleneck was neither the publish-health probe (self-heals in <250ms,
separately confirmed) nor `COMMAND_RESPONSE_TIMEOUT`, but the logic node's
own `ptc.logic_node.*` membership-lease renewal: a fixed
`tokio::time::interval(10s)` loop (`logic_handler.rs`) that, on a failed
renewal, simply waits for its next scheduled tick — up to the full 10s —
before trying again, regardless of how quickly Redis actually comes back.
Service resumed *exactly* when that key reappeared (t+11s in the QA
measurement), not when the probe recovered (t+<1s).

Fixed by replacing the fixed interval with a `sleep`-based loop: success
resets to the steady 10s interval (unchanged happy-path behavior and Redis
load); failure backs off using the same shape `reconnect_backoff` already
uses for the subscribe-retry loop in the same file (200ms, 400ms, 800ms,
1600ms, then capped at 2s) — bounding the "Redis came back, how long until
we notice" window to ~2s instead of up to 10s. Applied the identical fix
proactively to the structurally-identical loop on the transport side
(`transport_handler.rs::spawn_liveness`, which maintains `ptc.node.*`) —
not proven by QA to matter for this specific symptom (the gating dependency
for `create_room` is the *logic* node's key, not the transport's own), but
the same bug shape, same fix, same file family; fixing one and knowingly
leaving its twin unfixed would be inconsistent.

Re-verified directly: killed Redis for a real 4s outage, restarted it fresh,
and probed `create_room` repeatedly — recovered in **1247ms** (previously
10-18s), with the logic log showing the expected single failed renewal
followed by the backoff ramp succeeding well under the old fixed wait.

**OBS-QA-R5-B** (info, not fixed): once a logic node exhausts its
reconnect budget and exits, a surviving transport node's `/healthz` still
reports `"status":"ok"` even though no logic node remains for it to route
to. An observability gap, not a bug — left as a documented, deliberately
deferred follow-up; not implemented unless specifically requested.

## D7 - QA round 6: a fourth QA round, this time including general performance

QA re-verified D5/D6's fixes (PASS, with the nuances below) and separately
ran the general-performance round that closes (partially) the long-standing
`shard-fabric` question. Full detail lives in
`docs/support/qa-performance-findings.md` (round 6); summarized here only
as it bears on this change:

**Confirmed working, no action needed:**
- The reconnect-budget, probe-self-heal, and detection-window fixes (D5)
  all held up under re-test.
- D6's lease-renewal backoff fix: real and large (10-18s -> 0.46-3.4s
  recovery), but QA found the mechanism isn't quite what was designed —
  only 1-2 renewal-failure warnings are logged per outage regardless of
  duration (2s or 15s), meaning the outer `sleep`-based backoff loop rarely
  gets to retry more than once; the actual recovery is dominated by
  `redis-rs`'s own `ConnectionManager` reconnecting underneath, not the
  loop's 200ms-2s backoff schedule. This also explains why RTO is
  non-monotonic in outage duration (a 2s outage measured worse, 3448ms,
  than an 8s one, 462ms) — what matters is where the outage's end lands
  relative to a renewal attempt already in flight and blocked on
  `COMMAND_RESPONSE_TIMEOUT` (3s), not how long the outage lasted. Not
  changed further: the ~3.4s worst case is still a large improvement, and
  tightening it means touching `COMMAND_RESPONSE_TIMEOUT` itself (blast
  radius: the entire control shard) for a shrinking, already-rare tail.

**GAP-QA-R6-D (fixed) - intermittent 0-8% setup failures under multi-node
load.** Traced to the exact mechanism, not inferred: `claim_room`
(`broker.rs`, called from `logic_handler.rs`'s `handle_create_room`) and
`get_room_owner` (called from `transport_handler.rs`'s `resolve_room_owner`)
both go through the control shard's `COMMAND_RESPONSE_TIMEOUT`. Under a
burst of thousands of concurrent `CreateRoom`s across several logic nodes,
a single claim can exceed 3s from queueing alone — a real but transient
condition, not a Redis outage. Both failures were silently swallowed
(logged as a `warn!` and continued), so the ownership key simply never got
written; the second player's `JoinRoom` then had no owner to resolve and
misrouted to a random node that never had the room, timing out with
`plies=0`.

Fixed by retrying both (reusing `PUBLISH_ATTEMPTS`/`PUBLISH_RETRY_DELAY`,
the same shape already used by `publish_with_retry`). `claim_room` needed
one extra piece of care a naive retry would get wrong: `SET NX` can
succeed server-side after the client already gave up waiting on it, so a
retry can see the key already set — by this exact claim, under this exact
node id. That must read as success, not "someone else claimed it first":
after exhausting retries on errors, the fix checks who the current owner
actually is before reporting failure.

**Confirmed as a second, independent contributor, split into its own
change**: `docs/support/qa-performance-findings.md`'s single-control-lane
hypothesis was real — reproducing the exact QA scenario measured ~37%
setup failures from the default `LOGIC_CONTROL_LANES=1` alone, independent
of the Redis-timeout mechanism this change fixes. It is **not** fixed here:
an attempt to raise the default was reverted after a cleaner retest showed
the naive fix (raise the default) doesn't hold up — more lanes shifted more
concurrent load onto the shared control shard, and a 16-lane run measured
worse (19.4% failures) than an 8-lane one (5.6%) that initially looked like
the answer. See `openspec/changes/tune-logic-lane-defaults` for the full
investigation, what was tried, and what a trustworthy fix needs.

**GAP-QA-R6-A (fabric sharding, inconclusive, not this change's to
resolve)** - QA measured no throughput gain from sharding (1.08x, target
was >1.5x), but correctly caveated this as **not conclusive**: every node
in the test ran on one host, so the host's own CPU ceiling (~50-58k
moves/s) was the shared cap regardless of Redis topology. `shard-fabric`'s
own `tasks.md` already tracks the correct next step (task 2.3: re-measure
across separate hosts before concluding anything about sharding). Not
duplicated here; this change's scope is rating consistency and control-shard
reliability, not fabric scaling.

**GAP-QA-R6-E (fixed, trivial)** - `tests/split_e2e.rs`'s hardcoded port
range (18765-18873) now has a header comment warning a developer not to run
a manual server in that range while the suite runs.

**OBS-QA-R6-F** - high inter-replica variance (up to ±50%) noted for
future benchmarking; not actionable as code.

## Open Questions

- Whether `COMMAND_RESPONSE_TIMEOUT`'s 3s value, and `MAX_SUBSCRIBE_
  FAILURES`'s ~5 minute budget, should become configurable rather than
  constants, once real-world Redis latency/outage data exists.
- Whether the same "stalled, not just dead, Redis" gap D3 fixes for the
  inbound publish path also applies to the outbound subscription (today's
  `shard_healthy` is driven by reconnect-on-error, which a truly stalled —
  not erroring — connection would not trigger either). Not fixed here
  (QA did not report it as broken); worth a follow-up probe-based check if
  it ever is.
