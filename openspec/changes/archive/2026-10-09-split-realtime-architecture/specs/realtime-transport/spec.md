# Spec Delta

## Purpose

Separate the realtime transport (terminating client WebSocket connections)
from the authoritative game-logic tier, connecting them over a Redis event
fabric while keeping the client-facing wire protocol unchanged.

## ADDED Requirements

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
