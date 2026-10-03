# Design: Add game-end dialog and game controls

## Context

The core already reports terminal states: both view models refresh a
`GameStatus` after every move (`refreshStatus()`), producing
`Checkmated(winner: "White"|"Black")` or `Drawn`. Today the result is only
rendered by the small status line. The view models also already know the
`gameMode` (`twoPlayers` vs `cpu(difficulty)`), board input on a terminal
status is already ignored by the existing `select()` guard, and both
`BoardView`s resolve a square from a pixel position through one
square↔pixel mapping each (`FenBoard.squareName(row:col:)` with
`(col + 0.5)*cell, (row + 0.5)*cell`; row 0 = rank 8). No core or FFI change
is involved — this is a pure app-layer feature mirrored on both platforms.

## Goals / Non-Goals

Goals:

- A modal appears the instant the status becomes terminal (checkmate, draw,
  or resignation), with an unambiguous result (win/loss in CPU mode,
  winning color in two-player mode, draw).
- "Play again" restarts in the same mode/difficulty; "Done" just dismisses.
- Exactly one modal per game; deterministic, testable trigger logic.
- Resign, undo, and flip controls that behave consistently on both platforms
  and in both orientations, without changing move handling or the CPU driver.

Non-goals:

- No review of the final position, stats, or rating changes.
- No undo from a terminal position (the game-end modal offers "Play again"
  instead).
- No change to the core, the FFI, or the status line semantics beyond
  rendering the new terminal case.
- No localization (UI stays English, consistent with the existing screens).

## Decisions

### D1: Native modal primitives per platform

- **iOS**: SwiftUI `.alert("Game over", isPresented:)` with a message `Text`
  and two actions — `Button("Play again")` and `Button("Done", role: .cancel)`.
  Alternatives considered: a custom `.sheet` (the New-game pattern) would be
  more style-able, but an alert is the native "message + actions" modal, has
  less new view code, and is intrinsically blocking.
- **Android**: Material 3 `AlertDialog` with `title = "Game over"`, `text`
  result message, `confirmButton = "Play again"`, `dismissButton = "Done"`,
  and `onDismissRequest` mapped to dismissal (scrim tap = "Done").
- Both primitives lock the underlying screen while presented, which satisfies
  the "lock the UI while visible" requirement without extra code.

### D2: VM-owned presentation state (deterministic, dismiss-once)

The view does not derive visibility from the raw status; the VM owns it, so
both platforms share one rule and the "appears once" behavior is testable:

- New state in each VM: `gameEndMessage: String?` (null unless the current
  game is terminal) plus two flags — `gameEndPresented` (the modal has been
  shown for this game) and `gameEndDismissed` (the player closed it).
- Whenever the status becomes terminal (checkmate, draw, **or resignation**)
  and the game has not been presented yet, set `gameEndMessage` and
  `gameEndPresented = true`. This happens in `refreshStatus()` for
  checkmate/draw and in `resign()` for resignation — one shared private
  helper `markTerminalIfNeeded()` keeps the rule in one place.
- `startGame(_:)` resets `gameEndMessage = nil`, `gameEndPresented = false`,
  `gameEndDismissed = false` (alongside its existing resets).
- Dismiss ("Done" / scrim): `gameEndDismissed = true`, `gameEndMessage = nil`.
- "Play again": call `restart()`, which is `startGame(gameMode)` — it reuses
  the last mode and difficulty and, because `startGame` already bumps the
  CPU generation and clears `cpuThinking`, an in-flight CPU move is discarded
  exactly like today's New-game flow.

Why not derive `isPresented` from `status` in the view: a binding
`get { terminal && !dismissed }` re-presents whenever the view recomputes
after dismissal unless a flag survives, and the "Play again" action racing
the binding's `set(false)` across a `startGame` reset can stick
`dismissed = true` on the *new* game. VM-owned state has no race: the flags
are only ever mutated on the main thread by `refreshStatus`, `resign`,
`startGame`, or the dismiss action.

### D3: Result wording (English UI, title always "Game over")

| Situation | Two-player | CPU (human = White, design D6 of `add-cpu-opponent`) |
|---|---|---|
| Checkmate, White wins | "Checkmate! White wins." | "Checkmate! You won!" |
| Checkmate, Black wins | "Checkmate! Black wins." | "Checkmate! You lost." |
| Resignation, White resigns | "White resigns. Black wins." | "You resigned. You lost." |
| Resignation, Black resigns | "Black resigns. White wins." | n/a (the CPU never resigns) |
| Draw | "The game is drawn." | "The game is drawn." |

The message is a computed property on each VM from `status` + `gameMode`
(no storage beyond the flags above), so it can never desync from the status.
The status line gains one case: "White resigns" / "Black resigns".

### D4: Resign is an app-level terminal state

