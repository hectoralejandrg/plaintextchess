# Spec Delta

## Purpose
Crate-level contract for the chess-core library: game logic, board state,
Glicko-2 ratings, and the APIs an in-process consumer (the online server)
uses beyond the mobile FFI surface.

## ADDED Requirements

### Requirement: Rating state exposure for persistence
The core MUST expose the full Glicko-2 state of a session's player (rating,
rating deviation, volatility) as a snapshot that an in-process consumer can
read and use to restore a fresh rating session, so that an in-process server
can persist rating state and reload it across restarts. The snapshot APIs
MUST be crate-level only: the UniFFI surface exposed to the mobile clients
(functions, errors, and types) MUST remain unchanged.

#### Scenario: Reading the rating snapshot
- **WHEN** an in-process consumer requests the rating state of a game session
- **THEN** the core returns the session's current Glicko-2 snapshot: rating, rating deviation, and volatility

#### Scenario: Restoring a snapshot into a fresh session
- **WHEN** an in-process consumer creates a fresh session from a previously read snapshot
- **THEN** the new session's Glicko-2 state matches the snapshot exactly (rating, deviation, and volatility), and subsequent rating updates continue from that state

#### Scenario: The FFI surface stays unchanged
- **WHEN** the mobile language bindings are regenerated after this change
- **THEN** the exposed UniFFI surface is identical to before: the snapshot APIs are not part of it
