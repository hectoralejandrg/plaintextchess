# server Specification

## Purpose
Authoritative online game server: in-memory two-player rooms, core-validated moves, disconnect handling, and per-device Glicko2 ratings.

## Requirements
### Requirement: Server Room Management
The server MUST host two-player online rooms in memory. A player creates a
room and receives a room code: a 6-character code drawn from an unambiguous
alphabet (no easily confused characters such as 0/O or 1/I). When creating
a room the player MUST select one of the supported time controls, and the
room MUST keep that time control for the whole game. The server MUST assign
the creator a color at random (White or Black) and MUST report that color to
the creator when the room is created; a second player joins by presenting
the code and takes the opposite color, and MUST be told the room's time
control when joining. The creator MUST NOT be able to choose its color. A
room MUST hold at most two players. A player (device) MUST be able to belong
to at most one room at a time. Joining an unknown room or a room with a
malformed code MUST be an error, and a room whose players are all gone MUST
be removed from memory.

#### Scenario: Creating a room returns a code
- **WHEN** a player creates a room with one of the supported time controls
- **THEN** the server assigns it a unique 6-character code, seats the creator in a color chosen at random, stores the chosen time control with the room, reports the creator's color in the ready snapshot, and the room waits for an opponent

#### Scenario: Joining with the correct code seats the joiner as Black
- **WHEN** a second player presents an existing room's code
- **THEN** the joiner is seated in the color the creator does not hold, both players receive a state snapshot that includes the room's time control and each player's own color, and the game starts with White to move

#### Scenario: The creator cannot choose its color
- **WHEN** a player creates a room
- **THEN** the creator's color is decided by the server and is not derived from the create request

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
is that player's turn, is a legal move for the current position
(including full UCI promotion strings), and is applied before that
player's time has run out; any other submission MUST be rejected with
not_your_turn, illegal_move, or game_over and MUST leave the session
unchanged. After each accepted move or resignation the server MUST send
both players an incremental update carrying the applied move (when there
is one) and the resulting side to move, status, clocks, and ratings; the
server MUST send a full state snapshot (board, full move list, and the
other snapshot fields) when a player connects, re-attaches after a drop,
or needs a resync. Clients MUST treat the server's messages as the
authoritative source. The server MUST end the game on checkmate (winner is
the mating side), on a core-reported draw, on resignation (winner is the
opponent), on flag fall (winner is the opponent of the player whose time
ran out, or a draw when that opponent's material is insufficient to
checkmate), and on forfeit (winner is the opponent of the timed-out
player), and once ended it MUST reject further moves with game_over.

#### Scenario: A legal move is applied and broadcast
- **WHEN** the player to move submits a legal move
- **THEN** the server applies it to the authoritative session and both players receive an update carrying the applied move and the resulting side to move, clocks, and status

#### Scenario: Illegal or out-of-turn moves are rejected without side effects
- **WHEN** a player submits a move that is not legal, or submits while it is the opponent's turn, or submits after the game has ended
- **THEN** the server answers with the appropriate error (illegal_move, not_your_turn, or game_over), the authoritative session is unchanged, and no update is sent

#### Scenario: Checkmate or a draw ends the game
- **WHEN** a move leaves the opponent checkmated, or the position reaches a draw condition the core reports
- **THEN** the server marks the game ended (checkmated with a winner, or drawn), sends both players the terminal update with the final status and ratings, and rejects further moves with game_over

#### Scenario: Resignation ends the game
- **WHEN** a player resigns during play
- **THEN** the server marks the game ended with the opponent as winner, sends both players the terminal update, and rejects any further moves with game_over

#### Scenario: A move in flight is honored if applied before the flag
- **WHEN** the side to move's time runs out while that player's move is in transit, and the server applies the move before the flag fall is recorded
- **THEN** the move is accepted, the game continues, and both players receive the update with the applied move and the updated remaining times

#### Scenario: Flag fall ends the game with the other side winning
- **WHEN** the side to move's remaining time reaches zero and no move of theirs is applied
- **THEN** the server marks the game ended by flag fall with the opponent as winner, sends both players the terminal update, and rejects further moves with game_over

