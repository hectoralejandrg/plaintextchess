# Spec Delta: server

## MODIFIED Requirements

### Requirement: Server Room Management
The server MUST host two-player online rooms in memory. A player creates a
room and receives a room code: a 6-character code drawn from an unambiguous
alphabet (no easily confused characters such as 0/O or 1/I). When creating
a room the player MUST select one of the supported time controls, and the
room MUST keep that time control for the whole game. The creator takes the
White seat; a second player joins by presenting the code and takes the
Black seat, and MUST be told the room's time control when joining. A room
MUST hold at most two players. A player MUST be able to belong to at most
one room at a time: for a player playing as a guest this MUST be enforced
by their device identifier, and for an authenticated player it MUST be
enforced by their account as well as by their device identifier, so one
account cannot occupy two rooms by signing in on two devices. Joining an
unknown room or a room with a malformed code MUST be an error, and a room
whose players are all gone MUST be removed from memory.

#### Scenario: Creating a room returns a code
- **WHEN** a player creates a room with one of the supported time controls
- **THEN** the server assigns it a unique 6-character code, seats the creator as White, stores the chosen time control with the room, and the room waits for an opponent

#### Scenario: Joining with the correct code seats the joiner as Black
- **WHEN** a second player presents an existing room's code
- **THEN** the joiner is seated as Black, both players receive a state snapshot that includes the room's time control, and the game starts with White to move

#### Scenario: A room holds at most two players
- **WHEN** a third player presents a room's code
- **THEN** the server rejects the join with a room_full error and the room is unchanged

#### Scenario: A device belongs to one room at a time
- **WHEN** a device that already sits in a room tries to create or join another room
- **THEN** the server rejects the action with an already_in_room error

#### Scenario: One account cannot occupy two rooms through two devices
- **WHEN** an authenticated player who already sits in a room on one device signs in and tries to create or join another room from a second device
- **THEN** the server rejects the action with an already_in_room error and the existing game is unaffected

#### Scenario: Two guests that share nothing are unrelated
- **WHEN** two different unauthenticated devices each create their own room
- **THEN** both rooms are hosted independently, since neither device is linked to an account that spans them

#### Scenario: Unknown or malformed room codes are rejected
- **WHEN** a player presents a code that does not match any open room, or a code that is not 6 valid characters
- **THEN** the server rejects the join with room_not_found or invalid_room_code and creates no room

