# Spec Delta

## MODIFIED Requirements

### Requirement: iOS Game Mode Selection
The iOS game screen MUST let the player choose, when starting a new game,
whether the game is two players, against the CPU, or online. When the game
is against the CPU, the player MUST choose which difficulty to use (easy,
medium, or hard). When the game is online, the opponent is the player on
another device connected to the server, and the player's color MUST be
assigned by the server. Two players MUST be the default selection and MUST
keep the existing two-player behavior unchanged. The chosen mode and
difficulty MUST be fixed for the duration of that game.

The New game action MUST be available only while the current game is
finished (checkmate, draw, or resignation), while no move of the current
game has been played yet, or while the current game is in an error state
(FFI failure, as a recovery action). While a game with at least one played
move is in progress, the New game action MUST be unavailable, MUST be shown
in a disabled state, and MUST NOT start a new game when activated.

#### Scenario: New game offers the mode choice
- **WHEN** the player activates the New game action while it is available
- **THEN** the app presents a mode choice (two players, CPU, or online) with two players pre-selected, and when CPU is selected it also presents a difficulty choice (easy, medium, hard)

#### Scenario: Two-player game starts unchanged
- **WHEN** the player confirms a new game in two-player mode
- **THEN** the game starts from the standard initial position with the same behavior as before this change, and both sides are played by the players

#### Scenario: The choice is fixed per game
- **WHEN** a game is started in CPU mode at a given difficulty
- **THEN** that mode and difficulty are used for the whole game and only change when the player starts another new game

#### Scenario: New game is unavailable mid-game
- **WHEN** the current game has at least one move in the move list and the game is not finished
- **THEN** the New game action is shown disabled, and activating it does not start a new game: the move list, board, and status remain those of the current game

#### Scenario: New game becomes available again
- **WHEN** the current game is finished, or all of its moves have been taken back so that no move has been played
- **THEN** the New game action is available again and starting it begins a fresh game

#### Scenario: Online mode shows the connection options
- **WHEN** the player selects online in the new-game sheet
- **THEN** the app shows two options: create a new game, which connects to the server and shows the room code to share while waiting for the opponent, or join an existing game, which lets the player enter a room code

#### Scenario: Starting an online game defers local play to the server
- **WHEN** the player confirms create or join for an online game
- **THEN** the app connects to the online server, no local game is started, and the board and move list show the waiting or server state until the online game begins

