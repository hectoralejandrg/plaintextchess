# Spec Delta

## ADDED Requirements

### Requirement: Sharded Fabric
The transport<->logic bridge MUST be able to spread its per-room traffic across
multiple Redis shards instead of a single shared bus. A room's traffic MUST be
assigned to one shard (partitioned by room/connection key) so per-room and
per-connection order is preserved, low-volume lifecycle operations MUST use a
single consistent shard, and no client-visible message may change.

#### Scenario: A room maps to one shard
- **WHEN** a room's messages are published
- **THEN** they all go to the shard chosen by the room's key, so their order is preserved on that shard

#### Scenario: Distinct rooms use distinct shards
- **WHEN** several rooms produce traffic at once
- **THEN** their messages are spread across the configured shards rather than serialized on one instance

#### Scenario: Lifecycle operations are consistent
- **WHEN** a node registers, renews a lease, or lists logic nodes
- **THEN** that operation uses the control shard, giving one consistent view

#### Scenario: The client still receives the same messages
- **WHEN** a client plays a move
- **THEN** it receives exactly the same server messages, with the same fields and order, as with a single shard

#### Scenario: Sharding is measured, not assumed
- **WHEN** the sharded fabric is used
- **THEN** the change is backed by a measurement of move-ack p95 and per-node concurrent games at 1 shard vs N shards on the same hardware
