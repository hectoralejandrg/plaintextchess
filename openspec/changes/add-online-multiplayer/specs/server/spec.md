# Spec Delta

## Purpose
Authoritative online game server: in-memory two-player rooms, core-validated moves, disconnect handling, and per-device Glicko2 ratings.

## ADDED Requirements

### Requirement: Server Room Management
The server MUST host two-player online rooms in memory. A player creates a
room and receives a room code: a 6-character code drawn from an
unambiguous alphabet (no easily confused characters such as 0/O or 1/I).
The creator takes the White seat; a second player joins by presenting the
code and takes the Black seat. A room MUST hold at most two players. A
player (device) MUST be able to belong to at most one room at a time.
Joining an unknown room or a room with a malformed code MUST be an error,
and a room whose players are all gone MUST be removed from memory.

#### Scenario: Creating a room returns a code
- **WHEN** a player creates a room
- **THEN** the server assigns it a unique 6-character code, seats the creator as White, and the room waits for an opponent

#### Scenario: Joining with the correct code seats the joiner as Black
- **WHEN** a second player presents an existing room's code
- **THEN** the joiner is seated as Black, both players receive a state snapshot, and the game starts with White to move

#### Scenario: A room holds at most two players
- **WHEN** a third player presents a room's code
- **THEN** the server rejects the join with a room_full error and the room is unchanged

#### Scenario: A device belongs to one room at a time
- **WHEN** a device that already sits in a room tries to create or join another room
- **THEN** the server rejects the action with an already_in_room error

#### Scenario: Unknown or malformed room codes are rejected
- **WHEN** a player presents a code that does not match any open room, or a code that is not 6 valid characters
- **THEN** the server rejects the join with room_not_found or invalid_room_code and creates no room

#### Scenario: Empty rooms are cleaned up
- **WHEN** all players of a room are gone (the lobby player left, or a finished game's players stop being connected)
- **THEN** the server removes the room and its state from memory

### Requirement: Server Game Authority
Within a room the server MUST own the authoritative game session: the
server applies the moves, and check, checkmate, and draw state MUST come
from the core. A move submitted by a player MUST be accepted only when it
is that player's turn and is a legal move for the current position
(including full UCI promotion strings); any other submission MUST be
rejected with not_your_turn, illegal_move, or game_over and MUST leave the
session unchanged. After each accepted move or resignation the server MUST
send both players a state snapshot. The server MUST end the game on
checkmate (winner is the mating side), on a core-reported draw, on
resignation (winner is the opponent), and on forfeit (winner is the
opponent of the timed-out player), and once ended it MUST reject further
moves with game_over.

#### Scenario: A legal move is applied and broadcast
- **WHEN** the player to move submits a legal move
- **THEN** the server applies it to the authoritative session and both players receive the new state snapshot

#### Scenario: Illegal or out-of-turn moves are rejected without side effects
- **WHEN** a player submits a move that is not legal, or submits while it is the opponent's turn, or submits after the game has ended
- **THEN** the server answers with the appropriate error (illegal_move, not_your_turn, or game_over), the authoritative session is unchanged, and no snapshot is sent

#### Scenario: Checkmate or a draw ends the game
- **WHEN** a move leaves the opponent checkmated, or the position reaches a draw condition the core reports
- **THEN** the server marks the game ended (checkmated with a winner, or drawn), sends both players the final snapshot, and rejects further moves with game_over

#### Scenario: Resignation ends the game
- **WHEN** a player resigns during play
- **THEN** the server marks the game ended with the opponent as winner, sends both players the final snapshot, and rejects any further moves with game_over

### Requirement: Server Disconnect, Reconnect, and Forfeit
A player's disconnection MUST NOT immediately end the game: the server
MUST keep the room for a configurable grace window (default 120 seconds,
settable via the RECONNECT_GRACE_SECS environment variable), during which
the same player can re-attach to the same seat. While one player is
disconnected, the server MUST tell the remaining connected player that the
opponent is disconnected. When the grace window expires with the player
still absent, the server MUST end the game by forfeit with the connected
player as winner. Re-attachment MUST be limited to the player who occupied
the seat, matched by the player's device identifier.

#### Scenario: A disconnected player can re-attach within the grace window
- **WHEN** a player's connection drops and the same device identifier reconnects to the same room before the grace window expires
- **THEN** the server re-attaches that player to the same seat, the game continues unchanged, and the player receives the current state snapshot

#### Scenario: The opponent is told when the other player drops
- **WHEN** one player's connection drops during the grace window
- **THEN** the server tells the connected player that the opponent is disconnected, and the connected player may keep waiting or resign

#### Scenario: Expiring the grace window ends the game by forfeit
- **WHEN** a player stays disconnected beyond the grace window
- **THEN** the server ends the game with the connected player as winner (forfeit), sends the final snapshot, updates the ratings, and removes the room

#### Scenario: A lobby player who disconnects just leaves the room
- **WHEN** the waiting lobby player of a room disconnects, or a join leaves before the game starts
- **THEN** the server removes the room and no result or rating change is recorded

### Requirement: Server Online Rating
The server MUST keep a per-player rating, keyed by the player's persistent
device identifier, using the core's Glicko2 rating logic. A player the
server has not seen before MUST start at 1500. When a game ends, the
server MUST update both players' ratings with the earned point share
against the opponent's rating: 1.0 for the winner, 0.0 for the loser, and
0.5 for both on a draw. Ratings MUST be included in state snapshots for
both players. Ratings live in memory and MUST reset when the server
restarts (documented limitation of the in-memory MVP).

#### Scenario: A new player starts at 1500
- **WHEN** a device identifier the server has not seen before creates or joins a room
- **THEN** the server assigns it the default rating of 1500

#### Scenario: A finished game updates both ratings
- **WHEN** an online game ends by checkmate, resignation, or forfeit
- **THEN** the server updates the winner's rating and the loser's rating against each other using the core's rating logic, and both players see the new ratings in the final snapshot

#### Scenario: A drawn game splits the point
- **WHEN** an online game ends in a draw
- **THEN** the server updates both players' ratings with a 0.5 result against the opponent

#### Scenario: Ratings accumulate across games in one run
- **WHEN** the same device identifier plays several games on the same server run
- **THEN** each game uses the rating the previous game left, so ratings accumulate across games until the server restarts

### Requirement: Server Health and Configuration
The server MUST expose an HTTP health endpoint (`GET /healthz`) that
answers 200 while the server is ready to accept WebSocket connections, and
MUST serve the online protocol on a WebSocket endpoint. The bind address,
port, and reconnect grace window MUST come from environment configuration
(BIND, PORT, RECONNECT_GRACE_SECS) with safe built-in defaults, so the
same build can run locally and on a hosting platform. The server MUST
start without a database and terminate cleanly on SIGTERM.

#### Scenario: The health endpoint reports readiness
- **WHEN** a client requests `GET /healthz`
- **THEN** the server answers 200 with a machine-readable body while it is ready, and the WebSocket endpoint accepts new connections

#### Scenario: Configuration comes from the environment
- **WHEN** the server starts with BIND, PORT, or RECONNECT_GRACE_SECS set
- **THEN** it binds to that address and port and uses that grace window, and without them it uses the built-in defaults
