# Spec Delta

## MODIFIED Requirements

### Requirement: Android Playable Game Screen
The repository MUST contain an Android application project that bundles the
chess core native libraries for the supported ABIs and, at launch, presents a
playable two-player chess game: an 8×8 board rendered from the core's board
state, piece selection restricted to the core's legal moves, live game status
(whose move, check, checkmate, draw), and a new-game action. The game screen
MUST lay the game out as a top bar (the time control and an overflow menu), a
compact horizontal move strip, an opponent row and a player row (each showing
that side's name and, when known, its rating, plus its clock when the game has
a time control), the board, and a bottom action bar. FFI errors MUST be
surfaced in the UI instead of crashing.

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

#### Scenario: The game screen shows the new layout
- **WHEN** a game is on screen
- **THEN** the screen shows a top bar with the time control and an overflow menu, a horizontal move strip, an opponent row above the board and a player row below it, and a bottom action bar

## ADDED Requirements

### Requirement: Android Move History Navigation
The Android game screen MUST let the player browse the played moves. The
move strip MUST list every played move and highlight the ply currently
shown. Tapping a ply, or the previous/next controls, MUST render the board
at that ply. While a position other than the live one is shown, move input
MUST be disabled and the UI MUST indicate that the board is not live; a
control MUST return to the live position. Moves applied afterwards (locally
or from the server) MUST be appended to the strip without changing the ply
being viewed unless the live position was already shown. Navigation MUST
work the same in local, CPU, and online games.

#### Scenario: Browsing to an earlier position
- **WHEN** the player taps the previous control or an earlier ply in the move strip
- **THEN** the board shows that position, the strip highlights that ply, and move input is disabled

#### Scenario: Returning to the live position
- **WHEN** the player advances to the last ply or taps the jump-to-live control
- **THEN** the board shows the live position, the not-live indicator clears, and move input is re-enabled

#### Scenario: Jumping to a specific ply
- **WHEN** the player taps a ply in the move strip
- **THEN** the board renders the position after that ply and the strip highlights it

#### Scenario: New moves while reviewing history
- **WHEN** a move is applied while an earlier position is shown
- **THEN** the move is appended to the move strip, the viewed position does not change, and the UI stays marked as not live until the player returns to live
