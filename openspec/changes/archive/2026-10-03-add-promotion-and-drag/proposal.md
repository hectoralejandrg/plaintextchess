# Proposal

## Why

The milestone-1 game is playable but the interface is incomplete: pawn promotion
is currently **broken** (the app strips `get_valid_moves` results to destination
squares and sends a bare UCI like `e7e8`, which the core rejects with an error),
and the only way to move a piece is two-tap — dragging a piece is the expected
primary interaction for a chess board. The Rust core already accepts full-UCI
promotions via `play_move`, so this change is app-side only: no core, binding,
or build-script modifications.

## What Changes

- **Pawn promotion (both platforms)**: when a pawn's chosen destination is on
  the last rank, the app shows an **inline piece selector** (queen / rook /
  bishop / knight) anchored over the destination square instead of playing.
  Picking a piece plays the full UCI move with the promotion character
  (e.g. `e7e8q`); dismissing cancels with no state change. The move list
  records the 5-character UCI (consistent with the existing UCI-notation
  decision).
- **Piece drag & drop (both platforms)**: press-drag a piece of the side to
  move → the piece lifts and follows the pointer while its legal destinations
  (from `get_valid_moves`) are highlighted; releasing over a legal destination
  plays the move through the same core path as tap selection; releasing over an
  illegal square is a no-op with "not a legal move" feedback. The two-tap
  selection is retained alongside drag.
- **Move animation (both platforms)**: a short (~0.2 s) slide of the moved
  piece from origin to destination, skipped when the user has reduce-motion
  (iOS) / system animations disabled (Android).
- **Out of scope (explicit)**: captured-piece / castling / en-passant special
  animations, board flip or rotation, timers, CPU opponent, rating UI,
  persistence — all unchanged from milestone 1. No new external dependencies;
  the iOS 15.0 baseline is unchanged.

## Capabilities

### New Capabilities

- (none)

### Modified Capabilities

- `ios`: adds the requirements `iOS Pawn Promotion`, `iOS Piece Drag and
  Drop`, and `iOS Move Animation` to the game-screen capability; the existing
  `iOS Playable Game Screen`, XCFramework, and build-configuration requirements
  are untouched
- `android`: adds the requirements `Android Pawn Promotion`, `Android Piece
  Drag and Drop`, and `Android Move Animation`; the existing
  `Android Playable Game Screen`, native-library, and Gradle-wrapper
  requirements are untouched

## Impact

- Code: `ios/app/PlainTextChess/` — `GameViewModel.swift` (promotion detection
  + `confirmPromotion`/`cancelPromotion` intents, extended DEBUG script hook),
  `BoardView.swift` (drag gesture, inline picker overlay, slide animation),
  `ContentView.swift` (wiring only, if any)
- Code: `android/app/src/main/java/com/hectoralejandrg/plaintextchess/` —
  `GameViewModel.kt` (mirror of the iOS VM changes), `BoardView.kt` (pointer
  drag gesture, picker overlay, slide animation), `MainActivity.kt` (wiring
  only, if any)
- No changes to: `core/` (Rust crate + FFI surface), UniFFI-generated
  bindings, `scripts/`, `config/`, `tests/` validators, `.github/workflows/`
  (CI keeps validating the same artifacts)
- Docs: `docs/app-projects.md` (game now has promotion, drag & drop, and move
  animation) and `openspec/specs/{ios,android}/ui-spec.md` (drag & drop and
  promotion were listed there as future work)
