# Spec Delta

## MODIFIED Requirements

### Requirement: Android Game Mode Selection
The Android game screen MUST let the player choose, when starting a new game,
whether the game is two players, against the CPU, or online. When
the game is against the CPU, the player MUST choose which difficulty to use
(easy, medium, or hard). When the game is online, the opponent is the
player on another device connected to the server, and the player's color
MUST be assigned by the server. When the player creates an online game,
the app MUST let the player pick the room's time control from the
supported presets (15+10, 10+0, 5+0, 3+2, 1+0), and the chosen control
MUST be fixed for that room; when the player joins an existing game, the
app MUST show the room's time control in the waiting state so the player
can see it before the game starts. Two players MUST be the default
selection and MUST keep the existing two-player behavior unchanged. The
chosen mode, difficulty, and time control MUST be fixed for the duration
of that game.

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

#### Scenario: Creating an online game chooses the time control
- **WHEN** the player chooses online, selects create, and picks one of the time-control presets
- **THEN** the room is created with that time control, the waiting state shows the room code together with the chosen time control, and the clocks display the base time of the control

#### Scenario: Starting an online game defers local play to the server
- **WHEN** the player confirms create or join for an online game
- **THEN** the app connects to the online server, no local game is started, and the board and move list show the waiting or server state until the online game begins

### Requirement: Android Game-End Dialog
When a game ends, the Android game screen MUST present a modal the moment
the game status becomes terminal (checkmate, draw, resignation, or flag
fall). The modal MUST make the result unambiguous: in a two-player game
the outcome MUST state which color won on checkmate ("Checkmate! White
wins." / "Checkmate! Black wins."), which side resigned on resignation
("White resigns. Black wins." / "Black resigns. White wins."), or that the
game was drawn; in a game against the CPU the outcome MUST be stated from
the player's perspective (the player always plays White), so a White
checkmate MUST say the player won, a Black checkmate MUST say the player
lost, a resignation MUST say the player lost, and a draw MUST say the game
was drawn. In an online game the outcome MUST be stated from the player's
perspective, using the color the server assigned to the player: a win MUST
say the player won (including a win by the opponent's forfeit), a loss
MUST say the player lost, and a draw MUST say the game was drawn. A win or
loss by flag fall MUST be stated as a win or loss on time ("You won on
time." / "You lost on time."), and a flag-fall draw MUST say the game was
drawn. The modal MUST appear exactly once
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
- **THEN** the game ends immediately, any in-flight CPU move is discarded, and a modal appears stating that the player lost

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
- **WHEN** an online game ends by checkmate, resignation, forfeit, flag fall, or draw
- **THEN** the modal appears stating the result from the player's perspective ("You won.", "You lost.", "You won on time.", "You lost on time.", or "Draw.") and offers only the dismissal action

#### Scenario: Online game result by flag fall
- **WHEN** an online game ends because a player's time ran out
- **THEN** the modal states that the player won on time when the opponent's time ran out, that the player lost on time when the player's own time ran out, or "Draw." when the winner's material was insufficient to checkmate, and offers only the dismissal action

## ADDED Requirements

### Requirement: Android Online Clock UI
The Android game screen MUST display both players' remaining time in
online games: one clock per player, clearly associated with each side, so
that the opponent's clock and the player's clock are never confusable.
The clock of the side to move MUST be visually emphasized so the active
clock is identifiable at a glance. The displayed time MUST be derived from
the server's state snapshots, which are the source of truth, with smooth
local countdown interpolation between snapshots; each new snapshot MUST
re-synchronize the clocks to its values. While the room is in the lobby
(the opponent has not joined yet) both clocks MUST show the base time of
the chosen time control and neither clock MUST be emphasized as active.
When the reconnecting banner is shown, the clocks MUST keep displaying
their last known times and MUST resume from the re-attached snapshot's
remaining times. The clock area MUST NOT change the board's size: the
board MUST keep the exact full-width size in every online state where
clocks are visible.

#### Scenario: Clocks display and track the game
- **WHEN** an online game is in progress
- **THEN** both clocks are visible showing the player's and the opponent's remaining time, the clock of the side to move is emphasized, the countdown decreases smoothly between snapshots, and each snapshot re-synchronizes both clocks

#### Scenario: The lobby shows the base time
- **WHEN** the player has created a room and is waiting for the opponent to join
- **THEN** both clocks show the base time of the chosen time control, neither clock is emphasized as active, and the waiting state shows the chosen time control next to the room code

#### Scenario: Clocks survive a connection drop
- **WHEN** the connection drops, the reconnecting banner is shown, and the connection is later restored
- **THEN** the clocks keep displaying their last known times while the banner is shown, and when the game resumes they continue from the remaining times in the re-attached state snapshot

#### Scenario: Clocks never change the board size
- **WHEN** the game screen moves between the online waiting state, an in-progress state with active clocks, and the reconnecting banner
- **THEN** the rendered board keeps the exact same full-width size in every one of those states
