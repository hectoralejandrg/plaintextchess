# ios Specification

## Purpose
Define comprehensive build specifications for the iOS chess application, including build configuration, deployment procedures, and platform-specific requirements.

## Requirements

### Requirement: iOS Build Configuration
The build system MUST specify consistent configuration for iOS application builds across different environments.

#### Scenario: Build Configuration Setup
- **WHEN** project requires iOS build configuration
- **THEN** the build system MUST provide standardized configuration files

#### Scenario: Environment Configuration
- **WHEN** application needs different build environments (debug, release)
- **THEN** the build system MUST support environment-specific configurations

### Requirement: iOS Deployment Specifications
The build system MUST define deployment procedures for iOS application distribution.

#### Scenario: App Store Deployment
- **WHEN** application needs to be distributed via App Store
- **THEN** the build system MUST provide deployment specifications

#### Scenario: Enterprise Distribution
- **WHEN** application needs enterprise distribution
- **THEN** the build system MUST support enterprise deployment procedures

### Requirement: Performance Build Requirements
The build system MUST enforce performance targets for iOS application builds.

#### Scenario: Build Performance Targets
- **WHEN** iOS application is being built
- **THEN** the build system MUST meet performance requirements

### Requirement: iOS Playable Game Screen
The repository MUST contain an iOS application project that compiles against
`ChessCore.xcframework` and, at launch, presents a playable two-player chess
game: an 8×8 board rendered from the core's board state, piece selection
restricted to the core's legal moves, live game status (whose move, check,
checkmate, draw), a move list, and a new-game action. FFI errors MUST be
surfaced in the UI instead of crashing.

#### Scenario: App build for the simulator
- **WHEN** the app project is built for the iOS simulator
- **THEN** the build completes successfully with the chess core framework linked

#### Scenario: Game screen renders the initial position
- **WHEN** the app launches on a simulator
- **THEN** the game screen displays the standard start position on an 8×8 board with the player to move indicated, without crashing or FFI errors

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

### Requirement: iOS XCFramework Simulator Support
The iOS build MUST produce `ChessCore.xcframework` containing both the device slice and the Apple-simulator slice, so the framework can be linked by app builds targeting either destination.

#### Scenario: Build produces both slices
- **WHEN** the iOS build script runs
- **THEN** the resulting XCFramework contains one device slice and one simulator slice, each with the chess core static library and a valid framework Info.plist

#### Scenario: Simulator app uses the simulator slice
- **WHEN** an app build targets the iOS simulator
- **THEN** the correct simulator slice of the framework is selected and the app runs on the simulator

### Requirement: iOS Pawn Promotion
The iOS game screen MUST support pawn promotion: when a pawn's destination is
on the last rank, the app MUST offer a choice between queen, rook, bishop, and
knight, and MUST execute the move in the core session using the full UCI
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

### Requirement: iOS Piece Drag and Drop
The iOS game screen MUST support moving pieces by dragging in addition to the
existing two-tap selection: the user MUST be able to press and drag a piece of
the side to move, and the move MUST be played when the piece is released over
a legal destination.

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

### Requirement: iOS Move Animation
The iOS game screen MUST animate played moves with a short slide: the moved
piece MUST travel from its origin square to its destination square, and the
animation MUST be skipped when the user has enabled Reduce Motion.

#### Scenario: A played move slides the piece
- **WHEN** a move is played by tap or by drag-drop
- **THEN** the moved piece visibly slides from the origin square to the destination square over a short duration (~0.15–0.25 s) instead of teleporting, and the board is correct once the animation ends

#### Scenario: Reduce Motion disables the slide
- **WHEN** the system Reduce Motion accessibility setting is enabled and a move is played
- **THEN** the piece appears on the destination square immediately, without the slide animation

### Requirement: iOS Game Mode Selection
The iOS game screen MUST let the player choose, when starting a new game,
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

### Requirement: iOS CPU Opponent
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
