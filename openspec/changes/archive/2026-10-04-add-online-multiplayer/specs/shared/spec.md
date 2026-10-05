# Spec Delta

## ADDED Requirements

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
