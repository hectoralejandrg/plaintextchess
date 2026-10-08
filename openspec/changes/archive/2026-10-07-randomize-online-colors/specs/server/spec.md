# Spec Delta

## MODIFIED Requirements

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
