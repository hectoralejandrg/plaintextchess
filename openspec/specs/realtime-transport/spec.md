# realtime-transport Specification

## Purpose
Separate the realtime transport (terminating client WebSocket connections)
from the authoritative game-logic tier, connecting them over a Redis event
fabric while keeping the client-facing wire protocol unchanged.

## Requirements

### Requirement: Transport and Logic Tier Separation
The system MUST run the realtime transport as its own process that terminates
every client WebSocket connection, separately from the logic process that owns
authoritative game state. The transport tier MUST NOT own authoritative room,
board, clock, or rating state. Clients MUST continue to connect to the same
WebSocket endpoint path with the same versioned JSON protocol they use today.

#### Scenario: Clients connect through the transport tier unchanged
- **WHEN** a client opens the WebSocket endpoint using the existing protocol version
- **THEN** the connection is terminated by the transport tier and the client observes no change in the endpoint or version handshake

#### Scenario: The transport tier holds no authoritative game state
- **WHEN** the transport process is restarted while the logic process stays up
- **THEN** no authoritative game, clock, or rating state is lost

### Requirement: Transport-to-Logic Bridging over Redis
The transport tier and the logic tier MUST exchange client messages and server
responses through a Redis event fabric with per-connection addressing, and MUST
preserve the order of game transitions within a room. A message accepted from a
client MUST be delivered to the logic tier and MUST NOT be silently dropped
while the fabric is healthy.

#### Scenario: A move reaches the authoritative tier
- **WHEN** a client sends a move frame to the transport tier
- **THEN** the logic tier receives it, applies the game rule, and the resulting server messages are delivered back to the correct connections in order

#### Scenario: Messages are addressed per connection
- **WHEN** the logic tier emits a response for one connection
- **THEN** only the transport node holding that connection delivers it, and no other client receives it

### Requirement: Transport Protocol Fidelity
A connection served through the split tiers MUST behave exactly as it did on
the monolithic server: the same server message types (snapshots, incremental
updates, auth replies, structured errors), the same error codes, the same
protocol close behavior, the same keepalive pings, and the same reconnect
grace.

#### Scenario: Snapshot then incremental updates
- **WHEN** a game is in progress through the split tiers
- **THEN** the client receives a full snapshot on connect and incremental updates on each move, exactly as the monolithic server produced

#### Scenario: Protocol errors close the connection
- **WHEN** a client sends malformed JSON or an unknown message type
- **THEN** the transport tier closes the connection with the same protocol close code the monolithic server used

#### Scenario: Keepalive and reconnect grace are preserved
- **WHEN** a client goes idle or briefly disconnects
- **THEN** the transport tier sends keepalives and the logic tier honours the same reconnect grace window as before

### Requirement: Transport Backpressure and Coalescing
The transport tier MUST bound per-connection outbound buffering and MUST
coalesce state-class frames so a slow client converges to the newest
authoritative state, while one-shot control and error frames MUST NOT be
dropped.

#### Scenario: A slow client cannot grow memory without bound
- **WHEN** a client stops reading while the server keeps producing state updates
- **THEN** the transport tier drops superseded state frames, delivers the newest state, and keeps its buffered memory bounded

#### Scenario: A structured error is never coalesced away
- **WHEN** the server produces a one-shot error for a connection with pending state frames
- **THEN** the error frame is delivered and not discarded by coalescing

### Requirement: Authentication Stays on the Authoritative Tier
Authentication and session decisions MUST remain with the authoritative tier
(or an auth service it owns); the transport tier MUST NOT verify credentials or
mint, accept, or revoke sessions on its own, and MUST forward auth messages
unmodified.

#### Scenario: Login is decided by the authoritative tier
- **WHEN** a client sends a login message
- **THEN** the transport tier forwards it, the authoritative tier makes the decision, and the reply reaches the client through the transport tier

#### Scenario: An invalid credential is rejected as before
- **WHEN** a client sends an unknown username or a wrong password
- **THEN** the client receives the same structured auth error it received on the monolithic server

### Requirement: Broker is Required in Split Mode
When configured in split mode, the tiers MUST refuse to start without a working
Redis connection, and a loss of the fabric at runtime MUST surface a clear
error to affected clients rather than silently dropping messages or serving
stale state.

#### Scenario: Missing broker stops startup
- **WHEN** a tier is started in split mode with no reachable Redis
- **THEN** the tier exits with an error instead of serving traffic

#### Scenario: Broker loss is surfaced
- **WHEN** the Redis connection drops while clients are connected
- **THEN** affected clients receive a clear error or disconnect, and the tiers reconnect or exit rather than silently misbehaving

#### Scenario: An idle client is disconnected when the broker drops
- **WHEN** the broker is lost while a client is connected but sends nothing
- **THEN** that client still receives a disconnect or error rather than silence

#### Scenario: Subscriptions are re-established after recovery
- **WHEN** the broker becomes reachable again while the tiers are still running
- **THEN** each tier re-establishes its subscription and a new client is served again

#### Scenario: A client is served right after the broker recovers
- **WHEN** a client sends a frame as the broker becomes reachable again
- **THEN** the frame reaches the logic tier and the reply is delivered, rather than being lost to a stale connection

#### Scenario: A client is not silently ignored while the logic resubscribes
- **WHEN** a client sends a frame while the logic tier is still resubscribing after a broker restart
- **THEN** the frame is delivered once the logic resubscribes, or the client receives a clear disconnect — never a silent drop

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
