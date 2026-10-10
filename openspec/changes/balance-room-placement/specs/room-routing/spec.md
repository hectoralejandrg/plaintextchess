# Spec Delta

## ADDED Requirements

### Requirement: Balanced Room Placement
When a player creates a room, the system MUST choose among the live logic
instances so that new rooms are distributed across them, rather than always
selecting the same instance. Placement MUST NOT change the ownership contract:
exactly one instance owns each room, the ownership record is written before a
join can be routed, and failover keeps working.

#### Scenario: Two live instances each receive rooms
- **WHEN** two logic instances are live and many rooms are created
- **THEN** the rooms are distributed across both instances instead of all landing on one

#### Scenario: A single instance takes every room
- **WHEN** only one logic instance is live
- **THEN** it receives every new room (placement has nothing to balance)

#### Scenario: A dead instance is never chosen
- **WHEN** a logic instance has stopped renewing its lease
- **THEN** new rooms are not placed on it

#### Scenario: Ownership is unchanged by placement
- **WHEN** a room is created on a chosen instance
- **THEN** its ownership record names exactly that instance before any join routes to it
