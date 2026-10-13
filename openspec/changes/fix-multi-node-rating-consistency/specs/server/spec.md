# Spec Delta

## MODIFIED Requirements

### Requirement: Server Online Rating
The server MUST keep a per-player Glicko-2 rating, keyed by the player's
persistent device identifier (spec "Server Persistence"), and load the
full persisted state at startup so ratings survive restarts. Without the
database the server MUST start with the in-memory default (1500 / 200 / 0.06)
for each unseen device; with the database (`DATABASE_URL`; default
`sqlite:./data/chess-server.db`) ratings are loaded at startup from the
`players` table and committed in a single idempotent transaction at game
end. Ratings MUST be included in every state snapshot. In a multi-logic-node
deployment, where each logic node keeps its own independent database, a
device's rating MUST be the same regardless of which logic node currently
owns its game — a cross-node freshness mechanism MUST prevent a node that
has not yet seen a device's most recent game from showing that device a
stale or default rating.

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

#### Scenario: A rating survives a cross-node room placement
- **WHEN** a device's game ends on one logic node, and that device is later seated in a new room owned by a different logic node
- **THEN** the rating shown in that new room reflects the result of the device's previous game, not the default or a stale value from the new node's own database

#### Scenario: A rating mirror failure does not block the game
- **WHEN** the cross-node freshness mechanism cannot be reached (e.g. Redis is unavailable)
- **THEN** the game still completes and the result is still delivered to both players, using each node's own locally known rating, exactly as it behaved before this mechanism existed
