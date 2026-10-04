# Spec Delta

## ADDED Requirements

### Requirement: Core Game Session Isolation
The Rust core MUST expose each game session as an independent object that
owns its own board state, side to move, and player rating: sessions MUST
NOT share mutable state, a move played or state read on one session MUST
NOT affect any other session, and creating, playing in, or discarding a
session MUST NOT alter any other session's state. All exported session
operations MUST be safe to call concurrently from multiple threads, and the
read-only operations (board state, valid moves, piece queries, check,
checkmate, draw, CPU move, and rating queries) MUST NOT mutate the
session.

#### Scenario: Two sessions do not interfere
- **WHEN** two sessions are created and a move is played in one of them
- **THEN** the board state of the other session is still the standard initial position and its valid moves are unchanged

#### Scenario: CPU move query does not mutate the session
- **WHEN** a CPU move is requested from a session
- **THEN** the call returns a legal UCI move for the position, and the session's board state, side to move, and history are unchanged afterwards

#### Scenario: Concurrent read-only calls are safe
- **WHEN** multiple threads request CPU moves and board state concurrently, both from the same session and from other sessions
- **THEN** all calls complete without an FFI error, and every session's position remains consistent with the moves actually played in it

#### Scenario: End of one game does not affect another
- **WHEN** one session reaches checkmate or a position with no legal moves
- **THEN** any other session is unaffected: it still reports its own side to move, its own legal moves, and its own status
