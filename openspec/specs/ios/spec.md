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
whether the game is two players, against the CPU, or online. When
the game is against the CPU, the player MUST choose which difficulty to use
(easy, medium, or hard). When the game is online, the opponent is the
player on another device connected to the server, and the player's color
MUST be assigned by the server. When the player creates an online game,
the app MUST let the player pick the room's time control from the
supported presets (15+10, 10+0, 5+0, 3+2, 1+0), and the chosen control
MUST be fixed for that room; when the player joins an existing game, the
app MUST show the room's time control in the waiting state so the player
can see it before the game starts. Two players MUST be the default
selection and MUST keep the existing two-player behavior unchanged. The
chosen mode, difficulty, and time control MUST be fixed for the duration
of that game.

The New game action MUST be available only while the current game is
finished (checkmate, draw, or resignation), while no move of the current
game has been played yet, or while the current game is in an error state
(FFI failure, as a recovery action). While a game with at least one played
move is in progress, the New game action MUST be unavailable, MUST be shown
in a disabled state, and MUST NOT start a new game when activated.

#### Scenario: New game offers the mode choice
- **WHEN** the player activates the New game action while it is available
- **THEN** the app presents a mode choice (two players, CPU, or online) with two players pre-selected, and when CPU is selected it also presents a difficulty choice (easy, medium, hard)

#### Scenario: Two-player game starts unchanged
- **WHEN** the player confirms a new game in two-player mode
- **THEN** the game starts from the standard initial position with the same behavior as before this change, and both sides are played by the players

#### Scenario: The choice is fixed per game
- **WHEN** a game is started in CPU mode at a given difficulty
- **THEN** that mode and difficulty are used for the whole game and only change when the player starts another new game

#### Scenario: New game is unavailable mid-game
- **WHEN** the current game has at least one move in the move list and the game is not finished
- **THEN** the New game action is shown disabled, and activating it does not start a new game: the move list, board, and status remain those of the current game

#### Scenario: New game becomes available again
- **WHEN** the current game is finished, or all of its moves have been taken back so that no move has been played
- **THEN** the New game action is available again and starting it begins a fresh game

#### Scenario: Online mode shows the connection options
- **WHEN** the player selects online in the new-game sheet
- **THEN** the app shows two options: create a new game, which connects to the server and shows the room code to share while waiting for the opponent, or join an existing game, which lets the player enter a room code

#### Scenario: Creating an online game chooses the time control
- **WHEN** the player chooses online, selects create, and picks one of the time-control presets
- **THEN** the room is created with that time control, the waiting state shows the room code together with the chosen time control, and the clocks display the base time of the control

#### Scenario: Starting an online game defers local play to the server
- **WHEN** the player confirms create or join for an online game
- **THEN** the app connects to the online server, no local game is started, and the board and move list show the waiting or server state until the online game begins
### Requirement: iOS CPU Opponent
In a game against the CPU, the CPU MUST play the black side. When it is the
CPU's turn, the app MUST indicate that the CPU is thinking, MUST not accept
board input for any move, MUST compute the CPU's move in the core off the
main thread, and MUST play the resulting move through the same move path as
human moves so that the move list entry, the last-move highlight, and the
slide animation behave identically. Because a new game cannot be started
while a game is in progress (iOS Game Mode Selection), a pending CPU move
MUST be discarded when the game is ended by resignation or when the move
that triggered it is taken back, and MUST NOT be applied to the game that
follows. The CPU MUST make no further moves once the game is over.

#### Scenario: CPU responds to a human move
- **WHEN** in a CPU game the player plays a legal move
- **THEN** the app shows the thinking state and the CPU plays a legal black move, the board updates to the new position, and the move is appended to the move list

#### Scenario: Input is disabled while thinking
- **WHEN** it is the CPU's turn and its move is being computed
- **THEN** tapping or dragging any piece produces no move, no selection, and no error

#### Scenario: CPU move animates like a human move
- **WHEN** the CPU plays its move
- **THEN** the moved piece slides from its origin square to its destination square exactly as for a human move, and the board is correct once the animation ends

