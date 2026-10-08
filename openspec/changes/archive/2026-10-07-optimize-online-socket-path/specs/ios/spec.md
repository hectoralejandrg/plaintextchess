# Spec Delta

## MODIFIED Requirements

### Requirement: iOS Online Multiplayer
The iOS game screen MUST support playing an online game against another
device: the player MUST be able to create a game (the app connects, shows
the room code, and waits for the opponent to join) or to join an existing
game by entering a room code. After the game starts, the app MUST render
the game from the server's authoritative state: a move MUST be applied
locally only after the server confirms it, and the board, move list,
last-move highlight, status line, and game-end dialog MUST reflect the
server's state. The app MUST apply the server's incremental updates by
replaying the move into its local mirror session and updating the clocks,
side to move, and status; it MUST use the full state snapshot only when it
connects or re-attaches. The app MUST show whose turn it is and clearly
indicate when it is the player's own turn. When the player plays a move (by
tap or drag, including pawn promotion through the existing piece selector),
the app MUST send the full UCI move to the server and MUST show an error and
keep the server's position when the server rejects it. When the connection
drops during a game, the app MUST show a reconnecting state, keep retrying
while the server's reconnect window is open, re-attach to the same seat
when possible, and resume from the server's full state snapshot. The app
MUST store a persistent anonymous device identifier (generated on first
launch) that identifies the player to the server across launches. The
server URL MUST be a constant in code, overridable in DEBUG builds via a
launch argument, so verification can point at a local server; in release
builds the override MUST be inert.

#### Scenario: Creating an online game shows the room code
- **WHEN** the player chooses online and selects create
- **THEN** the app connects to the server, displays the 6-character room code with a copy action, shows the color the server assigned the creator, and shows the standard start position with a waiting state until the opponent joins

#### Scenario: Joining an online game with the room code
- **WHEN** the player selects join and enters an existing room's code
- **THEN** the app joins the room, the game starts with each player holding the color the server assigned, and the board renders from the server's state snapshot oriented with the player's own color at the bottom

#### Scenario: An invalid room code is an error
- **WHEN** the player enters a room code that does not exist, is malformed, or belongs to a full room
- **THEN** the app shows the corresponding error (room not found, room full), the player stays on the join screen, and can correct the code or create a game instead

#### Scenario: A move is sent to the server and applied on confirmation
- **WHEN** it is the player's turn and the player plays a move, including a pawn promotion chosen from the existing piece selector
- **THEN** the app sends the full UCI move to the server, and the move appears on the board, in the move list, and in the last-move highlight only after the server's update confirms it

#### Scenario: A rejected move shows the error without changing the position
- **WHEN** the server rejects the player's move (illegal move, or not the player's turn)
- **THEN** the app shows the error, the board and move list remain exactly in the server's state, and the selection is cleared

#### Scenario: The turn indicator shows whose move it is
- **WHEN** the online game is in progress
- **THEN** the status area shows whose turn it is and clearly marks the moments when it is the player's own turn

#### Scenario: Incremental updates drive the clocks and status
- **WHEN** the server applies a move or a terminal result
- **THEN** the app replays the update into its mirror session and updates the board, move list, side to move, clocks, and status without waiting for a full snapshot

#### Scenario: A connection drop shows reconnecting and re-attaches
- **WHEN** the WebSocket disconnects during an online game
- **THEN** the app shows a reconnecting banner over the board, keeps retrying automatically, and when the re-attachment succeeds the game resumes from the server's full state snapshot with the move list intact

#### Scenario: A forfeit by timeout ends the game locally
- **WHEN** the opponent stays disconnected beyond the server's reconnect window
- **THEN** the server reports a forfeit result and the app shows the game-end modal stating that the player won

#### Scenario: DEBUG builds accept a custom server URL
- **WHEN** the DEBUG build is launched with the online-URL override launch argument
- **THEN** the app uses that server URL for online games; in release builds the argument has no effect
