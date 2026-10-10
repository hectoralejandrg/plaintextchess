# Spec Delta

## ADDED Requirements

### Requirement: Legal Move Queries Reflect the Current Position
The core MUST return, for a given square, exactly the legal moves of the
position's current legal-move set, and after a move is played, subsequent
queries MUST reflect the new position. Any internal caching of legal moves is an
implementation detail: it MUST be invalidated by every move and MUST NOT change
the result of a query.

#### Scenario: A square's moves match the position
- **WHEN** the core is asked for the legal moves of a square
- **THEN** it returns exactly the moves of that square in the position's current legal-move set

#### Scenario: A move invalidates any cached moves
- **WHEN** a move is played and a square is queried afterwards
- **THEN** the answer reflects the position after the move, not any previously cached set

#### Scenario: A cached answer equals a freshly computed one
- **WHEN** a square is queried twice with no move in between
- **THEN** both answers are identical, whether the second is served from a cache or recomputed