### Requirement: iOS Game-End Dialog
When a game ends, the iOS game screen MUST present a modal the moment the
game status becomes terminal (checkmate, draw, or resignation). The modal
MUST make the result unambiguous: in a two-player game the outcome MUST state
which color won on checkmate ("Checkmate! White wins." / "Checkmate! Black
wins."), which side resigned on resignation ("White resigns. Black wins." /
"Black resigns. White wins."), or that the game was drawn; in a game against
the CPU the outcome MUST be stated from the player's perspective (the player
always plays White), so a White checkmate MUST say the player won, a Black
checkmate MUST say the player lost, a resignation MUST say the player lost,
and a draw MUST say the game was drawn. In an online game the outcome MUST
be stated from the player's perspective, using the color the server assigned
to the player: a win MUST say the player won (including a win by the
opponent's forfeit), a loss MUST say the player lost, and a draw MUST say
the game was drawn. The modal MUST appear exactly once
per game: after the player dismisses it, it MUST NOT reappear while the game
remains in that terminal state, and the game's status line MUST keep showing
the result. In local games (two players or CPU) the modal MUST offer a
"Play again" action that starts a new game in the same mode (and CPU
difficulty, when applicable) and a dismissal action that closes the modal
without changing the game; in an online game the modal MUST offer only the
dismissal action, because starting another online game requires a new room.
While the modal is visible it
MUST lock the screen behind it, and board input on the final position MUST
remain ignored as it is at any terminal status.

#### Scenario: Checkmate in a two-player game
- **WHEN** a two-player game reaches checkmate
- **THEN** a modal appears stating the result, naming the winning color (e.g. "Checkmate! White wins."), and offering "Play again" and a dismissal action

#### Scenario: CPU game the player loses by checkmate
- **WHEN** in a game against the CPU the CPU (Black) delivers checkmate
- **THEN** a modal appears stating that the player lost the game

#### Scenario: Resignation ends the game with the result modal
- **WHEN** the player resigns a two-player game
- **THEN** the game ends immediately and a modal appears stating which side resigned and which side won (e.g. "White resigns. Black wins.")

#### Scenario: Resigning a CPU game
- **WHEN** in a game against the CPU the player resigns
- **THEN** the game ends immediately, any in-flight CPU move is discarded, and a modal appears stating that the player lost the game

#### Scenario: Draw
- **WHEN** a game ends in a draw (stalemate or another draw condition)
- **THEN** a modal appears stating that the game was drawn

#### Scenario: Dialog appears once
- **WHEN** the player dismisses the game-end modal and the game remains in its terminal state
- **THEN** the modal does not reappear, and the status line still shows the result

#### Scenario: Play again restarts in the same mode
- **WHEN** the player taps "Play again" in the game-end modal of a local game (two players or CPU) started in a given mode
- **THEN** a new game starts from the standard initial position in that same mode, and a new game that later ends presents a fresh game-end modal

#### Scenario: Online game result modal
- **WHEN** an online game ends by checkmate, resignation, forfeit, or draw
- **THEN** the modal appears stating the result from the player's perspective ("You won.", "You lost.", or "Draw.") and offers only the dismissal action

### Requirement: iOS Game Controls
The iOS game screen MUST offer three game controls with consistent availability:
"Resign", "Undo", and "Flip board".

**Resign**: during play, tapping Resign MUST end the game immediately. In a
two-player game the side to move resigns and the opponent is the winner; in a
game against the CPU the human (White) resigns and Black is the winner. A
resigned game is terminal: the CPU MUST make no further moves, board input
MUST be ignored, and the game-end modal MUST be presented.

**Undo**: during play, tapping Undo MUST take back the last move. In a
two-player game exactly one move is removed from the move list; in a game
against the CPU the last pair (the human's move and the CPU's reply) is
removed so that it is again the human's turn. When the CPU is computing a
move, tapping Undo MUST discard the in-flight CPU move and take back the
human's last move. After undo the board, move list, last-move highlight, and
game status MUST reflect the position before the taken-back move. Undo MUST
be unavailable when the move list is empty or when the game is over.

**Flip board**: tapping Flip board MUST render the board rotated 180° so the
opposite color's pieces are on the bottom edge, with the rank and file
coordinates following the squares. Flipping MUST NOT change the game state,
and all board interactions (two-tap selection, drag & drop, legal-move
highlighting, promotion) MUST keep working identically in both orientations.
The orientation MUST persist when a new game starts.

In an online game the availability differs from local play: Undo MUST be
unavailable for the whole game, Resign MUST be available at any point while
the game is in progress (including while the opponent has the turn), and
Flip board MUST behave exactly as in local games. While the game is
connected but it is not the player's turn, tapping Resign resigns the game
for the player; board input that would claim the opponent's move MUST be
ignored.

While a game is over, Resign and Undo MUST be unavailable.

#### Scenario: Resign ends a two-player game
- **WHEN** in a two-player game with White to move the player taps Resign
- **THEN** the game is over immediately, the status line shows that White resigned and Black wins, and the game-end modal states "White resigns. Black wins."

#### Scenario: Resign while the CPU is thinking
- **WHEN** in a CPU game the player taps Resign while the CPU is computing its move
- **THEN** the in-flight CPU move is discarded, the game ends with the human as the loser, and the game-end modal states that the player lost

#### Scenario: Undo takes back the last move in two-player mode
- **WHEN** in a two-player game with at least one move played the player taps Undo
- **THEN** exactly the last move is removed from the move list and the board, last-move highlight, and status reflect the position before that move

#### Scenario: Undo takes back a pair in CPU mode
- **WHEN** in a CPU game the human has played a move, the CPU has replied, and the player taps Undo
- **THEN** both moves are removed from the move list, the board returns to the position before the human's move, and it is the human's turn again with no CPU move pending

#### Scenario: Undo while the CPU is thinking
- **WHEN** in a CPU game the player taps Undo while the CPU is computing its move
- **THEN** the in-flight CPU move is discarded, the human's last move is taken back, the board shows the position before that move, and it is the human's turn

#### Scenario: Undo unavailable with nothing to undo or a finished game
- **WHEN** the move list is empty, or the game has ended (checkmate, draw, or resignation)
- **THEN** the Undo control is not available

#### Scenario: Flip rotates the board without changing the game
- **WHEN** the player taps Flip board during a game
- **THEN** the board is rendered rotated 180° with the coordinates on the opposite edges, the game state (position, side to move, move list) is unchanged, and a second tap restores the original orientation

#### Scenario: Board interactions work in both orientations
- **WHEN** the board is flipped and the player selects a piece by tap or drag, including a pawn move to the last rank
- **THEN** legal destinations are highlighted correctly, the move plays exactly as in the unflipped orientation, and the promotion selector still appears when required

#### Scenario: Undo is unavailable in online games
- **WHEN** the player is in an online game, whether or not moves have been played and regardless of whose turn it is
- **THEN** the Undo control is not available

#### Scenario: Resign is available during the opponent's turn
- **WHEN** in an online game it is the opponent's turn and the player taps Resign
- **THEN** the app resigns the online game, the server ends the game with the player as the loser, and the game-end modal states that the player lost

## ADDED Requirements

### Requirement: iOS Online Multiplayer
The iOS game screen MUST support playing an online game against another
device: the player MUST be able to create a game (the app connects, shows
the room code, and waits for the opponent to join) or to join an existing
game by entering a room code. After the game starts, the app MUST render
the game from the server's authoritative state: a move MUST be applied
locally only after the server confirms it, and the board, move list,
last-move highlight, status line, and game-end dialog MUST reflect the
server's state. The app MUST show whose turn it is and clearly indicate
when it is the player's own turn. When the player plays a move (by tap or
drag, including pawn promotion through the existing piece selector), the
app MUST send the full UCI move to the server and MUST show an error and
keep the server's position when the server rejects it. When the connection
drops during a game, the app MUST show a reconnecting state, keep retrying
while the server's reconnect window is open, re-attach to the same seat
when possible, and resume from the server's state snapshot. The app MUST
store a persistent anonymous device identifier (generated on first launch)
that identifies the player to the server across launches. The server URL
MUST be a constant in code, overridable in DEBUG builds via a launch
argument, so verification can point at a local server; in release builds
the override MUST be inert.

