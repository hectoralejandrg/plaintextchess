# Spec Delta

## REMOVED Requirements

### Requirement: iOS App Project
**Reason**: The placeholder milestone is complete; the app now presents a playable
two-player chess game instead of a placeholder screen (see
`add-playable-chess-ui/proposal.md`).
**Migration**: The app-project requirement is re-expressed as the
`iOS Playable Game Screen` requirement in this delta. The XCFramework
requirement and build-configuration requirements in this capability are
unaffected.

## ADDED Requirements

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
