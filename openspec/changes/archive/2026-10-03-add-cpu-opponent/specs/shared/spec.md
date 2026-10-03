# Spec Delta

## ADDED Requirements

### Requirement: Core CPU Move
The Rust core MUST expose a CPU-move function for a game session that returns
one legal move for the side to move at a requested difficulty level:
1 (easy), 2 (medium), or 3 (hard). The returned move MUST be a legal UCI move
for the current position, with promotion moves reported as 5-character UCI
strings exactly like every other move the core reports. The call MUST NOT
mutate the session state, and MUST complete within a bounded time: a
hard-level move from a typical position MUST finish in under one second on
modern consumer devices. At medium and hard difficulty the returned move MUST
be a deterministic function of the position and the difficulty level.

#### Scenario: CPU move is legal and non-mutating
- **WHEN** a CPU move is requested from any position where the side to move has at least one legal move
- **THEN** the returned UCI string is a legal move for that position, and the session's board, side to move, and history are unchanged by the call

#### Scenario: All three difficulty levels are accepted
- **WHEN** a CPU move is requested at difficulty 1, 2, or 3
- **THEN** the request succeeds and returns a legal move for the current position

#### Scenario: Invalid difficulty is an error
- **WHEN** a CPU move is requested at a difficulty outside the 1–3 range
- **THEN** the call returns an error instead of a move and the session is unchanged

#### Scenario: CPU promotion uses full UCI
- **WHEN** the move chosen by the CPU at any difficulty promotes a pawn
- **THEN** the returned UCI string is the 5-character form including the promotion piece

#### Scenario: Medium and hard are deterministic
- **WHEN** the same position is presented to the CPU at medium or hard difficulty
- **THEN** the returned move is the same on every call for that position and difficulty

#### Scenario: Hard move is fast
- **WHEN** a hard-level CPU move is requested from the standard starting position
- **THEN** the move is returned in under one second
