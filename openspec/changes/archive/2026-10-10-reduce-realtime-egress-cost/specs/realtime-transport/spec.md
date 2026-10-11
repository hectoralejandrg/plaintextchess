# Spec Delta

## ADDED Requirements

### Requirement: Bounded Realtime Egress Cost
The transport tier MUST bound its per-move egress cost - the flushes, write time,
and bytes it spends delivering server messages to a client - so that the per-node
ceiling is not fixed by the flush cost of each frame. It MUST keep every
client-visible frame, its fields, and its per-connection order unchanged.

#### Scenario: A connection's ready frames share one flush
- **WHEN** several server frames are ready for one connection at once
- **THEN** the transport writes them with one flush instead of one flush per frame, without changing the frames or their order

#### Scenario: The client still receives the same frames
- **WHEN** a client plays a move
- **THEN** it receives exactly the same frames, with the same fields and order, as before

#### Scenario: Egress cost is measured, not assumed
- **WHEN** the egress path is optimized
- **THEN** the change is backed by a measurement of flushes, write time, and bytes per move on the reference hardware, and the per-node ceiling at move-ack p95 = 100 ms is re-reported
