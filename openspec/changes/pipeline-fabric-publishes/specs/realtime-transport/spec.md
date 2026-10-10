# Spec Delta

## ADDED Requirements

### Requirement: Pipelined Fabric Publish
The transport<->logic bridge MUST NOT pay a full Redis round trip per fabric
message on the hot path. The publisher MUST keep multiple fabric publishes in
flight (pipelined) on a connection, in the order the messages were produced, so
per-room and per-connection order is preserved. It MUST bound the in-flight
work and apply backpressure past that bound rather than dropping messages while
the fabric is healthy, and it MUST NOT change any client-visible message.

#### Scenario: Multiple publishes share one round trip
- **WHEN** a shard has several outbound messages ready to publish
- **THEN** it sends them in a single pipelined round trip instead of awaiting one reply per message

#### Scenario: Order is preserved under pipelining
- **WHEN** several messages for one room are published in a pipeline
- **THEN** each connection receives its own messages in their original order

#### Scenario: Backpressure instead of loss
- **WHEN** a shard reaches its in-flight cap
- **THEN** it applies backpressure (the producer waits) and no message is dropped while the fabric is healthy

#### Scenario: The client still receives the same messages
- **WHEN** a client plays a move
- **THEN** it receives exactly the same server messages, with the same fields and order, as before
