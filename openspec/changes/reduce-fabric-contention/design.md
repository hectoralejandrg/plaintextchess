# Design

## Context

See `proposal.md` - Why. The profiling report pinned two per-message shared
resources:

- Logic: redis-rs wraps each command in a response timeout; the timeout arms a
  tokio timer, and inserting/removing that timer touches the shared timer-wheel
  behind a `Mutex`. Under load (many concurrent publishes) this is ~25% of the
  node's active time.
- Transport: every accepted frame calls `set_conn_logic_node`, taking a global
  `std::sync::Mutex` on the connection->logic map (~19% of active time).

The `MultiplexedConnection` already pipelines commands; the cost is the
per-command timer bookkeeping, not the network.

## Goals / Non-Goals

**Goals:**
- Remove the per-message shared timer from the publish path and the per-frame
  global lock from the transport affinity update.
- Preserve all behavior, routing, and client-visible results.
- Make the fix measurable with the profiling/load harness.

**Non-Goals:**
- SQLite WAL commit cost (largely inflated by synthetic churn; a separate
  durability change if wanted).
- Redis server-side scaling beyond ~150k ops/s (pipelining/sharding; separate).
- Changing channels, envelopes, or routing.

## Decisions

### D1 - Publish over a dedicated no-timeout connection
The hottest shared cost was a tokio timer armed for **every** Redis command: the
publish path went through a `ConnectionManager`, whose config sets
`response_timeout = Duration::MAX` (it never uses the `None`/no-timer path), so
each command registers a timer under the shared timer-wheel lock. A URL parameter
(`response_timeout=0`) does **not** help: redis-rs parses only
`protocol`/`db`/`user`/`pass` from the URL and the manager ignores it anyway.

Instead, the publish path uses its own plain `MultiplexedConnection` (created
with no response timeout, so commands do not arm a timer), reconnected lazily
when a publish fails. The `ConnectionManager` stays for all other commands
(registry, leases, membership), which are periodic rather than per-message.
Alternative: a small pool of no-timeout connections; rejected as unnecessary
because one multiplexed connection already pipelines concurrent commands.

Trade-off: without a per-command timeout, a hung Redis no longer fails a single
publish quickly; the connection error still surfaces (and the retry loop
reconnects), which the broker-loss tests cover.

### D2 - Affinity map without a per-frame global lock
Shard the transport's connection->logic map by `hash(connection_id) % N` (as the
logic side already does for its connection map), or write the affinity only when
it changes (skip the lock when the value is already correct). Sharding removes
the global lock; write-on-change removes most of its frequency. The cheaper fix
that the profiling shows sufficient is chosen; the two can be combined.

### D3 - Behavior preserved
Neither change alters routing, ownership, or protocol. The broker keeps its
publish retry/liveness behavior; the transport keeps skipping stale nodes and
closing clients when no logic is subscribed.

## Risks / Trade-offs

- [No per-command timeout hides a slow broker] -> the connection manager still
  reports connection-level errors, and the split tests cover broker loss
  recovery; a coarse timeout is kept if needed.
- [Sharding the affinity map changes iteration] -> any code that iterates all
  connections must iterate all shards (as the logic side already does); covered
  by the existing failover/registry tests.
- [Optimizing the wrong thing] -> the change is gated on the profile dropping,
  not on a code assumption.

## Open Questions

- Whether the pinned redis-rs honors "no timeout" by skipping the timer, or
  whether a dedicated no-timeout connection is required.
- Whether SQLite WAL commit cost matters under realistic (slow) game churn - a
  separate change.
