# Spec Delta

## MODIFIED Requirements

### Requirement: iOS Game Mode Selection
The iOS game screen MUST let the player choose, when starting a new game,
whether the game is two players or against the CPU, and, when against the CPU,
which difficulty to use (easy, medium, or hard). Two players MUST be the
default selection and MUST keep the existing two-player behavior unchanged.
The chosen mode and difficulty MUST be fixed for the duration of that game.

The New game action MUST be available only while the current game is
finished (checkmate, draw, or resignation), while no move of the current
game has been played yet, or while the current game is in an error state
(FFI failure, as a recovery action). While a game with at least one played
move is in progress, the New game action MUST be unavailable, MUST be shown
in a disabled state, and MUST NOT start a new game when activated.

#### Scenario: New game offers the mode choice
- **WHEN** the player activates the New game action while it is available
- **THEN** the app presents a mode choice (two players or CPU) with two players pre-selected, and when CPU is selected it also presents a difficulty choice (easy, medium, hard)

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

### Requirement: iOS CPU Opponent
In a game against the CPU, the CPU MUST play the black side. When it is the
CPU's turn, the app MUST indicate that the CPU is thinking, MUST not accept
board input for any move, MUST compute the CPU's move in the core off the
main thread, and MUST play the resulting move through the same move path as
human moves so that the move list entry, the last-move highlight, and the
slide animation behave identically. Because a new game cannot be started
while a game is in progress (iOS Game Mode Selection), a pending CPU move
MUST be discarded when the game is ended by resignation or when the move
that triggered it is taken back, and MUST NOT be applied to the game that
follows. The CPU MUST make no further moves once the game is over.

#### Scenario: CPU responds to a human move
- **WHEN** in a CPU game the player plays a legal move
- **THEN** the app shows the thinking state and the CPU plays a legal black move, the board updates to the new position, and the move is appended to the move list

#### Scenario: Input is disabled while thinking
- **WHEN** it is the CPU's turn and its move is being computed
- **THEN** tapping or dragging any piece produces no move, no selection, and no error

#### Scenario: CPU move animates like a human move
- **WHEN** the CPU plays its move
- **THEN** the moved piece slides from its origin square to its destination square exactly as for a human move, and the board is correct once the animation ends

#### Scenario: Resigning discards a pending CPU move
- **WHEN** in a CPU game the player resigns while the CPU is computing its reply to the player's last move
- **THEN** the in-flight CPU move is not applied, the move list contains only the moves that were played before the resignation, and the CPU reply never appears in the move list or in the game that follows

#### Scenario: New game discards a pending CPU move
- **WHEN** a new game is started while a CPU move is still in flight (the New game gate prevents this from user action; the system MUST guarantee it defensively)
- **THEN** the in-flight move is not applied to the new game, and the new game starts from the standard initial position

#### Scenario: CPU respects the end of the game
- **WHEN** the game reaches checkmate, stalemate, or another draw condition
- **THEN** the app displays the game result, and the CPU makes no further moves

## ADDED Requirements

### Requirement: iOS Single Active Game Session
The iOS game screen MUST keep at most one active core game session at a
time, and all board rendering, move execution, legal-move queries,
move-list contents, and game status MUST be derived only from that active
session. Any action that starts a new game (New game, Play again) MUST
replace the active session atomically: from that moment on, no move,
result, or in-flight CPU reply from the replaced session MUST be applied
to, or observable in, the active session. Starting a game, undoing a move,
and resigning MUST invalidate any CPU move in flight at that moment, so
its result is never applied to the session that follows. An undo that
rebuilds the session MUST replace the active session only when the replay
of the kept moves succeeds; when the replay fails, the active session and
its bookkeeping MUST remain unchanged.

#### Scenario: New game replaces the session
- **WHEN** the player starts a new game in a state where the New game action is available
- **THEN** the move list is empty, the board shows the standard initial position with White to move, and every move played afterwards affects only the new session

#### Scenario: In-flight CPU reply is discarded when the game ends
- **WHEN** in a CPU game the player resigns while the CPU is computing its reply
- **THEN** the in-flight reply is discarded, the move list shows only the moves that had been played, and the discarded reply does not appear when the next game starts

#### Scenario: Play again starts an isolated session
- **WHEN** the player taps "Play again" in the game-end modal
- **THEN** a new game starts from the standard initial position in the same mode, and no move, result, or pending CPU move from the previous game carries over

#### Scenario: Undo leaves only the rebuilt session
- **WHEN** the player taps Undo during play
- **THEN** the active session is the rebuilt session matching the position before the taken-back move, with no CPU move pending and no trace of the taken-back move in the move list or the board
