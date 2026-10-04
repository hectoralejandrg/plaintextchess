# Proposal

## Why

The app currently assumes — implicitly, with no spec-level guarantee and no
verification — that a single game session is active at a time, and that
"New game" may replace a running game at any moment. In practice this means a
player can abandon a game in progress and lose its moves with no friction, and
the "one active session" invariant exists only as code convention. The user
wants that guarantee made explicit and enforced: a new game may be started only
when the previous one is finished (checkmate/draw) or someone resigned (or no
move was played yet), and the single-active-session invariant is hardened and
verified on both platforms.

## What Changes

- **New game availability rule (visible UI change, user-requested)**: the
  "New game" action is enabled only when the current game is terminal
  (checkmate, draw, resignation), has no moves played yet, or is in an error
  state (FFI failure, as a recovery action). While a game with at least one
  played move is in progress it is shown disabled (greyed) and does nothing
  when tapped. Resign remains the escape to end a game early.
- **Single active session, made a requirement**: at most one core
  `GameSession` is live per game screen; starting a new game (New game,
  Play again) atomically replaces it, and no move, result, or in-flight CPU
  reply from a replaced session can ever be applied to the active one. This is
  currently code convention only; it becomes a spec requirement with
  verification on iOS and Android.
- **Defense in depth**: in addition to the existing `cpuGeneration` guard, the
  CPU result application will also verify session identity (the session that
  computed the move is still the active one), closing any future code path
  that could replace the session without bumping the generation.
- **Core session isolation, made a requirement + tested**: each
  `new_game_session` instance owns its state with no shared mutable global
  state; read-only operations never mutate; sessions never interfere.
  Verified with new core integration tests.
- **Updated CPU-opponent requirement**: a user-facing discard scenario ("Resigning
  discards a pending CPU move") is added; the existing "New game discards a
  pending CPU move" scenario is no longer reachable from user action (New game
  is disabled mid-game) and is re-scoped as the defensive system guarantee that
  no in-flight CPU move can ever leak into a new game.
- **Android DEBUG verification**: a debug-only CPU-delay intent extra
  (release-inert) so the "while the CPU is thinking" scenarios can be
  reproduced deterministically on the emulator (hard replies in <80 ms today).

## Capabilities

### New Capabilities

(None — this change strengthens existing capabilities; no new spec
directories.)

### Modified Capabilities

- `ios`: adds "iOS Single Active Game Session"; modifies "iOS Game Mode
  Selection" (New game availability rule) and "iOS CPU Opponent" (adds a
  resignation-discard scenario; the pending-move-discard scenario is re-scoped
  as a defensive system guarantee).
- `android`: adds "Android Single Active Game Session"; modifies "Android Game
  Mode Selection" and "Android CPU Opponent" with the same deltas as iOS.
- `shared`: adds "Core Game Session Isolation" (independent per-instance
  state, thread-safe exported operations, non-mutating read-only calls).

## Impact

- **iOS app** (`ios/app/PlainTextChess`): `GameViewModel.swift`
  (`canStartNewGame` gate honored by `startGame`, session-identity check in
  CPU result application, DEBUG script tokens honor the gate),
  `ContentView.swift` (New game button bound to availability, disabled state).
  No FFI surface change.
- **Android app** (`android/app`): `GameViewModel.kt` (same gate and
  identity check), `MainActivity.kt` (New game button disabled state), new
  debug-only `cpu_delay_ms` intent extra. No FFI surface change.
- **Core** (`core/`): integration tests only (session isolation, concurrent
  read-only access). Public FFI surface unchanged; no binding regeneration.
- **Behavior**: existing DEBUG verification recipes that used `newgame`
  mid-game will use `resign` + `playagain` instead; mid-game `newgame` is
  rejected. "Undo back to zero moves" re-enables New game (consistent with the
  "no moves yet" rule).
