# Spec Delta

## MODIFIED Requirements

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
connected at the flag. When both players are disconnected at the same time,
the server MUST hold each seat for its own grace window and let either
player re-attach and resume; a seat whose grace window expires while the
other player is connected forfeits to the connected player, and if every
grace window expires with no player connected the room is removed with no
result, exactly as an empty room.

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

#### Scenario: Both players disconnect within the grace window
- **WHEN** both players' connections drop before the grace window expires
- **THEN** the server holds each seat for its own grace window, either player may re-attach and resume the game, a seat whose grace expires while the other player is connected forfeits to the connected player, and if both grace windows expire with neither player present the room is removed with no result