#### Scenario: A flag fall with insufficient material is a draw
- **WHEN** the side to move's time runs out and the opponent holds insufficient material to deliver checkmate
- **THEN** the server marks the game ended as a draw, sends both players the terminal update, and rejects further moves with game_over

#### Scenario: A connecting or re-attaching player receives a full snapshot
- **WHEN** a player connects to a room or re-attaches after a drop
- **THEN** the server sends the full state snapshot (board, full move list, side to move, status, clocks, and ratings) so the client rebuilds the game without relying on any update it may have missed

### Requirement: Server Connection Resilience
The server MUST bound the outbound buffering of each connection. When a
client cannot consume its updates as fast as they are produced, the server
MUST coalesce pending state updates so the newest authoritative state is
always delivered and the client converges, without accumulating unbounded
memory; one-shot control frames (authentication replies and structured
errors) MUST NOT be dropped. A slow client MUST NOT block the reading of
frames on its own connection or on any other. The server MUST send
periodic keepalive pings so a peer that has stopped responding is detected
and treated as a disconnect without waiting for the whole reconnect
window.

#### Scenario: A slow client cannot grow server memory without bound
- **WHEN** a player's connection stops reading its frames while the game keeps producing updates
- **THEN** the server's pending outbound state for that connection stays bounded (intermediate state updates are coalesced) instead of growing with the number of updates

#### Scenario: A slow client converges to the latest state
- **WHEN** the slow client resumes reading
- **THEN** it receives the newest authoritative state (not necessarily every intermediate one), and its board, move list, and clocks match the server

#### Scenario: Control frames are never coalesced away
- **WHEN** authentication replies or structured errors are queued alongside coalesced state updates
- **THEN** every control frame is delivered, in order relative to the frames around it

#### Scenario: An unresponsive peer is detected by keepalive
- **WHEN** a connected peer stops responding to the server's keepalive pings
- **THEN** the server treats it as disconnected (holding the seat for the grace window as usual) without waiting for the full reconnect window to elapse
### Requirement: Server Disconnect, Reconnect, and Forfeit
A player's disconnection MUST NOT immediately end the game: the server
MUST keep the room for a configurable grace window (default 120 seconds,
settable via the RECONNECT_GRACE_SECS environment variable), during which
the same player can re-attach to the same seat. While one player is
disconnected, the server MUST tell the remaining connected player that the
opponent is disconnected, and the disconnected player's clock MUST keep
counting. When the grace window expires with the player still absent, the
server MUST end the game by forfeit with the connected player as winner.
Re-attachment MUST be limited to the player who occupied the seat, matched
by the player's device identifier. When a player re-attaches, the game
MUST resume with that player's remaining time at the value the server
measured; if the player's time ran out while they were disconnected, the
game MUST end by flag fall when they re-attach (or when the grace window
expires, whichever comes first), with the same outcome as if they had been
connected at the flag.

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

#### Scenario: Re-attach resumes with the remaining time
- **WHEN** a disconnected player re-attaches within the grace window after time has elapsed
- **THEN** the player receives a snapshot carrying their reduced remaining time and the game continues from that time, without any extra time being granted

