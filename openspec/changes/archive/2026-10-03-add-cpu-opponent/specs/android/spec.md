# Spec Delta

## ADDED Requirements

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
