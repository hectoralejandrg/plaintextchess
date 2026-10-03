# android Specification

## Purpose
Define comprehensive build specifications for the Android chess application, including build configuration, deployment procedures, and platform-specific requirements.

## Requirements

### Requirement: Android Build Configuration
The build system MUST specify consistent configuration for Android application builds across different environments.

#### Scenario: Build Configuration Setup
- **WHEN** project requires Android build configuration
- **THEN** the build system MUST provide standardized configuration files

#### Scenario: Environment Configuration
- **WHEN** application needs different build environments (debug, release)
- **THEN** the build system MUST support environment-specific configurations

### Requirement: Android Deployment Specifications
The build system MUST define deployment procedures for Android application distribution.

#### Scenario: Google Play Distribution
- **WHEN** application needs to be distributed via Google Play
- **THEN** the build system MUST provide deployment specifications

#### Scenario: Enterprise Distribution
- **WHEN** application needs enterprise distribution
- **THEN** the build system MUST support enterprise deployment procedures

### Requirement: Performance Build Requirements
The build system MUST enforce performance targets for Android application builds.

#### Scenario: Build Performance Targets
- **WHEN** Android application is being built
- **THEN** the build system MUST meet performance requirements

### Requirement: Android Playable Game Screen
The repository MUST contain an Android application project that bundles the
chess core native libraries for the supported ABIs and, at launch, presents a
playable two-player chess game: an 8×8 board rendered from the core's board
state, piece selection restricted to the core's legal moves, live game status
(whose move, check, checkmate, draw), a move list, and a new-game action. FFI
errors MUST be surfaced in the UI instead of crashing.

#### Scenario: Debug APK contains the native libraries
- **WHEN** the app project is built for debug
- **THEN** the resulting APK contains the chess core shared library for both the aarch64 and x86_64 ABIs

#### Scenario: Game screen renders the initial position
- **WHEN** the app launches on a device or emulator matching a supported ABI
- **THEN** the game screen displays the standard start position on an 8×8 board with the player to move indicated, without crashing, UnsatisfiedLinkError, or FFI errors

#### Scenario: Legal move selection
- **WHEN** the user taps a square occupied by a piece of the player to move
- **THEN** the squares that are legal destinations for that piece are highlighted and no square that is not a legal destination is highlighted

#### Scenario: Playing a legal move
- **WHEN** the user taps a highlighted destination square
- **THEN** the move is executed in the core session, the board updates to the new position, and the move is appended to the move list

#### Scenario: Illegal move is rejected
- **WHEN** the user taps a square that is not a legal destination for the selected piece
- **THEN** the board position does not change and the user is shown that the move is not legal

#### Scenario: Check is surfaced
- **WHEN** a played move leaves the opponent in check
- **THEN** the game screen indicates that the opponent is in check

#### Scenario: Game over by checkmate or draw
- **WHEN** the core session reports checkmate or a draw
- **THEN** the game screen displays the game result and offers a new-game action that starts a fresh session with the standard start position

#### Scenario: FFI failure is rendered in the UI
- **WHEN** a call into the chess core fails
- **THEN** the app displays the error in the UI instead of crashing

### Requirement: Self-Sufficient Android Build via Gradle Wrapper
The Android app project MUST be buildable with a committed Gradle wrapper so that no global Gradle installation is required; a machine with only a compatible JDK and the Android SDK MUST be able to build the app.

#### Scenario: Wrapper build on a clean machine
- **WHEN** the wrapper build entry point is run on a machine without a global Gradle installation
- **THEN** the build completes and produces the app package using the wrapper's pinned Gradle version

### Requirement: Android Pawn Promotion
The Android game screen MUST support pawn promotion: when a pawn's destination
is on the last rank, the app MUST offer a choice between queen, rook, bishop,
and knight, and MUST execute the move in the core session using the full UCI
string with the chosen piece's promotion character.

#### Scenario: Promotion choice is offered on a last-rank destination
- **WHEN** the user selects a pawn and chooses a highlighted destination on the last rank, for which the core reports one move per promotion piece
- **THEN** the app shows a piece selector (queen, rook, bishop, knight) anchored near the destination square and the board position does not change until a piece is chosen

#### Scenario: Promotion plays the full UCI move
- **WHEN** the user picks a piece from the selector
- **THEN** the app calls the core with the full UCI string including the promotion character (e.g. `e7e8q`), the board updates to show the promoted piece on the last rank, and the move is appended to the move list with its promotion character

#### Scenario: Under-promotion is honored
- **WHEN** the user picks a rook, bishop, or knight instead of the queen
- **THEN** the core session plays that promotion and the board shows the corresponding promoted piece on the last rank

