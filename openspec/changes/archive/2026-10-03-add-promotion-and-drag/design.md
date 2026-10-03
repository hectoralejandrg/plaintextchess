# Design

## Context

See proposal.md for motivation. State relevant to the approach:

- Milestone 1 shipped a playable two-player game on both platforms:
  `GameViewModel` (iOS `ObservableObject`, Android `remember`-held plain class)
  plus a hand-rolled `BoardView` (SwiftUI nested stacks / Compose rows of
  squares), two-tap selection, in-board coordinates, and the bundled cburnett
  piece set. The archived `add-playable-chess-ui` design (D1–D7) is the
  foundation this change builds on; its D3 ("two-tap, no drag & drop") is
  deliberately superseded here.
- The core's `get_valid_moves(square)` returns **full UCI strings**. For a
  pawn on the penultimate rank it returns four strings that differ only in a
  5th promotion character (`a2a1q`, `a2a1r`, `a2a1b`, `a2a1n`). Today the app
  strips every move to its destination square and `playMove` sends a bare
  `from + to` (`a2a1`), which the core rejects → promotion is currently
  broken (an error is rendered in the UI, no move is played).
- Baselines unchanged: iOS 15.0 (SwiftUI, `ObservableObject` — no
  `@Observable`), Android minSdk 24 / Kotlin / Compose BOM 2024.08.00.
  No new external dependencies (project convention: self-contained builds).
- Verification today is checklist-based: iOS via a DEBUG launch-arg script
  hook (`-PLAINTCHESS_SCRIPT "…"`, tokens are UCI moves) that drives the same
  `select()` intent path as taps, plus screenshots; Android via real
  `adb shell input tap` at coordinates computed from `uiautomator dump`.

## Goals / Non-Goals

**Goals:**
- Fix promotion end-to-end without touching the core: legality and
  promotion availability stay 100% core-driven.
- Drag & drop that coexists with two-tap and has identical observable
  semantics on both platforms (same selection model, same feedback).
- A short, cheap slide animation for played moves, user-disable-able.

**Non-Goals:**
- Captured-piece, castling, or en-passant special animations; board
  flip/rotation; timers; CPU, rating, persistence; adopting a chess-UI
  library (ChessboardKit-style rewrite stays a separate future change —
  drag & drop here is hand-rolled on purpose, mirroring the milestone-1
  decision).

## Decisions

### D1 — Promotion detection from UCI moves (no new core surface)
When a piece is selected — by tap **or** drag start — the VM already fetches
`get_valid_moves(from)`. Moves with a 5th UCI character are promotions; the
VM groups them by destination square. When the user chooses a destination
that has such grouped moves, the VM does **not** play: it sets a
`pendingPromotion(from, to, options)` state (`options` = the full UCI
strings, ordered q/r/b/n) and stops. Two new intents:
`confirmPromotion(piece)` plays `from + to + piece` through the existing
move path (board refresh, status pull, move-list append — the list then
records the 5-character UCI, consistent with the UCI-notation decision), and
`cancelPromotion()` clears the pending promotion and the selection with no
state change.
**Why:** zero core/FFI/build changes; every legality question (which
promotions exist at all, whether the target square is capturable, etc.)
remains answered by the core.
**Alternatives:** a core `get_promotion_choices()` call (out of scope — new
FFI surface for what the UCI list already answers); auto-promote to queen
(always) (loses under-promotion, and the spec requires the choice).

### D2 — Inline promotion picker, not a modal
Both platforms render the four pieces as a small semi-opaque card **over the
destination square**, offset into the board interior: white pawns promote on
the top row, so the card hangs below the square; black pawns promote on the
bottom row, so it rises above it. Pieces are shown in the existing cburnett
assets with a generous tap target (≥ 44 pt / 48 dp equivalent); tapping any
square outside the card cancels.
**Why:** keeps the user's attention on the promotion square
(lichess-style), avoids platform dialog plumbing, and needs no modality in
SwiftUI/Compose.
**Trade-off:** the card temporarily covers part of the board; acceptable
because any tap dismisses it and the destination square stays visible.
**Alternatives:** a centered modal dialog (adds a platform-conditional UI
layer and moves the eye away from the square — rejected).

### D3 — Drag & drop: one container gesture, same semantics as tap
The drag is pure view-layer state: `dragFrom` (the square under the initial
touch) and `dragLocation` (pointer position). No new VM state is introduced
for the gesture itself: drag start calls the **same** `select(from)` intent a
tap calls, so `selectedSquare`/`legalTargets` (and therefore D1's promotion
detection) behave identically. The view only decides geometry:
- **iOS**: a `DragGesture(minimumDistance: ~8 pt)` on the board container
  (not per-square) computing the touched square from the start location. The
  existing per-square `onTapGesture` is retained; SwiftUI disambiguates — a
  tap that never exceeds the minimum distance stays a tap. While dragging:
  the dragged piece renders as an overlay at the pointer position (slightly
  enlarged) and its origin square renders empty; legal targets stay
  highlighted. On end: the destination square is computed from the release
  location; if it is a legal target, the view calls the same
  `select(to)` intent as a tap (which triggers D1 when it is a promotion);
  otherwise it clears the lift and shows "Not a legal move".
