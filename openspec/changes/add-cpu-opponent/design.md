# Design

## Context

- The Rust core (`chess-core`, shakmaty 0.8.1) owns every chess rule.
  `GameSession` (UniFFI 0.28) wraps `BoardManager` (a `Chess` position,
  `core/src/domain/board.rs`) and `RatingManager`; moves travel as UCI
  strings, promotions as 5-character UCI.
- Both apps are thin operators over the session: each view model tracks
  `toMove` locally and applies every move through one internal path
  (`playMove(from, to, uci)` → `session.playMove`, flip `toMove`, set
  `lastMove`, append to the move list, refresh board + status). Milestone 2
  added the promotion picker, drag & drop, and the last-move slide overlay on
  both platforms.
- No CPU logic exists anywhere; no engine crate is used. Core dependencies:
  `shakmaty`, `glicko2`, `uniffi` (no `rand`).
- Project principle: all chess logic stays in the Rust core.
- The FFI surface is documented in `openspec/specs/shared/ffi-contracts.md`;
  UniFFI regenerates the Swift/Kotlin bindings at build time.

## Goals / Non-Goals

**Goals:**

- One CPU implementation in the core, called identically from both apps.
- A real, bounded difficulty gradient (easy / medium / hard) with a hard
  sub-second time bound on modern devices.
- CPU moves flow through the existing move path so move list, highlight, and
  slide animation are identical to human moves.
- No new external dependencies; core rules unchanged.

**Non-Goals:**

- Choosing which side the CPU plays (it is always Black for this change).
- Persisting the mode/difficulty across games or app launches.
- A strong engine: no transposition table, no time management, no opening
  book, no repetition-aware play.
- Rating integration for CPU games (rating is a separate future change).
- Sound, undo/history, or any online features.

## Decisions

### D1 — The CPU move is a pure query on the session

`GameSession::get_cpu_move(difficulty: i32) -> Result<String, ChessError>`
returns the UCI of one legal move for the side to move; the session is not
mutated. The app then plays the result through its existing move path.

- Why: one execution path per platform — 5-char promotion UCI, `lastMove`,
  the slide animation, the move list, and error handling all already work
  there. The FFI call stays trivially simple (int in, String/Result out).
- Alternative: `play_cpu_move(difficulty)` playing the move atomically in the
  core. Rejected: the app would lose routing through its standard path and
  would still have to synthesize `lastMove` from the returned move; one fewer
  FFI call is not worth a second move path.

### D2 — Search: depth-limited negamax + alpha-beta, per difficulty

- Depth (plies): easy = 2, medium = 3, hard = 4.
- Move ordering: captures first (MVV-lite). No transposition table; the
  ordering plus the small depth keeps hard well inside the time budget.
- Evaluation: material (P 100 / N 320 / B 330 / R 500 / Q 900 / K 20000) plus
  small piece-square bonuses (pawn advance/centering, knight/bishop center,
  king center penalty in the endgame). Exact tables are an implementation
  detail.
- Easy adds leaf-evaluation noise from a ~15-line xorshift64 PRNG (seeded
  from `SystemTime`, ± up to ~100 cP) so easy varies between games; medium
  and hard stay fully deterministic — same position + difficulty ⇒ same move
  on every platform (this is what makes the cross-platform check possible).
- Why no `rand` crate: a hand-rolled xorshift avoids a new external
  dependency for what is one perturbation call per leaf at easy depth.
- Why no time cap: a depth cap gives predictable, bounded runtime
  (hard ≈ tens to a few hundred ms in release on modern hardware); a time
  cap would add complexity and non-determinism to hard for little gain at
  these depths.

### D3 — Difficulty contract and error

`difficulty` is `i32` and must be in 1..=3; anything else returns a new
`ChessError::InvalidDifficulty` variant (additive — existing variants
untouched). Silent clamping was rejected: it would mask app bugs.

### D4 — Mode selection: a setup sheet from "New game" (both apps)

Tapping **New game** now opens a sheet instead of resetting directly:

- Mode: two players (pre-selected) or CPU.
- When CPU: difficulty — easy / medium / hard.
- Confirm starts the game with the chosen mode; two-player confirm is the
  existing behavior.