#### Scenario: Promotion choice is cancellable
- **WHEN** the user dismisses the selector without picking a piece
- **THEN** no move is played, the board position does not change, and the previous selection is cleared

### Requirement: Android Piece Drag and Drop
The Android game screen MUST support moving pieces by dragging in addition to
the existing two-tap selection: the user MUST be able to press and drag a
piece of the side to move, and the move MUST be played when the piece is
released over a legal destination.

#### Scenario: Dragging lifts the piece and highlights legal destinations
- **WHEN** the user touches down on a square occupied by a piece of the side to move and moves the finger
- **THEN** the piece follows the drag (lifted above the board) and that piece's legal destinations are highlighted, with no square that is not a legal destination highlighted

#### Scenario: Dropping on a legal destination plays the move
- **WHEN** the user releases the dragged piece over a highlighted destination square
- **THEN** the move is executed in the core session exactly as with tap selection: the board updates, the move is appended to the move list, and the game status refreshes

#### Scenario: Dropping on an illegal square is rejected
- **WHEN** the user releases the dragged piece over a square that is not a legal destination
- **THEN** no move is played, the board position does not change, the piece returns to its origin square, and the user is shown that the move is not legal

#### Scenario: Dragging a pawn to the last rank opens the promotion selector
- **WHEN** the user releases a dragged pawn over a last-rank destination
- **THEN** the promotion piece selector is shown, and the move is played only after a piece is chosen

#### Scenario: Tap selection still works alongside drag
- **WHEN** the user taps a piece without dragging
- **THEN** the two-tap selection behavior is unchanged (select, highlight, tap destination to play)

### Requirement: Android Move Animation
The Android game screen MUST animate played moves with a short slide: the
moved piece MUST travel from its origin square to its destination square, and
the animation MUST be skipped when the user has disabled system animations.

#### Scenario: A played move slides the piece
- **WHEN** a move is played by tap or by drag-drop
- **THEN** the moved piece visibly slides from the origin square to the destination square over a short duration (~0.15–0.25 s) instead of teleporting, and the board is correct once the animation ends

#### Scenario: System animations disabled skip the slide
- **WHEN** the user has disabled system animations (animation duration scale set to none) and a move is played
- **THEN** the piece appears on the destination square immediately, without the slide animation

### Requirement: Android Game Mode Selection
The Android game screen MUST let the player choose, when starting a new game,
whether the game is two players or against the CPU, and, when against the CPU,
which difficulty to use (easy, medium, or hard). Two players MUST be the
default selection and MUST keep the existing two-player behavior unchanged.
The chosen mode and difficulty MUST be fixed for the duration of that game.

#### Scenario: New game offers the mode choice
- **WHEN** the player activates the New game action
- **THEN** the app presents a mode choice (two players or CPU) with two players pre-selected, and when CPU is selected it also presents a difficulty choice (easy, medium, hard)

#### Scenario: Two-player game starts unchanged
- **WHEN** the player confirms a new game in two-player mode
- **THEN** the game starts from the standard initial position with the same behavior as before this change, and both sides are played by the players

#### Scenario: The choice is fixed per game
- **WHEN** a game is started in CPU mode at a given difficulty
- **THEN** that mode and difficulty are used for the whole game and only change when the player starts another new game

### Requirement: Android CPU Opponent
In a game against the CPU, the CPU MUST play the black side. When it is the
CPU's turn, the app MUST indicate that the CPU is thinking, MUST not accept
board input for any move, MUST compute the CPU's move in the core off the
main thread, and MUST play the resulting move through the same move path as
human moves so that the move list entry, the last-move highlight, and the
slide animation behave identically. If a new game is started while the CPU is
thinking, the pending CPU move MUST be discarded and MUST NOT be applied to
the new game. The CPU MUST make no further moves once the game is over.

#### Scenario: CPU responds to a human move
- **WHEN** in a CPU game the player plays a legal move
- **THEN** the app shows the thinking state and the CPU plays a legal black move, the board updates to the new position, and the move is appended to the move list

#### Scenario: Input is disabled while thinking
- **WHEN** it is the CPU's turn and its move is being computed
- **THEN** tapping or dragging any piece produces no move, no selection, and no error

#### Scenario: CPU move animates like a human move
- **WHEN** the CPU plays its move
- **THEN** the moved piece slides from its origin square to its destination square exactly as for a human move, and the board is correct once the animation ends

#### Scenario: New game discards a pending CPU move
- **WHEN** the player starts a new game while the CPU is computing a move
- **THEN** the in-flight move is not applied to the new game and the new game starts from the standard initial position

#### Scenario: CPU respects the end of the game
- **WHEN** the game reaches checkmate, stalemate, or another draw condition
- **THEN** the app displays the game result, and the CPU makes no further moves

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
