# Spec Delta

## ADDED Requirements

### Requirement: Coalesced Outbound Fan-Out
When several server messages are destined for connections served by the same
transport node and are produced together, the logic tier MUST deliver them to that
node in a single bounded fabric message rather than one fabric message per
connection. Per-connection message order and the client-visible messages MUST be
unchanged.

#### Scenario: Two seats on one node cost one fabric message
- **WHEN** a room action produces a message for each of two connections served by the same transport node
- **THEN** the logic tier sends one fabric message to that node carrying both, not two fabric messages

#### Scenario: The client still receives the same messages
- **WHEN** a client plays a move
- **THEN** it receives exactly the `Update` it receives today, with the same fields and order

#### Scenario: A connection's messages keep their order
- **WHEN** several messages for one connection are coalesced with messages for another connection on the same node
- **THEN** each connection still receives its own messages in their original order

### Requirement: No Redundant Re-serialization Across the Bridge
A client frame MUST cross the transport<->logic boundary without being parsed and
then re-serialized only to be parsed again. A frame the transport has already
decoded MUST be forwarded in a form the logic can consume with a single decode,
and the client-visible result MUST be unchanged.

#### Scenario: A move crosses the bridge once
- **WHEN** a client sends a move
- **THEN** the move is encoded once on the transport side and decoded once on the logic side

#### Scenario: Behavior is unchanged
- **WHEN** any client frame is forwarded under this rule
- **THEN** the logic applies exactly the same message it applies today
