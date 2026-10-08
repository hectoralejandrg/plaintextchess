# Spec Delta

## ADDED Requirements

### Requirement: Server Persistence
The server MUST persist the per-device Glicko-2 rating state (rating,
deviation, and volatility) and a record of every finished online game in a
local SQLite database. The database location MUST come from the `DATABASE_URL`
environment variable, with a built-in default that points to a file below the
server's working `data/` directory (created if missing). The database schema
MUST be managed by versioned migrations that run at startup. Before accepting
any connection, the server MUST load the persisted rating states into the
in-memory rating store, so snapshots and rating updates start from the
persisted values. When a game reaches a terminal result, the server MUST
record the finished game in a single transaction: a game row (with a unique
game identifier as idempotency key, the result and winner, the time control,
both device identifiers, the move count, and each player's rating before and
after), the post-game Glicko-2 state of both players, and a rating-history
entry per player. A record commit failure MUST NOT block, delay, or roll back
the game result for the connected players: the final state with the updated
in-memory ratings MUST still be delivered, and the failure MUST be logged.
Recording MUST be idempotent: committing the same finished game twice MUST NOT
change ratings or create a duplicate game row, and a stale commit MUST NOT
overwrite a newer persisted rating state.

#### Scenario: Ratings survive a server restart
- **WHEN** a device plays a finished game, the server process restarts, and the same device identifier reconnects
- **THEN** the device's rating is the value the finished game left (loaded from the database at startup), not the 1500 default

#### Scenario: A finished game is recorded durably
- **WHEN** an online game ends by checkmate, draw, resignation, forfeit, or flag fall
- **THEN** the database contains one game row for that game (result, winner, time control, both device identifiers, move count, ratings before and after) together with both players' post-game Glicko-2 state and a rating-history entry per player, committed in a single transaction

#### Scenario: Committing the same finished game twice changes nothing
- **WHEN** the same finished game's record is committed a second time (for example, a commit retried after a crash)
- **THEN** no duplicate game row is created and no further rating change is applied

#### Scenario: A database failure does not block the game
- **WHEN** the commit of a finished game's record fails because the database is unavailable or corrupt
- **THEN** the connected players still receive the final state with the updated in-memory ratings, and the failure is logged with enough detail to retry or repair

#### Scenario: An unreachable database fails startup
- **WHEN** the server starts and the configured database cannot be opened or its migrations cannot be applied
- **THEN** the server fails to start with a clear error instead of silently running without persistence

## MODIFIED Requirements

### Requirement: Server Online Rating
The server MUST keep a per-player rating, keyed by the player's persistent
device identifier, using the core's Glicko2 rating logic. A player the
server has not seen before — neither in memory nor in the persisted database
— MUST start at 1500. When a game ends, the server MUST update both players'
ratings with the earned point share against the opponent's rating: 1.0 for
the winner, 0.0 for the loser, 0.5 for both on a draw, and a flag-fall result
MUST be scored the same as a win/loss while a flag-fall draw MUST be scored
as a draw. Ratings MUST be included in state snapshots for both players.
Ratings MUST be durable across server restarts: the full Glicko-2 state
(rating, deviation, volatility) of both players MUST be persisted when a game
ends and reloaded at startup (see "Server Persistence"), so a restart MUST NOT
reset ratings to the default.

#### Scenario: A new player starts at 1500
- **WHEN** a device identifier the server has not seen before — neither in memory nor in the database — creates or joins a room
- **THEN** the server assigns it the default rating of 1500

#### Scenario: A finished game updates both ratings
- **WHEN** an online game ends by checkmate, resignation, or forfeit
- **THEN** the server updates the winner's rating and the loser's rating against each other using the core's rating logic, and both players see the new ratings in the final snapshot

#### Scenario: A drawn game splits the point
- **WHEN** an online game ends in a draw
- **THEN** the server updates both players' ratings with a 0.5 result against the opponent

#### Scenario: Ratings accumulate across games in one run
- **WHEN** the same device identifier plays several games on the same server run
- **THEN** each game uses the rating the previous game left, and each finished game's post-game state is persisted (see "Server Persistence")

#### Scenario: Ratings survive a server restart
- **WHEN** the server restarts and a device that played a finished game before the restart reconnects
- **THEN** the device's rating is the value the last finished game left, not the 1500 default

#### Scenario: A flag fall updates both ratings
- **WHEN** an online game ends by flag fall
- **THEN** the server updates the winner's rating up and the timed-out player's rating down against each other using the core's rating logic, and both players see the new ratings in the final snapshot

### Requirement: Server Health and Configuration
The server MUST expose an HTTP health endpoint (`GET /healthz`) that
answers 200 while the server is ready to accept WebSocket connections, and
MUST serve the online protocol on a WebSocket endpoint. The bind address,
port, reconnect grace window, and persistence database location MUST come
from environment configuration (BIND, PORT, RECONNECT_GRACE_SECS,
DATABASE_URL) with safe built-in defaults, so the same build can run locally
and on a hosting platform. The server MUST terminate cleanly on SIGTERM.

#### Scenario: The health endpoint reports readiness
- **WHEN** a client requests `GET /healthz`
- **THEN** the server answers 200 with a machine-readable body while it is ready, and the WebSocket endpoint accepts new connections

#### Scenario: Configuration comes from the environment
- **WHEN** the server starts with BIND, PORT, RECONNECT_GRACE_SECS, or DATABASE_URL set
- **THEN** it binds to that address and port, uses that grace window, and opens that database, using the built-in defaults for anything not set
