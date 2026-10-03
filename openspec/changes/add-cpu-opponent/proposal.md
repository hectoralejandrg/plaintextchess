# Proposal

## Why

The game currently only supports local two-player play. Players also want to
play against a computer opponent whose strength they can adjust, so a single
person can practice and enjoy the game. All chess logic already lives in the
Rust core (shakmaty), so the CPU "brain" belongs there too: one shared
implementation, no per-app heuristics, no new external dependencies.

## What Changes

- **New core function** `GameSession.get_cpu_move(difficulty)` returns a legal
  UCI move for the side to move at difficulty 1 (easy), 2 (medium), or
  3 (hard). It is a pure query — it never mutates the session — and is
  computed in the core with a depth-limited minimax/alpha-beta search over a
  material + piece-square evaluation (deeper search at higher difficulty;
  light randomness at easy). CPU promotion moves come back as 5-character UCI
  strings, like every other move the core reports.
- **New error variant** `ChessError::InvalidDifficulty` for out-of-range
  difficulty values.
- **Game mode selection on both apps**: the **New game** action now opens a
  setup sheet to choose the mode (two players — the default, with unchanged
  behavior — or CPU) and, for CPU, the difficulty (easy / medium / hard).
  The choice is fixed for the lifetime of that game; no persistence.
- **CPU opponent behavior on both apps**: the CPU plays Black. When it is the
  CPU's turn the app shows a "CPU is thinking…" status, disables board input,
  computes the move off the main thread, and plays it through the same move
  path as human moves (move list entry, last-move highlight, slide
  animation). A new game started while the CPU is thinking discards the
  pending move.
- Two-player mode is untouched apart from the new setup sheet; core game
  rules are unchanged.

## Capabilities

### New Capabilities

(none — the CPU behavior extends the existing capabilities)

### Modified Capabilities

- `shared`: the cross-platform FFI contract gains the core CPU-move function
  (legal move per difficulty level, error on invalid difficulty, no session
  mutation, bounded compute time) and the new error variant.
- `ios`: the game screen gains game-mode selection on new game and
  CPU-opponent play with a thinking state.
- `android`: mirrors `ios` (same selection, same CPU behavior).

## Impact

- **Core**: `core/src/domain/` (new `search.rs` module, small delegation in
  `board.rs`), `core/src/lib.rs` (FFI method + error variant),
  `openspec/specs/shared/ffi-contracts.md` (document the new function).
  No new Rust dependencies (easy-level randomness uses a small built-in
  xorshift PRNG, not a `rand` crate).
- **iOS**: `GameViewModel.swift` (mode state, thinking flag, off-main-thread
  CPU move, stale-move guard), `ContentView.swift` (setup sheet, thinking
  status text).
- **Android**: `GameViewModel.kt` (same, via coroutines on
  `Dispatchers.Default`), `MainActivity.kt` (setup sheet, thinking status
  text).
- **Build/CI**: the four Rust targets are rebuilt and the UniFFI bindings
  are regenerated on both platforms; no change to minimum OS versions,
  build scripts, or app dependencies.