The view model gains `gameMode` (twoPlayers | cpu(difficulty)) and
`startGame(mode)`; the sheet's confirm calls it. The choice lives only in the
current game (a new game reopens the sheet and defaults to two players) — no
storage. UI: SwiftUI sheet in `ContentView`; Material 3 sheet in
`MainActivity`'s game screen. Alternative considered: a persistent settings
screen — rejected per user preference (choice per game) and because
persisted settings can drift from the game actually being played.

### D5 — CPU turn driver in each app (same shape, platform threading)

After `startGame` and after every successful human move, the view model
checks: mode is CPU, it is the CPU's side to move, the game is not over, and
no CPU move is in flight. If so: set `cpuThinking = true`, compute
`get_cpu_move` off the main thread, then apply the result on the main thread
through the same internal move path a human move uses.

- iOS: `DispatchQueue.global(qos: .userInitiated).async` → FFI call →
  `DispatchQueue.main.async` to apply.
- Android: `viewModelScope.launch { withContext(Dispatchers.Default) { … } }`
  → apply on the main dispatcher.

Guards:

- **Input lock:** `select` / `canPickup` are no-ops while `cpuThinking`
  (spec: no move, no selection, no error while thinking).
- **Stale move:** a generation counter increments on every `startGame`; a
  completion only applies its move if the generation still matches, so a new
  game started mid-thought discards the in-flight move (spec scenario).
- **Error path:** if the FFI call fails, clear `cpuThinking` and surface the
  error through the existing `errorMessage`/failed-status path; the game
  continues with the human to move.
- **Termination:** after applying, the driver re-runs the check; when
  `refreshStatus` reports checkmate/draw there is nothing to do, so the CPU
  never moves past the end of the game.

### D6 — The CPU plays Black

`cpuColor = "b"`; the human plays white. The driver is written generically
(`toMove == cpuColor`), so allowing side choice later is a UI-only change.
Alternative considered: random side at game start — rejected (the mental
model "you are white" plus simpler verification).

### D7 — Documentation

`openspec/specs/shared/ffi-contracts.md` gains `get_cpu_move` in the Rust
module declaration and the new error variant; the regenerated UniFFI
bindings document themselves per platform.

### D8 — Verification hooks

The existing DEBUG script hook (iOS `-PLAINTCHESS_SCRIPT`) and the
uiautomator tap flow (Android) drive human moves; in a CPU game every human
move triggers a CPU reply, so the same human script at medium/hard must
produce **identical UCI move lists on both platforms** (determinism, D2) —
the cross-platform check for this change.

## Risks / Trade-offs

- [Hard is too slow on low-end devices] → 4-ply alpha-beta with capture
  ordering is node-bounded; a core unit test asserts a hard move from the
  start position completes in under 1 s (release). If real-device timing
  ever says otherwise, drop hard to 3 plies + better ordering — the
  difficulty mapping is a one-line change.
- [CPU strength feels artificial] → depth-4 material + piece-square play
  avoids obvious blunders and mates simple mates; the contract is a
  bounded, predictable gradient, not engine strength. Documented in the
  non-goals.
- [A search on the main thread would freeze the UI] → D5 computes off the
  main thread on both platforms; the driver ensures at most one in-flight
  CPU call (the session is internally mutex-guarded regardless).
- [A stale CPU move lands on the new game] → generation counter guard (D5),
  verified by the "new game discards a pending CPU move" scenario.
- [An FFI failure strands the app in "thinking"] → the error path clears
  `cpuThinking` and surfaces the error (D5).
- [The new sheet changes the New-game flow for existing two-player users] →
  two players stays the pre-selected default; two-player regression is
  explicitly verified on both platforms before closing.

## Migration Plan

No user migration and no breaking API: the FFI addition is purely additive
(new method, new error variant); existing two-player builds are unaffected.
Rollback is a commit revert — two-player mode is self-contained.

## Open Questions

- Exact piece-square tables (implementation detail; does not affect the
  contract).
- Exact "CPU is thinking…" copy per platform (trivial string resources).