#### Scenario: Resigning discards a pending CPU move
- **WHEN** in a CPU game the player resigns while the CPU is computing its reply to the player's last move
- **THEN** the in-flight CPU move is not applied, the move list contains only the moves that were played before the resignation, and the CPU reply never appears in the move list or in the game that follows

#### Scenario: New game discards a pending CPU move
- **WHEN** a new game is started while a CPU move is still in flight (the New game gate prevents this from user action; the system MUST guarantee it defensively)
- **THEN** the in-flight move is not applied to the new game, and the new game starts from the standard initial position

#### Scenario: CPU respects the end of the game
- **WHEN** the game reaches checkmate, stalemate, or another draw condition
- **THEN** the app displays the game result, and the CPU makes no further moves
### Requirement: iOS Game-End Dialog
When a game ends, the iOS game screen MUST present a modal the moment the
game status becomes terminal (checkmate, draw, resignation, or flag fall).
The modal MUST make the result unambiguous: in a two-player game the
outcome MUST state which color won on checkmate ("Checkmate! White wins." /
"Checkmate! Black wins."), which side resigned on resignation ("White
resigns. Black wins." / "Black resigns. White wins."), or that the game was
drawn; in a game against the CPU the outcome MUST be stated from the
player's perspective (the player always plays White), so a White checkmate
MUST say the player won, a Black checkmate MUST say the player lost, a
resignation MUST say the player lost, and a draw MUST say the game was
drawn. In an online game the outcome MUST be stated from the player's
perspective, using the color the server assigned to the player: a win MUST
say the player won (including a win by the opponent's forfeit), a loss
MUST say the player lost, and a draw MUST say the game was drawn. A win or
loss by flag fall MUST be stated as a win or loss on time ("You won on
time." / "You lost on time."), and a flag-fall draw MUST say the game was
drawn. The modal MUST appear exactly once
per game: after the player dismisses it, it MUST NOT reappear while the game
remains in that terminal state, and the game's status line MUST keep showing
the result. In local games (two players or CPU) the modal MUST offer a
"Play again" action that starts a new game in the same mode (and CPU
difficulty, when applicable) and a dismissal action that closes the modal
without changing the game; in an online game the modal MUST offer only the
dismissal action, because starting another online game requires a new room.
While the modal is visible it
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
- **THEN** the game ends immediately, any in-flight CPU move is discarded, and a modal appears stating that the player lost

#### Scenario: Draw
- **WHEN** a game ends in a draw (stalemate or another draw condition)
- **THEN** a modal appears stating that the game was drawn

#### Scenario: Dialog appears once
- **WHEN** the player dismisses the game-end modal and the game remains in its terminal state
- **THEN** the modal does not reappear, and the status line still shows the result

#### Scenario: Play again restarts in the same mode
- **WHEN** the player taps "Play again" in the game-end modal of a local game (two players or CPU) started in a given mode
- **THEN** a new game starts from the standard initial position in that same mode, and a new game that later ends presents a fresh game-end modal

#### Scenario: Online game result modal
- **WHEN** an online game ends by checkmate, resignation, forfeit, flag fall, or draw
- **THEN** the modal appears stating the result from the player's perspective ("You won.", "You lost.", "You won on time.", "You lost on time.", or "Draw.") and offers only the dismissal action

#### Scenario: Online game result by flag fall
- **WHEN** an online game ends because a player's time ran out
- **THEN** the modal states that the player won on time when the opponent's time ran out, that the player lost on time when the player's own time ran out, or "Draw." when the winner's material was insufficient to checkmate, and offers only the dismissal action
### Requirement: iOS Game Controls
The iOS game screen MUST offer three game controls with consistent availability:
"Resign", "Undo", and "Flip board".

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

In an online game the availability differs from local play: Undo MUST be
unavailable for the whole game, Resign MUST be available at any point while
the game is in progress (including while the opponent has the turn), and
Flip board MUST behave exactly as in local games. While the game is
connected but it is not the player's turn, tapping Resign resigns the game
for the player; board input that would claim the opponent's move MUST be
ignored.

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

#### Scenario: Undo is unavailable in online games
- **WHEN** the player is in an online game, whether or not moves have been played and regardless of whose turn it is
- **THEN** the Undo control is not available

#### Scenario: Resign is available during the opponent's turn
- **WHEN** in an online game it is the opponent's turn and the player taps Resign
- **THEN** the app resigns the online game, the server ends the game with the player as the loser, and the game-end modal states that the player lost
### Requirement: iOS Single Active Game Session
The iOS game screen MUST keep at most one active core game session at a
time, and all board rendering, move execution, legal-move queries,
move-list contents, and game status MUST be derived only from that active
session. Any action that starts a new game (New game, Play again) MUST
replace the active session atomically: from that moment on, no move,
result, or in-flight CPU reply from the replaced session MUST be applied
to, or observable in, the active session. Starting a game, undoing a move,
and resigning MUST invalidate any CPU move in flight at that moment, so
its result is never applied to the session that follows. An undo that
rebuilds the session MUST replace the active session only when the replay
of the kept moves succeeds; when the replay fails, the active session and
its bookkeeping MUST remain unchanged.

#### Scenario: New game replaces the session
- **WHEN** the player starts a new game in a state where the New game action is available
- **THEN** the move list is empty, the board shows the standard initial position with White to move, and every move played afterwards affects only the new session

#### Scenario: In-flight CPU reply is discarded when the game ends
- **WHEN** in a CPU game the player resigns while the CPU is computing its reply
- **THEN** the in-flight reply is discarded, the move list shows only the moves that had been played, and the discarded reply does not appear when the next game starts

#### Scenario: Play again starts an isolated session
- **WHEN** the player taps "Play again" in the game-end modal
- **THEN** a new game starts from the standard initial position in the same mode, and no move, result, or pending CPU move from the previous game carries over

#### Scenario: Undo leaves only the rebuilt session
- **WHEN** the player taps Undo during play
- **THEN** the active session is the rebuilt session matching the position before the taken-back move, with no CPU move pending and no trace of the taken-back move in the move list or the board
### Requirement: iOS Online Multiplayer
The iOS game screen MUST support playing an online game against another
device: the player MUST be able to create a game (the app connects, shows
the room code, and waits for the opponent to join) or to join an existing
game by entering a room code. After the game starts, the app MUST render
the game from the server's authoritative state: a move MUST be applied
locally only after the server confirms it, and the board, move list,
last-move highlight, status line, and game-end dialog MUST reflect the
server's state. The app MUST show whose turn it is and clearly indicate
when it is the player's own turn. When the player plays a move (by tap or
drag, including pawn promotion through the existing piece selector), the
app MUST send the full UCI move to the server and MUST show an error and
keep the server's position when the server rejects it. When the connection
drops during a game, the app MUST show a reconnecting state, keep retrying
while the server's reconnect window is open, re-attach to the same seat
when possible, and resume from the server's state snapshot. The app MUST
store a persistent anonymous device identifier (generated on first launch)
that identifies the player to the server across launches. The server URL
MUST be a constant in code, overridable in DEBUG builds via a launch
argument, so verification can point at a local server; in release builds
the override MUST be inert.

#### Scenario: Creating an online game shows the room code
- **WHEN** the player chooses online and selects create
- **THEN** the app connects to the server, displays the 6-character room code with a copy action, shows the color the server assigned the creator, and shows the standard start position with a waiting state until the opponent joins

#### Scenario: Joining an online game with the room code
- **WHEN** the player selects join and enters an existing room's code
- **THEN** the app joins the room, the game starts with each player holding the color the server assigned, and the board renders from the server's state snapshot oriented with the player's own color at the bottom

#### Scenario: An invalid room code is an error
- **WHEN** the player enters a room code that does not exist, is malformed, or belongs to a full room
- **THEN** the app shows the corresponding error (room not found, room full), the player stays on the join screen, and can correct the code or create a game instead

#### Scenario: A move is sent to the server and applied on confirmation
- **WHEN** it is the player's turn and the player plays a move, including a pawn promotion chosen from the existing piece selector
- **THEN** the app sends the full UCI move to the server, and the move appears on the board, in the move list, and in the last-move highlight only after the server confirms it

#### Scenario: A rejected move shows the error without changing the position
- **WHEN** the server rejects the player's move (illegal move, or not the player's turn)
- **THEN** the app shows the error, the board and move list remain exactly in the server's state, and the selection is cleared

#### Scenario: The turn indicator shows whose move it is
- **WHEN** the online game is in progress
- **THEN** the status area shows whose turn it is and clearly marks the moments when it is the player's own turn

#### Scenario: A connection drop shows reconnecting and re-attaches
- **WHEN** the WebSocket disconnects during an online game
- **THEN** the app shows a reconnecting banner over the board, keeps retrying automatically, and when the re-attachment succeeds the game resumes from the server's state snapshot with the move list intact

#### Scenario: A forfeit by timeout ends the game locally
- **WHEN** the opponent stays disconnected beyond the server's reconnect window
- **THEN** the server reports a forfeit result and the app shows the game-end modal stating that the player won

#### Scenario: DEBUG builds accept a custom server URL
- **WHEN** the DEBUG build is launched with the online-URL override launch argument
- **THEN** the app uses that server URL for online games; in release builds the argument has no effect
### Requirement: iOS Board Layout Stability
The 8x8 board on the iOS game screen MUST always span the full width of the
screen and keep that exact size in every game state. The board's size MUST be
derived solely from the screen width and MUST NOT shrink, grow, or otherwise
change when the surrounding content changes: the online waiting banner
appearing or disappearing, an error message appearing or clearing, the move
list growing (including reaching its display cap and scrolling), the
reconnecting banner overlaying the board, or the game-end modal being shown
or dismissed. The layout MUST keep every element visible (no clipping) in
the worst simultaneous state: the full-width board together with the move
list at its display cap and an error message.

#### Scenario: Board size is identical across screen states
- **WHEN** the game screen shows, in sequence, a fresh local game, a long local game with a capped move list and an error message, an online waiting state with the room-code banner, an online in-progress state, and the reconnecting banner over the board
- **THEN** the rendered board has exactly the same size in every one of those states, spanning the full width of the screen in each of them

#### Scenario: The full-width board never causes clipping
- **WHEN** the screen shows the worst simultaneous content: the full-width board, the move list at its display cap, and an error message
- **THEN** every element of the screen (status line, board, move list, error message, and the control buttons including New game) remains visible without clipping
### Requirement: iOS Online Clock UI
The iOS game screen MUST display both players' remaining time in online
games: one clock per player, clearly associated with each side, so that
the opponent's clock and the player's clock are never confusable. The
clock of the side to move MUST be visually emphasized so the active clock
is identifiable at a glance. The displayed time MUST be derived from the
server's state snapshots, which are the source of truth, with smooth local
countdown interpolation between snapshots; each new snapshot MUST
re-synchronize the clocks to its values. While the room is in the lobby
(the opponent has not joined yet) both clocks MUST show the base time of
the chosen time control and neither clock MUST be emphasized as active.
When the reconnecting banner is shown, the clocks MUST keep displaying
their last known times and MUST resume from the re-attached snapshot's
remaining times. The clock area MUST NOT change the board's size: the
board MUST keep the exact full-width size required by iOS Board Layout
Stability in every online state where clocks are visible.

#### Scenario: Clocks display and track the game
- **WHEN** an online game is in progress
- **THEN** both clocks are visible showing the player's and the opponent's remaining time, the clock of the side to move is emphasized, the countdown decreases smoothly between snapshots, and each snapshot re-synchronizes both clocks

#### Scenario: The lobby shows the base time
- **WHEN** the player has created a room and is waiting for the opponent to join
- **THEN** both clocks show the base time of the chosen time control, neither clock is emphasized as active, and the waiting state shows the chosen time control next to the room code

#### Scenario: Clocks survive a connection drop
- **WHEN** the connection drops, the reconnecting banner is shown, and the connection is later restored
- **THEN** the clocks keep displaying their last known times while the banner is shown, and when the game resumes they continue from the remaining times in the re-attached state snapshot

#### Scenario: Clocks never change the board size
- **WHEN** the game screen moves between the online waiting state, an in-progress state with active clocks, and the reconnecting banner
- **THEN** the rendered board keeps the exact same full-width size in every one of those states
