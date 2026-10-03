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