- New status case in both VMs: `resigned(winner: String)`.
- `resign()`:
  - allowed only while the game is playable (`.playing`, including while the
    CPU is thinking — that is the interesting case: the player changes their
    mind mid-ponder).
  - Two-player: the resigner is the side to move; winner = the opponent.
  - CPU: the resigner is the human (White); winner = "Black".
  - Bumps the CPU generation and clears `cpuThinking` (same mechanism as
    `startGame`), so any in-flight CPU move is discarded and never applied.
  - Sets `status = .resigned(winner)` and runs `markTerminalIfNeeded()`, so
    the modal shows through the exact same plumbing as checkmate/draw.
- The CPU driver guards on `.playing`, so a resigned game makes no further
  CPU moves; the board input guards already ignore terminal status.
- Alternatives considered: a core `resign()` FFI — rejected; the core stays
  a pure chess engine, and an app-level terminal status needs no contract
  change and no core rebuild.

### D5: Undo by app-level replay

The core has no undo, so undo is reconstructed:

- `undo()`:
  - allowed only while `.playing` and `moveList` is non-empty (the buttons
    are also disabled otherwise).
  - Plies removed: two-player → 1; CPU → the last pair (2) when the CPU has
    already replied, otherwise 1 (this is the "undo while the CPU is
    thinking" case: the in-flight move is discarded by bumping the
    generation, and the human's last move is taken back).
  - Rebuilds the session: `session = newGameSession(initialRating: 1500.0)`
    followed by replaying the kept UCI moves through the same
    `playMove`-equivalent path (the raw session call + local bookkeeping),
    then re-derives `board`, `toMove`, `lastMove` (the last replayed move or
    `nil`), trims `moveList`, and calls `refreshStatus()`.
- Why replay instead of FEN snapshots or a core `undo()`: the session is the
  single source of truth and every kept UCI was legal in the original game,
  so replay is deterministic and needs no FFI addition; typical game lengths
  make the replay cost negligible. Kept snapshot-per-move would duplicate
  state the core already owns, and a core `undo()` changes the FFI contract
  for a convenience that is fully solvable in the app layer.
- Trade-offs: undo re-renders the board from the replayed FEN without a slide
  animation (acceptable for a take-back); a failed replay (unexpected core
  error) surfaces through the existing `errorMessage` path and leaves the
  move list untouched (the trim happens only after a successful replay).

### D6: Flip board = view-layer orientation

- Each VM holds `boardOrientation: 0 | 180` (default 0 = White on the
  bottom) and a `flipBoard()` that toggles it. `startGame` does NOT reset it
  — orientation is a display preference that persists across new games in
  the session.
- Each `BoardView` renders the grid through a single display mapping:
  for a display cell `(dr, dc)` the square is
  `(row: orientation == 0 ? dr : 7 - dr, col: orientation == 0 ? dc : 7 - dc)`,
  and the point→square inverse uses the same rule. Every existing consumer
  (tap selection, drag lift/drop, legal-target highlighting, promotion
  anchor, coordinate labels) already goes through these two functions, so
  both orientations work without touching gesture or highlighting code.
  Coordinate labels derive from the real square name at the displayed edge,
  so they flip automatically.
- Alternatives considered: rotating the whole board view (e.g.
  `rotationEffect(.degrees(180))` / Compose `graphicsLayer` rotation) —
  rejected: it rotates the coordinate glyphs and markers upside down and
  complicates hit-testing; an index mapping is a two-line change in each
  platform's mapping functions.
- Controls placement (consistent both platforms, mirroring the existing
  "New game" affordance): a row of three secondary buttons — "Undo",
  "Resign", "Flip board" — next to/above the prominent "New game" action.
  Disabled states: Undo when `moveList` is empty or the status is terminal;
  Resign when the status is not `.playing`; Flip is always available.

### D7: iOS DEBUG script tokens for the new controls

The simulator script hook already special-cases tokens (`newgame`,
`cancelpromo`); this change adds `undo`, `resign`, `flip`, `done`, and
`playagain` tokens that call the same view-model entry points the controls
and the alert buttons call (`undo()`, `resign()`, `flipBoard()`,
`dismissGameEnd()`, `restart()`), plus an optional
`-PLAINTCHESS_SCRIPT_INTERVAL <seconds>` launch argument (default 0.5,
release-inert) so scripts can wait out CPU replies (the fixed 0.5 s interval
races the `cpuThinking` guard when the CPU reply is still in flight — a token
fired mid-thinking is ignored). Both are DEBUG-only and release-inert.

### D8: Verification plan (deterministic, scripted)

- **Game-end modal, iOS (simulator, DEBUG hooks)**
  - Two-player Scholar's Mate:
    `-PLAINTCHESS_SCRIPT "e2e4 e7e5 d1h5 b8c6 f1c4 g8f6 h5f7"` →
    "Game over / Checkmate! White wins." alert → screenshot; tap Done →
    alert gone, status line still shows the result.
  - CPU hard "you lost": CPU/Hard +
    `-PLAINTCHESS_SCRIPT "f2f3 g2g4" -PLAINTCHESS_SCRIPT_INTERVAL 2` →
    "Checkmate! You lost." alert; tap "Play again" → fresh board in
    CPU/Hard, no modal.
  - Two-player Fool's Mate `f2f3 e7e5 g2g4 d8h4` → "Checkmate! Black wins."
    (optional).
- **Game-end modal, Android (emulator, real taps)**
  - Two-player Scholar's Mate via the tap map (e2 607,1203 → e4 607,933;
    e7 607,528 → e5 607,798; d1 472,1338 → h5 1012,798; b8 202,393 → c6
    337,663; f1 742,1338 → c4 337,933; g8 877,393 → f6 742,663; h5 1012,798
    → f7 742,528) → dump shows "Game over" + "Checkmate! White wins." + both
    buttons; Done → no dialog, status line still present.
  - CPU/Hard Fool's-Mate loss: New game → CPU chip (423,2095) → Hard chip
    (546,2095) → Start (540,2232); f2 742,1203 → f3 742,1068, poll the dump
    until the CPU reply lands, g2 877,1203 → g4 877,933, poll → dump shows
    "Checkmate! You lost."; "Play again" → board resets in CPU/Hard.
