# Spec Delta

## ADDED Requirements

### Requirement: Android Game-End Dialog
When a game ends, the Android game screen MUST present a modal the moment the
game status becomes terminal (checkmate, draw, or resignation). The modal
MUST make the result unambiguous: in a two-player game the outcome MUST state
which color won on checkmate ("Checkmate! White wins." / "Checkmate! Black
wins."), which side resigned on resignation ("White resigns. Black wins." /
"Black resigns. White wins."), or that the game was drawn; in a game against
the CPU the outcome MUST be stated from the player's perspective (the player
always plays White), so a White checkmate MUST say the player won, a Black
checkmate MUST say the player lost, a resignation MUST say the player lost,
and a draw MUST say the game was drawn. The modal MUST appear exactly once
per game: after the player dismisses it, it MUST NOT reappear while the game
remains in that terminal state, and the game's status line MUST keep showing
the result. The modal MUST offer a "Play again" action that starts a new game
in the same mode (and CPU difficulty, when applicable) and a dismissal action
that closes the modal without changing the game. While the modal is visible it
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
- **WHEN** the player taps "Play again" in the game-end modal of a game started in a given mode
- **THEN** a new game starts from the standard initial position in that same mode, and a new game that later ends presents a fresh game-end modal

### Requirement: Android Game Controls
The Android game screen MUST offer three game controls with consistent
availability: "Resign", "Undo", and "Flip board".

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
