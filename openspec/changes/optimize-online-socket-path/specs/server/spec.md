# Spec Delta

## MODIFIED Requirements

### Requirement: Server Game Authority
Within a room the server MUST own the authoritative game session: the
server applies the moves, and check, checkmate, and draw state MUST come
from the core. A move submitted by a player MUST be accepted only when it
is that player's turn, is a legal move for the current position
(including full UCI promotion strings), and is applied before that
player's time has run out; any other submission MUST be rejected with
not_your_turn, illegal_move, or game_over and MUST leave the session
unchanged. After each accepted move or resignation the server MUST send
both players an incremental update carrying the applied move (when there
is one) and the resulting side to move, status, clocks, and ratings; the
server MUST send a full state snapshot (board, full move list, and the
other snapshot fields) when a player connects, re-attaches after a drop,
or needs a resync. Clients MUST treat the server's messages as the
authoritative source. The server MUST end the game on checkmate (winner is
the mating side), on a core-reported draw, on resignation (winner is the
opponent), on flag fall (winner is the opponent of the player whose time
ran out, or a draw when that opponent's material is insufficient to
checkmate), and on forfeit (winner is the opponent of the timed-out
player), and once ended it MUST reject further moves with game_over.

#### Scenario: A legal move is applied and broadcast
- **WHEN** the player to move submits a legal move
- **THEN** the server applies it to the authoritative session and both players receive an update carrying the applied move and the resulting side to move, clocks, and status

#### Scenario: Illegal or out-of-turn moves are rejected without side effects
- **WHEN** a player submits a move that is not legal, or submits while it is the opponent's turn, or submits after the game has ended
- **THEN** the server answers with the appropriate error (illegal_move, not_your_turn, or game_over), the authoritative session is unchanged, and no update is sent

#### Scenario: Checkmate or a draw ends the game
- **WHEN** a move leaves the opponent checkmated, or the position reaches a draw condition the core reports
- **THEN** the server marks the game ended (checkmated with a winner, or drawn), sends both players the terminal update with the final status and ratings, and rejects further moves with game_over

#### Scenario: Resignation ends the game
- **WHEN** a player resigns during play
- **THEN** the server marks the game ended with the opponent as winner, sends both players the terminal update, and rejects any further moves with game_over

#### Scenario: A move in flight is honored if applied before the flag
- **WHEN** the side to move's time runs out while that player's move is in transit, and the server applies the move before the flag fall is recorded
- **THEN** the move is accepted, the game continues, and both players receive the update with the applied move and the updated remaining times

#### Scenario: Flag fall ends the game with the other side winning
- **WHEN** the side to move's remaining time reaches zero and no move of theirs is applied
- **THEN** the server marks the game ended by flag fall with the opponent as winner, sends both players the terminal update, and rejects further moves with game_over

#### Scenario: A flag fall with insufficient material is a draw
- **WHEN** the side to move's time runs out and the opponent holds insufficient material to deliver checkmate
- **THEN** the server marks the game ended as a draw, sends both players the terminal update, and rejects further moves with game_over

#### Scenario: A connecting or re-attaching player receives a full snapshot
- **WHEN** a player connects to a room or re-attaches after a drop
- **THEN** the server sends the full state snapshot (board, full move list, side to move, status, clocks, and ratings) so the client rebuilds the game without relying on any update it may have missed

## ADDED Requirements

### Requirement: Server Connection Resilience
The server MUST bound the outbound buffering of each connection. When a
client cannot consume its updates as fast as they are produced, the server
MUST coalesce pending state updates so the newest authoritative state is
always delivered and the client converges, without accumulating unbounded
memory; one-shot control frames (authentication replies and structured
errors) MUST NOT be dropped. A slow client MUST NOT block the reading of
frames on its own connection or on any other. The server MUST send
periodic keepalive pings so a peer that has stopped responding is detected
and treated as a disconnect without waiting for the whole reconnect
window.

#### Scenario: A slow client cannot grow server memory without bound
- **WHEN** a player's connection stops reading its frames while the game keeps producing updates
- **THEN** the server's pending outbound state for that connection stays bounded (intermediate state updates are coalesced) instead of growing with the number of updates

#### Scenario: A slow client converges to the latest state
- **WHEN** the slow client resumes reading
- **THEN** it receives the newest authoritative state (not necessarily every intermediate one), and its board, move list, and clocks match the server

#### Scenario: Control frames are never coalesced away
- **WHEN** authentication replies or structured errors are queued alongside coalesced state updates
- **THEN** every control frame is delivered, in order relative to the frames around it

#### Scenario: An unresponsive peer is detected by keepalive
- **WHEN** a connected peer stops responding to the server's keepalive pings
- **THEN** the server treats it as disconnected (holding the seat for the grace window as usual) without waiting for the full reconnect window to elapse
