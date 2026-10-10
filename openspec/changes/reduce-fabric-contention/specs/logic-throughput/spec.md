# Spec Delta

## ADDED Requirements

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
