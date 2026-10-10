# Proposal

## Why

A profiling run with two logic nodes showed that **all 794 rooms were placed on
one node** while the second stayed at 0% CPU: for a `create_room` (which has no
room owner yet) the transport probes the registered logic nodes in list order
and the first one always accepts, so every new room lands on the same node. As
a result, adding logic nodes gives **0x** capacity today - the opposite of the
horizontal scaling the tiered architecture promises. Fixing placement is the
prerequisite for every later capacity gain.

## What Changes

- When a client creates a room, the transport MUST choose among the **live**
  logic nodes so new rooms are **distributed**, instead of always selecting the
  first one.
- Placement MUST keep the existing ownership contract: exactly one owner per
  room, ownership recorded before joins can route, and failover unchanged.
- With a single logic node the behavior is unchanged (that node takes every
  room).
- Room-scoped frames and joins keep routing to the room's recorded owner; only
  the *new-room* decision changes.

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `room-routing`: adds balanced placement of new rooms across live logic
  instances, so the number of instances actually multiplies capacity; the room
  ownership record and failover semantics are unchanged.

## Impact

- **Server code**: the create-room path in
  `server/src/infrastructure/transport_handler.rs` (node selection) and, if a
  load signal is needed, `server/src/infrastructure/broker.rs`.
- **Deployment/ops**: running M logic instances now distributes load instead of
  overloading the first; no topology change.
- **Clients**: none.
- **Specs**: modified `room-routing`.
