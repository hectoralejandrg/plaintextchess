# Tasks

## 1. Move-history state and per-ply snapshots

- [x] 1.1 iOS: add `viewPly` (nil = live) and an indexed per-ply board snapshot cache to `GameViewModel`, updated on local moves, CPU replies, undo, and each replayed online move/update; verify with unit tests that the cache matches the move list at every ply
- [x] 1.2 Android: mirror 1.1 in `GameViewModel`; verify with unit tests
- [x] 1.3 Both: while a past ply is shown, move/selection input MUST no-op and the core session MUST be unchanged; verify with tests that selecting a piece does nothing in history mode

## 2. iOS game-screen layout

- [x] 2.1 Rebuild `GameView`'s layout: a top bar (time control + overflow menu with New game, Flip board, and Undo for local/CPU), a horizontal move strip, an opponent row above the board and a player row below (name/rating/clock), and a bottom action bar (Resign + previous / jump-to-live / next); verify `xcodebuild … build` succeeds
- [x] 2.2 Wire the move strip (tap a ply) and the action bar to `viewPly`, add the not-live indicator and the return-to-live control; verify by browsing a local game and returning to live

## 3. Android game-screen layout

- [x] 3.1 Rebuild `GameScreen`'s layout with the same structure (top bar with overflow menu, horizontal move strip, player rows, bottom action bar); verify `./gradlew :app:assembleDebug` succeeds
- [x] 3.2 Wire the move strip and action bar to `viewPly`, add the not-live indicator and return-to-live; verify by browsing a local game and returning to live

## 4. Verification

- [x] 4.1 Local/CPU: play a game, browse back and forward, jump to a ply, confirm input is disabled while browsing and that undo returns the strip and cache to a consistent length
- [ ] 4.2 Online iOS↔Android: confirm each player row shows the correct name/rating and the clock, then browse history on one side while the other moves and return to live without desync
- [x] 4.3 Sync the `ios`/`android` deltas into the main specs and validate; verify `openspec validate --specs` reports no errors