#### Scenario: A flag that fell while disconnected is settled on re-attach
- **WHEN** a player's time runs out while they are disconnected and they re-attach before the disconnect grace expires
- **THEN** the game ends by flag fall with the opponent as winner (or as a draw when the opponent's material is insufficient to checkmate), exactly as if they had been connected at the flag
### Requirement: Server Online Rating
The server MUST keep a per-player Glicko-2 rating, keyed by the player's
persistent device identifier (spec "Server Persistence"), and load the
full persisted state at startup so ratings survive restarts. Without the
database the server MUST start with the in-memory default (1500 / 200 / 0.06)
for each unseen device; with the database (`DATABASE_URL`; default
`sqlite:./data/chess-server.db`) ratings are loaded at startup from the
`players` table and committed in a single idempotent transaction at game
end. Ratings MUST be included in every state snapshot.

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

#### Scenario: A flag fall updates both ratings
- **WHEN** an online game ends by flag fall
- **THEN** the server updates the winner's rating up and the timed-out player's rating down against each other using the core's rating logic, and both players see the new ratings in the final snapshot
### Requirement: Server Persistence
The server MUST persist the per-device Glicko-2 rating state (rating,
rating deviation, and volatility) and a record of every finished online game
in a local SQLite database (local file, WAL mode). The database location MUST
come from the `DATABASE_URL` environment variable (`sqlite:...`) with a
built-in default (`sqlite:./data/chess-server.db`); the default's parent
`data/` directory MUST be created at startup. The database schema MUST be
managed by versioned migrations that run at startup. Before accepting any
connection, the server MUST load the persisted rating states into the
in-memory rating store. When a game reaches a terminal result, the server
MUST record the finished game in a single idempotent transaction: a game row
(with a UUIDv4 `game_id` as idempotency key, the wire status name, the
optional `"white"`/`"black"` winner for decisive results, both device
identifiers, the move count, each player's pre/post rating, the time control,
and an `ended_at_ms` wall-clock timestamp stamped at commit time), both
players' post-game Glicko-2 state (`ON CONFLICT DO UPDATE` guarded by
`updated_at_ms < excluded.updated_at_ms`), and a `rating_history` row per
player (`INSERT OR IGNORE`, PK `(device_id, game_id)`). A failed commit MUST
be logged and MUST not block the delivery of the terminal snapshot to the
players (design D7: the final snapshot and in-memory ratings are always
delivered; the crash-loss window is documented as bounded to the millisecond
between commit and crash).

#### Scenario: A finished online game survives a server restart
- **WHEN** the server stops and restarts against the same `DATABASE_URL` file
- **THEN** the persisted `players` table reloads both devices' full Glicko-2 state into the in-memory rating store, and the new game starts from those values (not from the 1500 default)

### Requirement: Server Health and Configuration
The server MUST expose an HTTP health endpoint (`GET /healthz`) that
answers 200 while the server is ready to accept WebSocket connections, and
MUST serve the online protocol on a WebSocket endpoint. The bind address,
port, reconnect grace window (`RECONNECT_GRACE_SECS`), and persistence
database location (`DATABASE_URL`, default `sqlite:./data/chess-server.db`)
MUST come from environment configuration with safe built-in defaults, so the
same build can run locally and on a hosting platform. The server MUST start
without a database and terminate cleanly on SIGTERM. The database's parent
directory (default `data/`) MUST be created automatically at startup so the
persistence file can be created on first use.

#### Scenario: The health endpoint reports readiness
- **WHEN** a client requests `GET /healthz`
- **THEN** the server answers 200 with a machine-readable body while it is ready, and the WebSocket endpoint accepts new connections

#### Scenario: Configuration comes from the environment
- **WHEN** the server starts with BIND, PORT, or RECONNECT_GRACE_SECS set
- **THEN** it binds to that address and port and uses that grace window, and without them it uses the built-in defaults
### Requirement: Server Time Control and Clock
The server MUST support a fixed set of time controls for online rooms,
each a base time with a Fischer increment (added to the moving player's
remaining time when they complete a move). The supported presets MUST be
15+10, 10+0, 5+0, 3+2, and 1+0 (base minutes plus increment seconds). The
clock for a room MUST start when the game starts (both seats filled), with
each side at the base time, and MUST run entirely server-side: remaining
time MUST be derived from server timestamps, MUST be unaffected by
client-reported values, and MUST keep counting for a disconnected player.
Every state snapshot MUST carry each player's remaining time in
milliseconds together with the room's time control, so that a client can
display the clocks and a re-attaching client can resume from the last
authoritative time.

#### Scenario: The clock starts when the game starts
- **WHEN** a second player joins a room created with a time control
- **THEN** the clocks start at the base time for both players, and the first (White) clock begins counting down as soon as the game starts

#### Scenario: A completed move adds the increment
- **WHEN** a player completes a move on a control with a nonzero increment
- **THEN** that player's remaining time increases by the increment in addition to keeping their remaining time after the time they spent thinking, as reflected in the next snapshot

#### Scenario: Snapshots carry authoritative remaining time
- **WHEN** the server sends a state snapshot
- **THEN** it includes the remaining time for both players and the room's time control, and clients MUST treat the snapshot values as the source of truth for the clocks
