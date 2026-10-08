# Rust Core Spec

## Purpose
Crate-level contract for the chess-core library: game logic, board state,
Glicko-2 ratings, and the APIs an in-process consumer (the online server)
uses beyond the mobile FFI surface.

## Requirements

### Requirement: Rating state exposure for persistence
The core MUST expose the full Glicko-2 state of a session's player (rating,
rating deviation, and volatility) as a snapshot that an in-process consumer
can read and use to restore a fresh rating session, so that an in-process
server can persist rating state and reload it across restarts. The snapshot
APIs MUST be crate-level only: the UniFFI surface exposed to the mobile clients
(functions, errors, and types) MUST remain unchanged.

#### Scenario: Reading the rating snapshot
- **WHEN** an in-process consumer requests the rating state of a game session
- **THEN** the core returns the session's current Glicko-2 snapshot: rating, rating deviation, and volatility

#### Scenario: Restoring a snapshot into a fresh session
- **WHEN** an in-process consumer creates a new session from a `RatingSnapshot`
- **THEN** the core creates a fresh board (standard start position) with the player's Glicko-2 state restored to the snapshot's values (rating, deviation, volatility)
