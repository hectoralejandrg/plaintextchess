# Proposal: Add game-end dialog and game controls

## Why

When a game ends (checkmate, draw, or resignation), the result is only visible
in the small status line ("Checkmate! White wins" / "Game drawn"). The player
easily misses it, and in a game against the CPU there is no clear statement of
the personal outcome. The app should present a modal the moment the game ends
that makes the result unambiguous — in particular whether the player won or
lost against the CPU. The game screen also lacks the everyday controls a chess
game offers: resigning the game, taking back the last move, and flipping the
board orientation.

## What Changes

- Game-end modal (iOS + Android): when the game status becomes terminal
  (checkmate, draw, **or resignation**), the app presents a modal with a
  "Game over" title, a result message, and two actions: "Play again" (restart
  a new game in the same mode/difficulty) and "Done" (dismiss). iOS uses a
  native alert; Android a Material 3 `AlertDialog`. The modal appears once per
  game; after dismissal the status line keeps showing the result.
- Result wording:
  - Two-player: "Checkmate! White wins." / "Checkmate! Black wins." /
    "White resigns. Black wins." / "Black resigns. White wins." /
    "The game is drawn."
  - CPU mode (the human always plays White): "Checkmate! You won!" /
    "Checkmate! You lost." / "You resigned. You lost." /
    "The game is drawn."
- Resign action (new): a "Resign" control shown during play. In two-player
  mode the side to move resigns; in CPU mode the human (White) resigns. The
  game ends immediately, any in-flight CPU move is discarded, and the game-end
  modal is shown.
- Undo action (new): an "Undo" control shown during play. In two-player mode
  it takes back the last move; in CPU mode it takes back the last pair
  (the human's move plus the CPU's reply) so it is always the human's turn
  again. While the CPU is thinking, undoing discards the in-flight CPU move
  and takes back the human's last move. It is unavailable with an empty move
  list or once the game is over.
- Flip action (new): a "Flip board" control that rotates the board 180°
  (rank/file coordinates follow the squares). Default is White on the bottom;
  the orientation persists across new games within the app session. Taps,
  drags, legal-move highlighting, and the promotion anchor keep working in
  both orientations.
- Regression guarantee: all existing interactions (two-tap moves, drag & drop,
  promotion, new game, CPU driver) MUST keep working in every game state and
  in both orientations.
- No Rust core / FFI changes: resignation is an app-level terminal status;
  undo is implemented by replaying the kept UCI moves into a fresh session;
  flipping is view-layer only.

## Capabilities

### Modified Capabilities

- `ios`: add a requirement that the iOS game screen presents a modal at game
  end (checkmate, draw, or resignation) with a clear win/loss result and
  restart/dismiss actions, and a requirement for the new game controls
  (resign, undo last move, flip board).
- `android`: add the equivalent requirements for the Android game screen.

No new capabilities and no changes to `shared` (the core already exposes
checkmate/draw status; the FFI contract is untouched).

## Impact

- `ios/app/PlainTextChess/GameViewModel.swift`: terminal-state-driven
  `gameEndDialog` state (message, presented/dismissed flags, `restart()`),
  new `resigned(winner)` status, `resign()`, `undo()`, `boardOrientation` +
  `flipBoard()`.
- `ios/app/PlainTextChess/ContentView.swift`: attach the alert; add the
  Resign/Undo/Flip controls; add DEBUG-only script tokens `undo`, `resign`,
  `flip` and an optional `-PLAINTCHESS_SCRIPT_INTERVAL <seconds>` launch
  argument (default 0.5, release-inert) so simulator verification can drive
  the new controls and wait out CPU replies.
- `ios/app/PlainTextChess/BoardView.swift`: orientation-aware
  square↔pixel mapping.
- `android/.../GameViewModel.kt`: mirror of the VM additions.
- `android/.../MainActivity.kt`: the `AlertDialog`, the control row, and the
  DEBUG tap flows.
- `android/.../BoardView.kt`: orientation-aware square↔pixel mapping.
- No core, FFI, build-script, or CI changes. No new dependencies.
- Verification: iOS simulator via the DEBUG script hook (two-player
  Scholar's Mate → "White wins" modal; CPU/hard Fool's-Mate script → "You
  lost" modal; `undo`/`resign`/`flip` tokens for the new controls); Android
  emulator via real taps and `uiautomator` dumps.