- **Android**: `Modifier.pointerInput` on the board box with
  `awaitEachGesture`: `awaitFirstDown()` → square from position;
  subsequent drag events update `dragLocation`; release → same drop logic.
  The existing `clickable` tap path is retained.
**Why one gesture on the container instead of 64 per-square gestures:**
offset math and hit-testing have a single source of truth (board-local
coordinates → one `div`), and the lift overlay is trivial to position.
**Alternatives:** per-square `draggable` modifiers (64 gesture states,
fragmented coordinate math); a chess-UI library (dependency + baseline
constraints — rejected, see milestone-1 D1).

### D4 — Move animation: render-only overlay slide, ~200 ms
The VM publishes the new board immediately after a successful move (single
source of truth is unchanged). The slide is a **render-only effect** keyed on
`lastMove`: the view draws the moved piece as an overlay that starts at the
origin square's center and animates its offset to the destination center
(iOS `withAnimation(.easeInOut(duration: 0.2))`; Compose
`withAnimation(tween(200, ...))` translating an `Offset`). A capture is not
an extra animation: the captured piece simply disappears as part of the board
update. Drag-drop plays the move through the same path, so it animates the
same way (the piece is already at the destination, so the overlay settles in
place — no redundant slide back to origin).
- iOS Reduce Motion: `UIAccessibility.isReduceMotionEnabled` → duration 0.
- Android "No animation": `Settings.Global.ANIMATOR_DURATION_SCALE == 0f` →
  skip.
**Why:** no animation state in the VM (state and visuals can't desynchronize);
a fast scripted sequence (0.5 s token interval in the hook) comfortably
exceeds the animation duration.
**Trade-off:** a ~200 ms overlay render pass per move; negligible on the
target hardware.

### D5 — Verification hooks (DEBUG only)
- **iOS**: extend the existing `-PLAINTCHESS_SCRIPT` hook: a 5-character
  token (`a2a1q`) drives the full picker intent path —
  `select(from)` → `select(to)` → `confirmPromotion(piece)` — so promotions
  run through the exact intents the picker buttons call; a 4-character
  token landing on a promotion destination stops with the picker open
  (screenshot-able). Add a `-PLAINTCHESS_DRAG "from,to"` launch arg that
  forces an in-flight lift on `from` and then runs the same drop entry point
  the gesture calls, so the lift rendering and the drop logic are
  screenshot-verifiable without a pointer. Optionally an anim-scale arg to
  stretch the slide for mid-flight screenshots.
- **Android**: no app hook needed — `adb shell input swipe` injects real
  gestures; picker taps use `uiautomator dump` clickable bounds as in
  milestone 1.
**Why:** keeps the established verification approach (documented checklists +
screenshots + real input injection on Android) with zero new test
infrastructure.

## Risks / Trade-offs

- [Drag/tap ambiguity] → minimum-distance threshold (~8–12 pt/dp); anything
  below it stays the tap path. Both platforms' specs carry an explicit
  "tap still works" scenario that the task checklists verify.
- [Promotion card covers part of the board] → it is the only open UI while
  pending (any outside tap dismisses) and the destination square remains
  visible; the offset-into-interior rule keeps it on-board.
- [Pointer math drifts across densities] → all drag math happens in one
  board-local coordinate space (pixels from the gesture layer, converted
  once); verified on the emulator with a real `input swipe`.
- [Slide overlay desynchronizes with fast moves] → the overlay is keyed on
  `lastMove` and removed at animation end; the VM is already at the new
  position, so a mistimed overlay can never corrupt state.
- [The two hand-rolled VMs drift apart] → identical state shape and intent
  names (`pendingPromotion`, `confirmPromotion`, `cancelPromotion`) on both
  platforms; the cross-platform task runs the same promotion game on both.
- [iOS drag can only be verified synthetically in the CI-less local flow] →
  the DEBUG drag hook exercises the same drop entry point the gesture calls;
  a final manual pointer check on the booted simulator closes the gap.

## Migration Plan

In-place edits to the existing VM and board files on both platforms; no
artifact, CI, dependency, or baseline changes. Rollback is a plain git
revert; milestone 1's two-tap behavior remains the fallback path throughout
(the drag is additive).

## Open Questions

- None that block implementation. Board flip, castling drag hints, and
  captured-piece animations remain future polish and are deliberately
  excluded from this change's specs.
