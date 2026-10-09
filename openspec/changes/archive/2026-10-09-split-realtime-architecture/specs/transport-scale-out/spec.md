# Spec Delta

## Purpose

Allow the realtime transport tier to run as multiple interchangeable nodes
behind a load balancer, so client connection capacity grows by adding nodes
while a single authoritative logic instance remains in charge of game state.

## ADDED Requirements

### Requirement: Interchangeable Transport Nodes
The system MUST allow more than one transport node to run at the same time
behind a load balancer, and any node MUST be able to serve any newly connecting
client. Transport nodes MUST remain free of authoritative state so they can be
added or removed without migrating game data.

#### Scenario: A new client lands on any node
- **WHEN** a client connects while several transport nodes are running behind the load balancer
- **THEN** the node that accepts it can serve the full session without requiring a specific node

#### Scenario: A node can be added without downtime
- **WHEN** an additional transport node is started behind the load balancer
- **THEN** new connections are served by it while existing connections continue unaffected

#### Scenario: The load balancer needs no affinity
- **WHEN** a load balancer spreads clients across transport nodes without sticky sessions
- **THEN** any node can serve any client, because routing targets the node recorded for a connection rather than which node a client keeps hitting

### Requirement: Connection Registry
The system MUST keep, in shared storage, a record of which transport node holds
each live connection, keyed by connection id, so routing can fall back to it when
the owning node is not already known and stale entries can be detected and
re-resolved. The record maps a connection to its node; session and account state
is NOT stored here and stays on the authoritative tier.

#### Scenario: A response is delivered to the right node
- **WHEN** the authoritative tier emits a message for a connection held by a particular node
- **THEN** the message is delivered to that node, resolved from the connection's owning node (falling back to the registry when it is not already known)

#### Scenario: Registry is updated on connect and disconnect
- **WHEN** a client connects or disconnects
- **THEN** the registry reflects the owning node, and a stale entry no longer routes messages after the disconnect

### Requirement: Cross-Node Message Delivery
A client's inbound messages MUST be routed to the authoritative tier regardless
of which transport node received them, and outbound messages MUST be delivered
to the node holding the target connection, even when inbound and outbound pass
through different nodes.

#### Scenario: Move accepted on one node, response on the owner
- **WHEN** a client connected to node A sends a move and the authoritative tier produces a response
- **THEN** the response is delivered to node A and then to the client

### Requirement: Room-Scoped Delivery Across Nodes
Any event the authoritative tier addresses to a room MUST reach the connection(s)
that room currently holds, whichever transport node they are on, so a client on
one node observes the same room events as a client on another. A site-wide
broadcast is a non-goal for this change.

#### Scenario: A room event reaches a held connection on another node
- **WHEN** the authoritative tier produces an event for a room whose player is connected through another node
- **THEN** the node holding that player delivers the event to them

#### Scenario: Both players observe the same room events
- **WHEN** the two players of a room are connected through different transport nodes
- **THEN** each receives the room's events, so both observe the same game state

### Requirement: Reconnect Across Nodes
When a client reconnects during the reconnect grace window, it MAY be accepted
by a different transport node than before, and the system MUST re-attach it to
its session without duplicating or losing authoritative state.

#### Scenario: Reconnect lands on another node
- **WHEN** a client drops and reconnects to a different transport node within the grace window
- **THEN** it is re-attached to its game session and receives an up-to-date snapshot

### Requirement: Transport Node Failure Isolation
The failure of one transport node MUST NOT affect connections held by other
nodes, and the sessions of clients on the failed node MUST remain recoverable
within the reconnect grace.

#### Scenario: One node crashes
- **WHEN** a transport node crashes
- **THEN** clients on other nodes are unaffected and clients from the failed node can reclaim their sessions after reconnecting

#### Scenario: No session is lost to a node failure
- **WHEN** a client on a crashed node reconnects within the grace window
- **THEN** the authoritative state for its game is still available and it re-attaches

#### Scenario: A crashed node's seats enter grace
- **WHEN** a transport node crashes without publishing a disconnect for its connections
- **THEN** the authoritative tier detects that the node is gone and treats its connections as disconnected, so their seats are held for the reconnect grace and the opponent observes them as offline

#### Scenario: A crashed node does not hold a seat forever
- **WHEN** a crashed node's players do not reconnect within the grace window
- **THEN** the game ends with a clear terminal outcome, exactly as if the players had disconnected

#### Scenario: A paused node recovers without orphaning its players
- **WHEN** a transport node loses its lease while still running (a pause longer than the lease TTL, or lost lease keys)
- **THEN** on recovery it closes its connections so the clients reconnect and re-attach within the grace, rather than leaving a present player silently dropped
