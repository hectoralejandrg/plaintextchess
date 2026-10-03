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
