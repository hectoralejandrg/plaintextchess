# Design

## Context

See proposal.md — Why. Current state, all of it already exists and works;
this change makes the "one active game" invariant explicit, adds a user-facing
availability rule for "New game", and hardens + verifies the session
isolation.

- **Core** (`core/src/lib.rs`): `new_game_session(initial_rating)` returns an
  `Arc<GameSession>`; each session owns `Mutex<BoardManager>` and
  `Mutex<RatingManager>`. There is no shared mutable global state.
  `get_cpu_move` is a pure query (spec: `shared` "Core CPU Move").
- **iOS VM** (`ios/app/PlainTextChess/GameViewModel.swift`):
  `startGame(_:)` replaces `session`, bumps `cpuGeneration`, clears
  selection/promotion (`clearSelection()` also nils `pendingPromotion`);
  `undo()` rebuilds a fresh session and commits it only on successful replay;
  `resign()` bumps the generation; the async CPU path
  (`triggerCpuMoveIfNeeded` → `applyCpuMove`) applies a result only when
  `cpuGeneration` still matches.
- **Android VM** (`android/.../GameViewModel.kt`): line-for-line mirror of the
  above (`Dispatchers.Main` scope, `withContext(Dispatchers.Default)` for the
  compute).
- **UI**: "New game" is an always-enabled button on both platforms; on iOS
  it opens a setup sheet, on Android a bottom sheet. DEBUG script tokens
  (`newgame`, `playagain`, `resign`, `undo`, …) call the same VM entry points
  as the buttons (release-inert hooks) and are the accepted verification
  mechanism on the simulator; on Android the equivalent is real taps plus a
  deterministic CPU-delay (which does not exist yet — hard replies in <80 ms
  on the emulator, so "while the CPU is thinking" cannot be caught by taps).

## Goals / Non-Goals

**Goals:**
- "New game" enabled iff: game terminal, or no move played yet, or game in an
  error state; disabled (greyed) otherwise, with `startGame` itself guarding
  the rule.
- One live `GameSession` per screen; replacing it can never let a stale CPU
  result (or any stale data) touch the new session.
- The invariant is *verified*: deterministic app-level recipes on both
  platforms + core integration tests.
- No FFI surface change; no binding regeneration.

**Non-Goals:**
- No multiplayer/server, no persisted resume, no analysis mode, no multiple
  simultaneous boards.
- No changes to move legality, CPU strength, or the existing undo/resign/flip
  behavior.

## Decisions

- **D1 — Single source of truth for availability: `canStartNewGame` in the
  VM.** Computed as `moveList.isEmpty || status terminal || status failed`.
  The UI button is bound to it (`enabled` on both platforms); `startGame`
  itself also guards: if not `canStartNewGame` it returns without touching
  any state. Guarding in the VM (not only the UI) keeps DEBUG script tokens,
  future entry points, and any future UI path honest. Alternative: gate only
  in the view layer — rejected: script tokens would still be able to replace
  a running game, silently breaking the invariant the change is about.
  "Undo back to zero moves" re-enables New game for free (`moveList.isEmpty`),
  which matches the "no move has been played yet" wording of the spec.
  `.failed` keeps New game available as a recovery action.
- **D2 — "Play again" always passes the gate.** `restart()` runs from the
  terminal modal, where the game is terminal by definition, so it needs no
  special-casing; it is a convenience over `startGame(gameMode)`. Keeping the
  gate in one place (D1) means terminal ⇒ `canStartNewGame` is true and
  `restart()` works unchanged.
- **D3 — Defense in depth: session identity check on CPU commit.** Today the
  in-flight result is guarded only by `cpuGeneration` (bumped by
  `startGame`/`undo`/`resign`). We add a second guard: when the CPU compute is
  launched, capture the session reference; when committing the result, apply
  it only if that reference is still `===` the current `session`. This closes
  any future code path that could replace the session without bumping the
  generation (e.g. a new undo-like feature). Cost: one reference comparison.
  Alternatives: a lock around session replacement (rejected: the VM is
  single-threaded on the main actor/dispatcher — the race is only across the
  async boundary, which the generation + identity checks cover), or dropping
  the identity check (rejected: the spec says "no in-flight reply from a
  replaced session is ever applied", and today that holds only by convention
  of every replacement bumping the generation).
- **D4 — Core: isolation stays per-instance; no FFI change.** The core already
  owns state per `GameSession` behind `Mutex`; nothing in this change mutates
  the public surface, so no binding regeneration. The new guarantee is made
  *provable* with integration tests in `core/tests` (isolation, non-mutating
  `get_cpu_move`, concurrent read-only calls from multiple threads).
- **D5 — iOS verification via existing DEBUG tokens.** The script tokens call
  the same entry points as the UI, so `newgame` mid-game now gets rejected by
  the D1 guard (verifies the gate end-to-end), and `resign` + `playagain`
  replace the old mid-game `newgame` recipes. `-PLAINTCHESS_CPU_DELAY`
  (existing) widens the thinking window for the "resign/undo while thinking"
  recipes. No new iOS test hook.
- **D6 — Android verification: debug-only `cpu_delay_ms` intent extra.**
  Mirrors the iOS `-PLAINTCHESS_CPU_DELAY` launch argument: read only in
  `DEBUG` builds (release-inert, same pattern as the iOS arg), delays the
  application of the CPU result so taps on Resign/Undo can deterministically
  land "while the CPU is thinking". Alternative: keep racing the emulator
  (rejected: proven flaky — hard's first reply lands in <80 ms, every
  attempt so far has missed). No other Android hook is needed: real taps
  cover the gate (New game greyed after the first move), the sheet flow is
  unchanged, and the uiautomator dump + `screencap` provide the assertions.

## Risks / Trade-offs

- [Behavior change visible to users: mid-game "New game" no longer works] →
  This is the point of the change (user-requested). Resign remains the escape;
  the button stays visible (greyed) so the affordance is not lost.
- [Old DEBUG verification recipes use `newgame` mid-game] → Updated as part of
  this change (tasks verify the new recipes: `resign` + `playagain`, `newgame`
  only at zero moves); the gate rejection of mid-game `newgame` is itself a
  verified scenario.
- [D1 gate could become stale if a new terminal-ish state is added later]
  → `canStartNewGame` is a single computed property; new states are added in
  one place. The spec pins the three enabling conditions explicitly.
- [D3 identity check adds a code path that could mask a real regression] →
  It only *rejects* applying a stale result (the safe direction); a legitimate
  in-flight move is never rejected because only `startGame`/`undo`/`resign`
  replace the session, and all of them also bump the generation (belt and
  braces).
- [Android `cpu_delay_ms` is a new debug surface] → DEBUG-only, release-inert,
  documented in tasks; same trust level as the existing iOS launch args.

## Migration Plan

1. Implement + verify iOS (gate, identity check, recipes).
2. Implement + verify Android (gate, identity check, `cpu_delay_ms`, recipes).
3. Add core isolation tests; `cargo test`.
4. Cross-platform consistency pass + `openspec validate --strict`.
5. Commit; push only after explicit user confirmation; CI must be green.
Rollback: revert the single feature commit — no data, FFI, or storage schema
is involved.

## Open Questions

(None — the remaining decisions, including "Undo back to zero moves
re-enables New game", were settled with the user during planning.)
