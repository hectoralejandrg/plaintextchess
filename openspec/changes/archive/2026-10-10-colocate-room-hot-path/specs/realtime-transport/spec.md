# Spec Delta

## ADDED Requirements

### Requirement: Room Locality for the Hot Path
The system SHOULD keep a room's connections on a single transport node and SHOULD
serve a colocated room's move/update path in-process, so a room's per-move cost
does not include a cross-node fabric hop. It MUST keep every client-visible
frame, its fields, and its per-connection order unchanged, and MUST keep the
fabric path for cross-node rooms and reconnects.

#### Scenario: A room's seats share a node
- **WHEN** a second player joins an existing room
- **THEN** their connection is placed on the transport node serving the room's first seat

#### Scenario: A colocated move stays local
- **WHEN** a move is played in a room whose connections are all on one node
- **THEN** the update is produced and enqueued to those connections without a cross-node fabric hop

#### Scenario: The client still receives the same frames
- **WHEN** a client plays a move
- **THEN** it receives exactly the same frames, with the same fields and order, as before

#### Scenario: Cross-node rooms still work
- **WHEN** a room's connections cannot be colocated (reconnect, or placement fails)
- **THEN** the move/update path uses the fabric exactly as before
