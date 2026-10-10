# Spec Delta

## ADDED Requirements

### Requirement: Connection Affinity Updates Do Not Serialize
Updating which logic instance serves a connection MUST NOT require a global lock
held on the per-frame path, so transport throughput does not degrade as the
number of connections grows.

#### Scenario: Affinity updates add no global-lock contention
- **WHEN** a transport node serves many connections under load
- **THEN** updating a connection's logic-node affinity adds no global-lock contention to the per-frame path (verified by profiling)

#### Scenario: Transport throughput scales with the connection count
- **WHEN** the number of connections grows on a multi-core host
- **THEN** transport throughput scales with cores instead of serializing on one lock
