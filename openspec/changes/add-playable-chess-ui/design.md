# Design

## Context

See proposal.md for motivation. State relevant to the approach:

- The Rust core already exposes the full gameplay surface through UniFFI
  (`new_game_session`, `get_board_state` (FEN), `get_valid_moves(square)`,
  `play_move(uci)`, `is_check`, `is_checkmate`, `is_draw`,
  `get_piece_at`, `serialize`/`deserialize`). **No core or binding changes.**
- Both apps are a single placeholder screen today (iOS `ContentView.swift`,
  Android `MainActivity.kt`) that already demonstrates the error-in-UI pattern
  to keep.
- Baselines: iOS 15.0 (SwiftUI), Android minSdk 24 / Kotlin 1.9.24 /
  Compose (BOM 2024.08.00). No new external dependencies allowed by project
  convention (self-contained builds).
- `openspec/specs/{ios,android}/ui-spec.md` sketches a fuller component set
  (pieces as images, drag & drop, timers). This design adopts a minimal
  subset for milestone 1; the ui-spec docs remain the longer-term reference.

## Goals / Non-Goals

**Goals:**
- A thin rendering layer: all game rules stay in the core; the app never
  re-implements chess logic.
- Identical observable behavior on both platforms (same selection model,
  same status feedback, same move-list format).
- iOS 15-compatible SwiftUI; no new Gradle dependencies.

**Non-Goals:**
- Rating UI/wiring, CPU opponent, persistence, drag & drop, piece-image
  assets, move animations, timers (all deferred).

## Decisions

### D1 — Board rendering: bundled "cburnett" vector piece set
Render pieces with the classic **cburnett** chess set (the lichess default),
shipped as vector assets: SVG image sets in the iOS asset catalog and
VectorDrawable resources on Android. **Why:** identical, high-quality pieces
on both platforms, independent of the system font (Unicode glyphs render
differently on Apple vs. Android fonts, and the "white" glyphs are hollow).
**Trade-off:** 12 assets per platform plus a CC BY-SA 3.0 attribution (see
`assets/chess-pieces/README.md`); the artwork is licensed separately from the
repo's MIT code.
**Alternatives:** Unicode glyphs (font-dependent and inconsistent across
platforms — rejected after visual review); a third-party chess UI library
(violates the self-contained/no-new-dependencies convention).

### D2 — Board state is the app-side source of truth
After every successful move the view model stores the board string returned
by `get_board_state()` and parses it into an 8×8 piece map for rendering. The
core's board state carries the 8-rank position only (no FEN side-to-move
field), so the side to move is tracked by the view model: it starts at "w"
for every new session and flips after each successful move (the app is the
session's sole operator in milestone 1). Legality is never computed in the
app — it is always asked to the core via `get_valid_moves`.
**Why:** the core (shakmaty) is authoritative; a duplicated app-side board
model risks diverging from it. Parsing FEN for display is a few lines and is
defensive (malformed FEN → error UI, same as any FFI failure).

### D3 — Two-tap selection model (no drag & drop)
Tap an own piece → its legal destinations (from `get_valid_moves`) are
highlighted; tap a highlighted square → `play_move(uci)`. Tapping a
non-highlighted square does not clear the selection; it shows "not a legal
move" feedback (per spec) so the user can pick another destination. Tapping
another own piece re-selects. Last move is highlighted (source + target
squares) for orientation.
**Why:** matches the spec scenarios exactly; drag gestures would add
platform-specific gesture plumbing with no milestone-1 value.

### D4 — State management without new dependencies
- iOS: `GameViewModel: ObservableObject` with `@Published` state, owned via
  `@StateObject` (iOS 15 baseline rules out the `@Observable` macro, which
  needs iOS 17).
- Android: a plain Kotlin `GameViewModel` class held with `remember` in the
  composable; plain functions drive state (no `lifecycle-viewmodel`, no Hilt).
Both VMs wrap the UniFFI `GameSession` and expose: `board` (8×8),
`selectedSquare`, `legalTargets`, `lastMove`, `status` (to-move / check /
checkmate / draw / error), `moveList`, and intents `select(square)` /
`play(move)` / `newGame()`.

### D5 — Move list in UCI notation
The list shows the core's UCI strings (`e2e4`, `g1f3`). **Why:** the FFI
surface returns/accepts UCI and does not provide SAN; adding SAN conversion
would be a core change (out of scope). UCI is unambiguous and renders cheaply.

### D6 — Game-over detection: pull after each move
After every successful `play_move`, the VM queries `is_checkmate()`,
`is_draw()`, `is_check()` and sets the status accordingly. On checkmate/draw
the screen shows the result with a New game action that creates a fresh
session (`new_game_session(1500.0)` — starting rating stays the fixed
default; rating UI is out of scope). **Why:** the FFI has no event/callback
mechanism, so polling after each mutation is the only option; the cost is
three cheap calls.

### D7 — Layout
Both platforms: status row (player to move + check flag) above the board,
move list below, New game action. The board fills the available width
edge-to-edge (no side padding): a plain 8×8 grid of squares (SwiftUI nested
stacks / Compose 8 rows of 8 squares) with **no extra row or column**.
Coordinates are drawn *inside* the edge squares — files `a–h` along the
bottom edge anchored to the bottom-left corner, ranks `8–1` along the right
edge anchored to the top-right corner — colored for contrast against the
square. Squares use the lichess-style brown pair (light `#F0D9B5` / dark
`#B58863`) with high-contrast selection markers.

## Risks / Trade-offs

- [Unicode glyphs look less polished than real piece art] → acceptable for
  milestone 1; asset upgrade is a follow-up that D1 makes trivial (swap the
  text for an `Image` per square).
- [App-side FEN parsing can be wrong] → parser validates shape (8 ranks × 8
  files, legal piece chars); any parse failure routes to the existing
  error-in-UI path instead of crashing.
- [iOS 15 baseline limits modern SwiftUI APIs] → VM uses
  `ObservableObject`/`@Published`; only iOS 15-compatible views are used
  (verified against the baseline in the build config).
- [No automated UI test harness in the repo] → spec scenarios are validated
  by documented simulator/emulator checklists in tasks.md; adding
  XCUITest/Compose-test infrastructure is a separate follow-up.
- [Two independent VMs can drift apart] → D4 keeps the state shape and
  intent names identical on both platforms; the task checklist verifies the
  same scripted game on both.

## Migration Plan

In-place replacement of the placeholder screens (files: iOS
`ContentView.swift` + new VM/views; Android `MainActivity.kt` + new
composables/VM). No data migration, no artifact or CI changes. Rollback is
a plain revert; the placeholder remains in git history.

## Open Questions

- None that block implementation. Board behavior in iPad/landscape is
  handled by the adaptive layout in D7 and can be tuned during manual QA.
