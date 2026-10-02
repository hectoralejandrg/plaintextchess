# Proposal

## Why

Both apps currently show a placeholder screen that only proves the FFI chain works (initial
FEN + starting rating). The natural next milestone is the first playable experience: a
two-players-local chess game on both platforms. The Rust core already exposes everything a
playable UI needs (`get_valid_moves`, `play_move`, `is_check`, `is_checkmate`, `is_draw`,
`get_board_state`), so this change is app-side only — no core, binding, or build-script
modifications.

## What Changes

- **iOS app** — replace the placeholder `ContentView` with a playable game screen:
  - 8×8 board rendered from the FEN returned by the core
  - tap-to-select a piece; legal destinations (from `get_valid_moves`) highlighted
  - tap a highlighted square to play the move (`play_move`); illegal targets are rejected
    with feedback and no state change
  - live status: whose move, check, checkmate, draw
  - move list of the game (UCI strings as returned by the core)
  - game-over banner with a "New game" action (fresh `GameSession`)
- **Android app** — the same playable screen in Jetpack Compose
- **Out of scope (explicit)**: Glicko-2 rating UI/wiring, CPU opponent, game persistence
  (serialize/deserialize exist in the FFI but are not surfaced yet), piece-drag gestures,
  animations beyond minimal state transitions
- Placeholder screen is removed (superseded by the game screen); the FFI error handling
  behavior (render errors in the UI instead of crashing) is retained

## Capabilities

### New Capabilities

- (none)

### Modified Capabilities

- `ios`: the placeholder-era `iOS App Project` requirement is removed and re-expressed as
  `iOS Playable Game Screen` (board rendering, move selection/validation, game status, move
  list, new game); the XCFramework/build-configuration requirements are untouched
- `android`: the same for the Compose app — `Android App Project` removed,
  `Android Playable Game Screen` added; native-library and Gradle-wrapper requirements
  untouched

## Impact

- Code: `ios/app/PlainTextChess/` (new SwiftUI views + view model),
  `android/app/src/main/java/com/hectoralejandrg/plaintextchess/` (new Compose screen +
  view model)
- No changes to: `core/` (Rust crate + FFI surface), `scripts/`, `config/`,
  `tests/` validators, `.github/workflows/` (CI keeps validating the same artifacts)
- Docs: `docs/app-projects.md` (app description) and `openspec/specs/{ios,android}/ui-spec.md`
  stay aligned with the implemented components
