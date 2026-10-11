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

### Requirement: Legal Move Queries Reflect the Current Position
The core MUST return, for a given square, exactly the legal moves of the
position's current legal-move set, and after any operation that changes the board
(a played move or a restored session) subsequent queries MUST reflect the new
position. Any internal caching of legal moves MUST be invalidated by every such
operation and MUST NOT change the result of a query.

#### Scenario: A square's moves match the position
- **WHEN** the core is asked for the legal moves of a square
- **THEN** it returns exactly the moves of that square in the position's current legal-move set

#### Scenario: A played move invalidates cached moves
- **WHEN** a move is played and a square is queried afterwards
- **THEN** the answer reflects the position after the move, not any previously cached set

#### Scenario: A restored session invalidates cached moves
- **WHEN** a session's board is replaced (a restore) and a square is queried afterwards
- **THEN** the answer reflects the restored position, not any cached set

#### Scenario: A cached answer equals a freshly computed one
- **WHEN** a square is queried twice with no board change in between
- **THEN** both answers are identical, whether the second is served from a cache or recomputed

### Requirement: CPU Move Within a Documented Time Budget
The core MUST produce a CPU move for every supported difficulty within a
documented time budget on the reference hardware, so a synchronous `get_cpu_move`
call has a bounded cost and cannot stall the caller indefinitely.

#### Scenario: The hardest move stays within budget
- **WHEN** the core is asked for a CPU move at the hardest difficulty
- **THEN** it returns within that difficulty's documented time budget on the reference hardware

#### Scenario: Every difficulty returns within its budget
- **WHEN** the core is asked for a CPU move at any supported difficulty
- **THEN** it returns within the budget documented for that difficulty
