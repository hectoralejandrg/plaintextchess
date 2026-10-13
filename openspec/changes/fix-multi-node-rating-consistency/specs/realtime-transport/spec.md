# Spec Delta

## ADDED Requirements

### Requirement: Bounded Control-Shard Command Latency
Every control-shard command (room ownership, connection registry, node
leases, session mirroring, rating mirroring) MUST fail within a bounded
time when Redis is unresponsive, instead of hanging indefinitely. The
per-move hot publish path is exempt from a per-command timeout (a
documented, deliberate trade-off — see the `realtime-transport` fabric
requirements on coalesced fan-out and pipelined publish) and MUST instead
be protected by a periodic health check (see the next requirement).

#### Scenario: A stalled Redis does not hang a terminal game result
- **WHEN** Redis stops responding to commands (without closing the connection) while a game is ending
- **THEN** both players receive the terminal result within the configured bound, not indefinitely

#### Scenario: Both players' rating mirrors are bounded together, not doubled
- **WHEN** a game ends and both players' ratings are mirrored
- **THEN** the two mirror attempts run concurrently, so an unresponsive Redis delays the result by at most one timeout period, not two

#### Scenario: The happy path pays no latency cost
- **WHEN** Redis is healthy and responds normally
- **THEN** control-shard commands complete at their normal latency, with no added delay from the bound

#### Scenario: Other control-shard operations are bounded the same way
- **WHEN** Redis is unresponsive during a room creation, a lease renewal, or a session lookup
- **THEN** that operation also fails within the same bounded time, rather than only the rating mirror being protected

### Requirement: Inbound Publish Path Health
The inbound publish path (transport-to-logic, on the hot per-move
connection that MUST NOT carry a per-command timeout) MUST still detect an
unresponsive Redis within a bounded time, via a periodic health check
rather than a per-message one, so a stalled shard fails new inbound frames
fast instead of hanging.

#### Scenario: Room creation against a stalled Redis fails fast
- **WHEN** Redis stops responding to commands while a client sends `CreateRoom`
- **THEN** the client receives a clear failure within the health-check interval, not an indefinite hang

#### Scenario: Per-move latency is unaffected
- **WHEN** the inbound publish path's shard is healthy
- **THEN** no additional timer or latency is added to any individual move, preserving the cost profile the fabric message-path work established
