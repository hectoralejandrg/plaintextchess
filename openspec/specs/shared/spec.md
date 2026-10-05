# shared Specification

## Purpose
Define cross-platform requirements and specifications that apply to both iOS and Android builds, ensuring consistency and coordination across platforms.

## Requirements

### Requirement: Cross-Platform Build Configuration
The build system MUST provide unified configuration that applies to both iOS and Android builds.

#### Scenario: Unified Configuration
- **WHEN** both iOS and Android builds require configuration
- **THEN** the build system MUST provide unified configuration

#### Scenario: Platform-Specific Variations
- **WHEN** different platforms need different configurations
- **THEN** the build system MUST support platform-specific variations within unified framework

### Requirement: Cross-Platform Dependency Management
The build system MUST manage dependencies consistently across both platforms.

#### Scenario: Dependency Specification
- **WHEN** project requires dependency management
- **THEN** the build system MUST specify dependencies for both platforms

#### Scenario: Platform-Specific Dependencies
- **WHEN** dependencies vary between platforms
- **THEN** the build system MUST handle platform-specific dependencies appropriately

### Requirement: Core CPU Move
The Rust core MUST expose a CPU-move function for a game session that returns
one legal move for the side to move at a requested difficulty level:
1 (easy), 2 (medium), or 3 (hard). The returned move MUST be a legal UCI move
for the current position, with promotion moves reported as 5-character UCI
strings exactly like every other move the core reports. The call MUST NOT
mutate the session state, and MUST complete within a bounded time: a
hard-level move from a typical position MUST finish in under one second on
modern consumer devices. At medium and hard difficulty the returned move MUST
be a deterministic function of the position and the difficulty level.

#### Scenario: CPU move is legal and non-mutating
- **WHEN** a CPU move is requested from any position where the side to move has at least one legal move
- **THEN** the returned UCI string is a legal move for that position, and the session's board, side to move, and history are unchanged by the call

#### Scenario: All three difficulty levels are accepted
- **WHEN** a CPU move is requested at difficulty 1, 2, or 3
- **THEN** the request succeeds and returns a legal move for the current position

#### Scenario: Invalid difficulty is an error
- **WHEN** a CPU move is requested at a difficulty outside the 1–3 range
- **THEN** the call returns an error instead of a move and the session is unchanged

#### Scenario: CPU promotion uses full UCI
- **WHEN** the move chosen by the CPU at any difficulty promotes a pawn
- **THEN** the returned UCI string is the 5-character form including the promotion piece

#### Scenario: Medium and hard are deterministic
- **WHEN** the same position is presented to the CPU at medium or hard difficulty
- **THEN** the returned move is the same on every call for that position and difficulty

#### Scenario: Hard move is fast
- **WHEN** a hard-level CPU move is requested from the standard starting position
- **THEN** the move is returned in under one second

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

### Requirement: Online Multiplayer Protocol
The online game MUST run over a single persistent WebSocket connection per
player, speaking versioned JSON: every message MUST be a JSON object with
`"v": 1` and a `type` field. Clients MUST send only the actions the
protocol defines, and the server MUST be the single source of truth for
game state: no client MAY report moves, results, or ratings, and every
client-visible state change MUST arrive as a server message. When the
server rejects an action it MUST answer with a structured error (a stable
machine-readable `code` and a human-readable `message`) and keep the
connection open; it MUST close the connection only for protocol-level
failures (malformed JSON, unknown message type, or unsupported version).
After any accepted action, and immediately after a player re-attaches, the
server MUST send a full state snapshot so that a client can always rebuild
the game screen from a single message.

#### Scenario: Client actions are versioned and structured
- **WHEN** a connected client sends a game action (create room, join room, move, resign, or leave)
- **THEN** the message is JSON with `v: 1` and a recognized `type`, and the server answers with either a confirmation/state message or a structured error

#### Scenario: Malformed or unknown messages break the connection
- **WHEN** a client sends a frame that is not valid JSON, has an unknown `type`, or an unsupported `v`
- **THEN** the server closes the connection with a protocol error and no game state is applied

#### Scenario: Errors carry a stable code
- **WHEN** the server rejects a client action
- **THEN** the error message contains a stable machine-readable code (room_not_found, room_full, already_in_room, invalid_room_code, not_your_turn, illegal_move, game_over, not_connected, or forfeit) and a human-readable message, and the connection stays open

#### Scenario: A state snapshot rebuilds the game screen
- **WHEN** the server sends a state snapshot to a player
- **THEN** the snapshot contains the board FEN, the full move list, the side to move, the game status (in progress, checkmated with winner, drawn, or resigned/forfeited with winner), the player's own color, and both players' current ratings, and a client that applies it can fully redraw the board, move list, and status without any other message

#### Scenario: Reconnect resyncs with a fresh snapshot
- **WHEN** a player reconnects to a room within the server's reconnect window
- **THEN** the server re-attaches that player to the same seat and immediately sends the current state snapshot, and the game continues from that state
