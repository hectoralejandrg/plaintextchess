# Spec Delta

## Purpose

Define how each online room is owned by exactly one logic instance and how
transport nodes route a connection's game messages to the room's owner,
including membership, leases, and failover when an owner is lost.

## ADDED Requirements

### Requirement: Single Room Ownership
Each active room MUST be owned by exactly one logic instance at any time, and
only the owner MUST apply moves or mutate that room's authoritative state
(board, move list, clocks, ratings).

#### Scenario: Only the owner mutates a room
- **WHEN** two logic instances are running and a room is created
- **THEN** exactly one instance is recorded as its owner and only that instance applies its transitions

#### Scenario: No split ownership
- **WHEN** ownership of a room is resolved concurrently
- **THEN** the system does not allow a second instance to own the same room at the same time

### Requirement: Room Ownership Record
The system MUST maintain a shared record mapping each active room (by its code)
to its owning logic instance, updated whenever a room is created, claimed, or
released.

#### Scenario: A created room is published with an owner
- **WHEN** a player creates a room and the room is placed on a logic instance
- **THEN** the ownership record maps the room code to that instance before clients can join

#### Scenario: A removed room is unpublished
- **WHEN** a room becomes empty and is removed
- **THEN** its ownership entry is removed so later joins cannot be routed to a dead room

### Requirement: Routing Frames to the Owner
A transport node MUST route each room-scoped client message to the room's
current owner, and if the owner is unknown or the entry is stale, it MUST
re-resolve ownership before discarding a message.

#### Scenario: A join is routed to the owner
- **WHEN** a joiner sends a join for a room code owned by another instance
- **THEN** the transport node routes the message to the owner and the owner seats the joiner

#### Scenario: Stale ownership is re-resolved
- **WHEN** a transport node finds an ownership entry pointing at an instance that no longer holds the room
- **THEN** it re-resolves the owner and routes the message correctly instead of dropping it

### Requirement: Logic Node Membership and Leases
Each logic instance MUST register itself with the event fabric and hold a lease
that it renews while alive, and ownership MUST be re-resolved when a lease
expires.

#### Scenario: A live node renews its lease
- **WHEN** a logic instance is running
- **THEN** it periodically renews its membership lease so its rooms keep resolving to it

#### Scenario: A lapsed membership lease is re-registered
- **WHEN** a logic instance's membership key is lost (e.g. a broker restart) while the instance is alive
- **THEN** its next renewal re-creates the lease (self-healing), rather than a no-op renew on a missing key

#### Scenario: An expired lease triggers re-resolution
- **WHEN** a logic instance's lease expires
- **THEN** its rooms are no longer owned by it and are eligible for re-claiming

### Requirement: Failover of Owned Rooms
When the owner of a room is lost, its ownership record MUST be freed so the room
can be re-claimed, no room MUST ever be owned by two instances, and the room's
players MUST receive a clear terminal outcome (not silence). Because game state
is not replicated, a lost room does not resume; restoring a game requires state
replication, which is out of scope for this change.

#### Scenario: A room's ownership is freed after owner failure
- **WHEN** a logic instance holding active rooms fails
- **THEN** each room's ownership record expires and can be re-claimed by a surviving instance, and no room is owned by two instances

#### Scenario: No double ownership during failover
- **WHEN** a failing owner's rooms are re-claimed
- **THEN** the ownership record has exactly one owner at every step

#### Scenario: A lost owner ends its rooms with a clear signal
- **WHEN** the logic instance owning a room is lost
- **THEN** that room's players receive a clear terminal signal (their connection is closed with a reason) rather than being left with a mute socket, and the room's ownership record is freed

### Requirement: Abandoned Connection Reclamation
A disconnect can be lost while the broker is down, leaving a connection recorded
whose node is gone. The system MUST NOT let such an abandoned connection block a
player indefinitely: the identity MUST be able to recover its seat (re-join its
room), and abandoned connection state MUST be reclaimed by node/liveness
detection or a bounded TTL rather than only when the room is removed.

#### Scenario: A reused identity can recover after a lost disconnect
- **WHEN** a client's earlier connection was abandoned during a broker outage and the same identity connects again
- **THEN** it can re-join its room and resume, and the abandoned connection state does not block it indefinitely

### Requirement: Single Writer for Durable Records
Because durable storage remains a single SQLite database, the system MUST
ensure that authentication, session, and finished-game records have a single
writer, or are partitioned so that no two logic instances write the same SQLite
database concurrently.

#### Scenario: Records are not written concurrently by two nodes
- **WHEN** multiple logic instances are running
- **THEN** durable record writes are serialized through a single designated writer or partitioned per instance, so the SQLite database is never written concurrently

#### Scenario: Auth state stays consistent across nodes
- **WHEN** a session is issued on one logic instance and used through another
- **THEN** the session is validated consistently rather than rejected for being unknown to that node
