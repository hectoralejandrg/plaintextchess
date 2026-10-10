# logic-throughput Specification

## Purpose
Define how a single logic node processes inbound messages concurrently across
independent rooms while preserving per-room order, isolating control-plane work
from the move path, bounding inbound memory with backpressure, and exposing
enough telemetry to measure per-node capacity.

## Requirements

### Requirement: Concurrent Per-Room Processing
An inbound envelope for one room MUST NOT block processing of envelopes for a
room assigned to a different lane. A logic node MUST always assign a given room
to the same lane, and MUST process messages for the same room in arrival order.

#### Scenario: Rooms on different lanes progress in parallel
- **WHEN** two rooms each have a pending move and are assigned to different lanes
- **THEN** the node applies both without one room's processing waiting on the other

#### Scenario: A room stays ordered
- **WHEN** several messages for one room arrive in sequence
- **THEN** the node applies them in that arrival order

#### Scenario: A room is always handled by the same lane
- **WHEN** messages for one room are routed by the node
- **THEN** they are always handled by the same worker, so concurrent lanes cannot reorder a room's messages

### Requirement: Control-Plane Isolation
A logic node MUST process non-room-scoped control messages (create room and
auth) so that they do not delay move acknowledgements for rooms already in play.
Room-scoped join, leave, and resign MUST stay on the room's lane so they keep
the room's order.

#### Scenario: Auth does not stall moves
- **WHEN** a slow authentication is in progress
- **THEN** move acknowledgements for in-play rooms are unaffected

#### Scenario: A create/auth burst does not stall moves
- **WHEN** many create-room or auth messages arrive while games are in play
- **THEN** the in-play games' move acknowledgements are not delayed by the burst

#### Scenario: A join to one room does not stall other rooms
- **WHEN** many joins target a single room
- **THEN** move acknowledgements for other rooms are unaffected

### Requirement: Bounded Inbound Capacity
Per-lane inbound capacity MUST be bounded. When a lane is saturated the node
MUST exert backpressure rather than buffer envelopes without limit, and it MUST
NOT drop an inbound envelope silently.

#### Scenario: Saturation applies backpressure
- **WHEN** inbound volume exceeds lane capacity
- **THEN** the node stops pulling new envelopes instead of growing memory without bound

#### Scenario: No silent drop under saturation
- **WHEN** a lane is saturated with messages pending
- **THEN** the node either processes every accepted envelope or surfaces the condition, and never discards one without a signal

### Requirement: Inbound Hot Path Does Not Fetch the Fabric Per Frame
The inbound hot path MUST NOT perform a redundant fabric (Redis) lookup per
message. Once a connection's target logic node is known, forwarding its frames
MUST cost a single publish; the full node list MAY be consulted only when the
known target does not accept the frame (the cold/fallback path).

#### Scenario: The hot path costs one publish
- **WHEN** a frame arrives for a connection whose target logic node is already known
- **THEN** the transport publishes it once without listing the fabric's logic nodes

#### Scenario: The fallback lists nodes
- **WHEN** the known target does not accept the frame
- **THEN** the transport consults the logic-node list and retries against the live nodes instead of dropping the frame

### Requirement: Observable Inbound Lanes
A logic node MUST expose per-lane queue depth and processing latency, and an
inbound move-dispatch latency, so per-node capacity and regressions are
measurable.

#### Scenario: Lane metrics are available under load
- **WHEN** the node is under load
- **THEN** per-lane depth, processing latency, and inbound move-dispatch latency can be read from its telemetry

### Requirement: Configurable Lanes With Safe Default
The number of inbound lanes MUST be configurable. An unset value MUST fall back
to a safe default and an invalid value MUST be rejected, and the default MUST
preserve the per-room ordering guarantees of this capability.

#### Scenario: Unset lane count uses the default
- **WHEN** no lane count is configured
- **THEN** the node starts with a safe default that preserves per-room ordering

#### Scenario: An invalid lane count is rejected
- **WHEN** the configured lane count is zero or otherwise invalid
- **THEN** the node refuses to start rather than silently disabling lanes

### Requirement: Outbound Publish Scales With Workers
A logic node MUST publish outbound messages without a per-message shared
bottleneck (a lock or timer held for each message), so its per-message execution
does not serialize across worker threads. Increasing the node's worker
concurrency MUST NOT increase per-message contention.

#### Scenario: No shared lock dominates the publish path
- **WHEN** the node is under sustained load with multiple worker threads
- **THEN** no single shared lock or timer dominates the outbound publish path (verified by profiling)

#### Scenario: Sustained throughput scales with workers
- **WHEN** the node runs on more cores with the same load
- **THEN** its sustained outbound throughput increases rather than plateauing on one shared lock
