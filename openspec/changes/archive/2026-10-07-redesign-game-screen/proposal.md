# Proposal

## Why

The game section is a plain vertical stack: a title, a status line, optional clocks, the board, a vertical move list, and a row of buttons. It is functional but sparse: whose game it is (names, ratings) is not shown, the move list is not glanceable, and there is no way to step back through the game to review it. The reference layout (an online chess app's game screen) is denser and more legible: a top bar with the time control and a menu, a compact horizontal move strip, player rows with name/rating/clock around the board, and a bottom action bar with history navigation.

## What Changes

- **New game-screen layout** on iOS and Android: a top bar (time control label + an overflow menu), a compact **horizontal move strip**, an **opponent row** and a **player row** showing name/rating and a clock, the board, and a **bottom action bar**. Applies to online and local/CPU games.
- **Move-history navigation**: step backward/forward through the played moves so the board shows any earlier position, jump to the start/end from the action bar, and jump to a specific ply by tapping it in the move strip. While reviewing history, move input is disabled and the UI shows that the board is not live; new moves keep being recorded and the player can return to the live position.
- **Player identity in the game screen**: online games show each side's username/display name and rating from the server state; local/CPU games show generic labels ("You", "CPU", "Opponent") with no live rating. A clock is shown only when the game has a time control.
- The existing control behaviors (Resign, Undo, Flip board, New game) are preserved; they are relocated into the new bar and overflow menu.

## Capabilities

### New Capabilities
- None.

### Modified Capabilities
- `ios`: "iOS Playable Game Screen" is restyled into the new layout; a new "iOS Move History Navigation" requirement covers browsing.
- `android`: "Android Playable Game Screen" is restyled; a new "Android Move History Navigation" requirement covers browsing.

## Impact

- **iOS**: `ios/app/PlainTextChess/ContentView.swift` (game screen layout, move strip, player rows, action bar) and `GameViewModel.swift` (navigation state and per-ply board snapshots).
- **Android**: `android/.../MainActivity.kt` (same layout) and `GameViewModel.kt` (navigation state and snapshots).
- **Specs**: `ios`, `android`.
- No server, protocol, persistence, clock, or rating-semantics changes; the board rendering and rules stay in the Rust core.