#### Scenario: Creating an online game shows the room code
- **WHEN** the player chooses online and selects create
- **THEN** the app connects to the server, displays the 6-character room code with a copy action, and shows the standard start position with a waiting state until the opponent joins

#### Scenario: Joining an online game with the room code
- **WHEN** the player selects join and enters an existing room's code
- **THEN** the app joins the room, the game starts with the creator playing White and the player playing Black, and the board renders from the server's state snapshot

#### Scenario: An invalid room code is an error
- **WHEN** the player enters a room code that does not exist, is malformed, or belongs to a full room
- **THEN** the app shows the corresponding error (room not found, room full), the player stays on the join screen, and can correct the code or create a game instead

#### Scenario: A move is sent to the server and applied on confirmation
- **WHEN** it is the player's turn and the player plays a move, including a pawn promotion chosen from the existing piece selector
- **THEN** the app sends the full UCI move to the server, and the move appears on the board, in the move list, and in the last-move highlight only after the server confirms it

#### Scenario: A rejected move shows the error without changing the position
- **WHEN** the server rejects the player's move (illegal move, or not the player's turn)
- **THEN** the app shows the error, the board and move list remain exactly in the server's state, and the selection is cleared

#### Scenario: The turn indicator shows whose move it is
- **WHEN** the online game is in progress
- **THEN** the status area shows whose turn it is and clearly marks the moments when it is the player's own turn

#### Scenario: A connection drop shows reconnecting and re-attaches
- **WHEN** the WebSocket disconnects during an online game
- **THEN** the app shows a reconnecting banner over the board, keeps retrying automatically, and when the re-attachment succeeds the game resumes from the server's state snapshot with the move list intact

#### Scenario: A forfeit by timeout ends the game locally
- **WHEN** the opponent stays disconnected beyond the server's reconnect window
- **THEN** the server reports a forfeit result and the app shows the game-end modal stating that the player won

#### Scenario: DEBUG builds accept a custom server URL
- **WHEN** the DEBUG build is launched with the online-URL override launch argument
- **THEN** the app uses that server URL for online games; in release builds the argument has no effect

### Requirement: iOS Board Layout Stability
The 8x8 board on the iOS game screen MUST always span the full width of the
screen and keep that exact size in every game state. The board's size MUST be
derived solely from the screen width and MUST NOT shrink, grow, or otherwise
change when the surrounding content changes: the online waiting banner
appearing or disappearing, an error message appearing or clearing, the move
list growing (including reaching its display cap and scrolling), the
reconnecting banner overlaying the board, or the game-end modal being shown
or dismissed. The layout MUST keep every element visible (no clipping) in
the worst simultaneous state: the full-width board together with the move
list at its display cap and an error message.

#### Scenario: Board size is identical across screen states
- **WHEN** the game screen shows, in sequence, a fresh local game, a long local game with a capped move list and an error message, an online waiting state with the room-code banner, an online in-progress state, and the reconnecting banner over the board
- **THEN** the rendered board has exactly the same size in every one of those states, spanning the full width of the screen in each of them

#### Scenario: The full-width board never causes clipping
- **WHEN** the screen shows the worst simultaneous content: the full-width board, the move list at its display cap, and an error message
- **THEN** every element of the screen (status line, board, move list, error message, and the control buttons including New game) remains visible without clipping