- **Resign, both platforms**
  - Two-player: fresh game (White to move) → Resign → "White resigns. Black
    wins." modal, status line "White resigns"; iOS via script token
    `resign`, Android via tap.
  - CPU: Resign while the CPU is thinking (iOS:
    `-PLAINTCHESS_CPU_DELAY 3` + script "e2e4 resign" with interval 2;
    Android: tap Resign during the "CPU is thinking…" state) →
    "You resigned. You lost."; no in-flight move is applied afterward.
- **Undo, both platforms**
  - Two-player: script "e2e4 e7e5 undo" (iOS) / tap Undo after two moves
    (Android) → move list shows only "1. e2e4", board at that position; a
    second undo → "No moves yet" / empty list, start position.
  - CPU/medium: "e2e4 undo" with interval 2 (iOS) / tap Undo after the CPU
    reply (Android) → move list empty, start position, White to move, no CPU
    move pending.
  - Undo-while-thinking: CPU game with a widened thinking window (iOS
    `-PLAINTCHESS_CPU_DELAY 3`, script "e2e4 undo"; Android: tap Undo during
    "CPU is thinking…") → in-flight move discarded, e2e4 taken back, White to
    move.
  - Disabled states: Undo greyed on a fresh game and on a finished game
    (screenshot on both platforms).
- **Flip, both platforms**
  - Flip on a fresh game → screenshot: Black's pieces on the bottom edge,
    coordinates mirrored; flip again → restored.
  - Flipped + a played move (script "flip e2e4" / tap e2→e4 on the flipped
    board) → the move lands on the correct square; legal targets and the
    last-move highlight are correct.
- **Draw**: no deterministic quick script; the draw path shares the same
  message property and modal plumbing as the other terminal cases, so it is
  verified by code inspection of the computed message plus one manual
  stalemate if time permits.
- **Regression ("normal interactions still work"), both platforms**
  - Two-tap and drag in both orientations (iOS: `-PLAINTCHESS_DRAG "e2,e4"`
    flipped and unflipped; Android: real taps).
  - Promotion picker still opens on a last-rank destination in both
    orientations (iOS 5-char script token; Android tap flow).
  - New game from the setup sheet in both orientations; CPU input lock
    while thinking still holds.
  - Determinism anchor re-run (medium `g8f6 f6e4 b8c6 e7e6`) to prove the CPU
    behavior is untouched.

## Risks / Trade-offs

- [Alert re-presentation after "Play again"] → D2's VM-owned flags:
  `startGame` resets them before the new game can become terminal; the alert
  binding only ever observes `gameEndMessage != nil`.
- [Undo replay diverges from the original session] → every kept UCI was
  legal in the original game and the core is deterministic, so replay is
  exact; an unexpected core error surfaces via `errorMessage` and leaves the
  move list untouched.
- [Android button tap coordinates shift after the new control row lands] →
  the verification tasks re-dump `uiautomator` for the new layout before
  tapping the controls.
- [Flip breaks a consumer that hardcodes geometry] → every square
  resolution goes through the two mapping functions per platform; tasks 1.3
  / 2.3 verify tap, drag, and promotion in both orientations.
- [CPU response time delays screenshots (hard ≈ seconds)] → verification uses
  the script interval argument / dump polling, not fixed races.
- [Word-to-word duplication of the rules on both platforms] → accepted: the
  two VMs are deliberately independent mirrors (established milestone
  pattern); the shared rule is specified once in both spec deltas.

## Migration Plan

None: additive UI behavior, no data or FFI migration. Rollback is a plain
revert of the feature commits; the status line behavior is untouched, so the
result remains visible the same way as before.
