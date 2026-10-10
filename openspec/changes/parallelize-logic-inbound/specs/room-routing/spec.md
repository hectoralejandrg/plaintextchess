# Spec Delta

## MODIFIED Requirements

### Requirement: Logic Node Membership and Leases
Each logic instance MUST register itself with the event fabric and hold a lease
that it renews while alive, and ownership MUST be re-resolved when a lease
expires. Renewal of the room-ownership leases it holds MUST be batched so a
sweep costs a bounded number of round trips rather than one per room, and the
renewal sweep MUST be able to complete within the lease TTL while the node is
under load, so an active node never spuriously re-claims its own rooms.

#### Scenario: A live node renews its lease
- **WHEN** a logic instance is running
- **THEN** it periodically renews its membership lease so its rooms keep resolving to it

#### Scenario: A lapsed membership lease is re-registered
- **WHEN** a logic instance's membership key is lost (e.g. a broker restart) while the instance is alive
- **THEN** its next renewal re-creates the lease (self-healing), rather than a no-op renew on a missing key

#### Scenario: An expired lease triggers re-resolution
- **WHEN** a logic instance's lease expires
- **THEN** its rooms are no longer owned by it and are eligible for re-claiming

#### Scenario: Renewal keeps up under load
- **WHEN** a node owns many rooms under sustained load
- **THEN** it renews all of its ownership leases within the lease TTL and does not spuriously re-claim rooms it still holds