#### Scenario: Empty rooms are cleaned up
- **WHEN** all players of a room are gone (the lobby player left, or a finished game's players stop being connected)
- **THEN** the server removes the room and its state from memory

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
by the player's resolved identity rather than by an identifier the client
merely asserts: for a guest that identity is the device identifier, and for
an authenticated player it is the account, so the same account MAY re-attach
from a different device than the one it started on. A client that presents
another player's device identifier without a session proving it is that
account MUST NOT take over the occupied seat and MUST be answered with
`room_full` rather than being handed the seat. When a player re-attaches,
the game MUST resume with that player's remaining time at the value the
server measured; if the player's time ran out while they were disconnected,
the game MUST end by flag fall when they re-attach (or when the grace window
expires, whichever comes first), with the same outcome as if they had been
connected at the flag.

#### Scenario: A disconnected player can re-attach within the grace window
- **WHEN** a player's connection drops and the same device identifier reconnects to the same room before the grace window expires
- **THEN** the server re-attaches that player to the same seat, the game continues unchanged, and the player receives the current state snapshot

#### Scenario: The same account re-attaches from a different device
- **WHEN** a player's connection drops and the same account, authenticated from a different device, reconnects to the same room before the grace window expires
- **THEN** the server re-attaches that player to the same seat and resumes the game with their remaining time measured by the server

#### Scenario: Replaying another player's device identifier does not take the seat
- **WHEN** a connection with no session presents a device identifier that occupies the seat of a game already in progress
- **THEN** the server refuses to re-attach that identity, answers with `room_full`, and leaves the seated player and the game unchanged

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

### Requirement: Server Persistence
The server MUST persist the per-device Glicko-2 rating state (rating,
rating deviation, and volatility) and a record of every finished online game
in a local SQLite database (local file, WAL mode). The database location MUST
come from the `DATABASE_URL` environment variable (`sqlite:...`) with a
built-in default (`sqlite:./data/chess-server.db`); the default's parent
`data/` directory MUST be created at startup. The database schema MUST be
managed by versioned migrations that run at startup, and that schema MUST
also hold the player accounts, device-to-account links, and sessions
described by the `player-auth` capability, created by the same startup
migration mechanism. Before accepting any connection, the server MUST load
the persisted rating states into the in-memory rating store, and MUST load
the persisted accounts, device-to-account links, and sessions so that a
linked device and an active session are still recognized after a restart. When
a game reaches a terminal result, the server MUST record the finished game in
a single idempotent transaction: a game row (with a UUIDv4 `game_id` as
idempotency key, the wire status name, the optional `"white"`/`"black"`
winner for decisive results, both device identifiers, the move count, each
player's pre/post rating, the time control, and an `ended_at_ms` wall-clock
timestamp stamped at commit time), both players' post-game Glicko-2 state
(`ON CONFLICT DO UPDATE` guarded by `updated_at_ms < excluded.updated_at_ms`),
and a `rating_history` row per player (`INSERT OR IGNORE`, PK
`(device_id, game_id)`). A failed commit MUST be logged and MUST not block
the delivery of the terminal snapshot to the players (design D7: the final
snapshot and in-memory ratings are always delivered; the crash-loss window is
documented as bounded to the millisecond between commit and crash). The same
rule MUST govern authentication writes: a failed account, link, or session
commit MUST be logged and MUST NOT leave the connection in an unusable state
and MUST NOT block the player from playing as a guest.

#### Scenario: A finished online game survives a server restart
- **WHEN** the server stops and restarts against the same `DATABASE_URL` file
- **THEN** the persisted `players` table reloads both devices' full Glicko-2 state into the in-memory rating store, and the new game starts from those values (not from the 1500 default)

#### Scenario: Accounts and sessions survive a server restart
- **WHEN** the server stops and restarts against the same `DATABASE_URL` file after a player registered and logged in
- **THEN** the persisted accounts, device-to-account links, and sessions are loaded at startup, and the player's session still authenticates and the device is still linked

#### Scenario: A failed authentication write does not block play
- **WHEN** persisting an account, device link, or session fails
- **THEN** the server logs the failure, keeps the connection usable, and lets the player continue to play as a guest

### Requirement: Server Health and Configuration
The server MUST expose an HTTP health endpoint (`GET /healthz`) that
answers 200 while the server is ready to accept WebSocket connections, and
MUST serve the online protocol on a WebSocket endpoint. The bind address,
port, reconnect grace window (`RECONNECT_GRACE_SECS`), and persistence
database location (`DATABASE_URL`, default `sqlite:./data/chess-server.db`)
MUST come from environment configuration with safe built-in defaults, so the
same build can run locally and on a hosting platform. The authentication
session lifetime and the password hashing cost MUST likewise come from
environment configuration with safe built-in defaults, so an operator can
tighten them without a rebuild; a configured value below the server's
supported minimum MUST fail startup with an explicit configuration error
rather than being silently weakened. The server MUST start without a database
and terminate cleanly on SIGTERM. The database's parent
directory (default `data/`) MUST be created automatically at startup so the
persistence file can be created on first use.

#### Scenario: The health endpoint reports readiness
- **WHEN** a client requests `GET /healthz`
- **THEN** the server answers 200 with a machine-readable body while it is ready, and the WebSocket endpoint accepts new connections

#### Scenario: Configuration comes from the environment
- **WHEN** the server starts with BIND, PORT, or RECONNECT_GRACE_SECS set
- **THEN** it binds to that address and port and uses that grace window, and without them it uses the built-in defaults

#### Scenario: Authentication configuration comes from the environment
- **WHEN** the server starts with the session lifetime or password hashing cost set
- **THEN** it uses those values, and with none of them set it starts successfully using its built-in authentication defaults

#### Scenario: A below-minimum authentication setting stops startup
- **WHEN** the server starts with a configured session lifetime or password hashing cost below the supported minimum
- **THEN** it exits with an explicit configuration error instead of starting with the weaker value
