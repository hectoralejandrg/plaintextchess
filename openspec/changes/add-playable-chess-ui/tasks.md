# Tasks

## 1. iOS — playable game screen (SwiftUI)

- [x] 1.1 Add a FEN parser + `GameViewModel` (`ObservableObject`, D4): holds the UniFFI `GameSession`, exposes board (8×8), selected square, legal targets, last move, status (to-move / check / checkmate / draw / error), move list; intents `select(square)`, `play(uci)`, `newGame()`; malformed FEN routes to the error state; verify with `xcodebuild -project ios/app/PlainTextChess.xcodeproj -scheme PlainTextChess -destination 'generic/platform=iOS Simulator' build` exiting 0
- [x] 1.2 Add `BoardView` + square/piece rendering per D1/D7 (Unicode glyphs, 8×8 grid from available width, selection border, legal-target markers, last-move highlight) and wire it as the app's main screen, replacing the placeholder; verify by installing on the simulator and taking a screenshot showing the standard start position on an 8×8 board
- [x] 1.3 Implement the two-tap selection loop per D3 (select own piece → highlight `get_valid_moves` targets; tap target → `play_move`, board + move list update; illegal target → "not a legal move" feedback with no state change; tapping another own piece re-selects); verify on the simulator with the scripted sequence e2e4, e7e5, g1f3, taking a screenshot after each move
- [x] 1.4 Implement the status row (player to move, check flag after each move via the D6 pull of `is_check`/`is_checkmate`/`is_draw`); verify by reaching check on the simulator (1.e4 d6 2.Bb5+) and confirming the check indicator
- [x] 1.5 Implement the move list and the game-over banner with New game (fresh session, board back to start position); verify by playing Scholar's mate on the simulator (e2e4 e7e5 f1c4 b8c6 d1h5 g8f6 h5f7) and confirming the result banner, the 7-entry move list, and that New game resets the board
- [x] 1.6 Update the intro + iOS sections of `docs/app-projects.md` (apps are now playable games, not placeholder scaffolds) and verify no stale "placeholder" claim remains for iOS (`grep -n placeholder docs/app-projects.md`)

## 2. Android — playable game screen (Jetpack Compose)

- [x] 2.1 Add the Kotlin `GameViewModel` (plain class, `remember`-held, D4 — same state shape and intents as the iOS VM) and verify `./gradlew :app:assembleDebug` (from `android/`, JAVA_HOME 17) exits 0
- [x] 2.2 Add the board composable per D1/D7 (8 rows × 8 squares, Unicode glyphs, selection/legal-target/last-move markers) and make it the app's main screen, replacing the placeholder; verify by installing the debug APK on the arm64-v8a emulator and screenshotting the standard start position
- [x] 2.3 Implement the two-tap selection loop per D3; verify on the emulator with the scripted sequence e2e4, e7e5, g1f3 with a screenshot after each move and confirm logcat has no `UnsatisfiedLinkError`/FATAL
- [x] 2.4 Implement the status row (to-move + check flag, D6 pull); verify by reaching check on the emulator (1.e4 d6 2.Bb5+)
- [x] 2.5 Implement the move list and game-over banner with New game; verify by playing Scholar's mate on the emulator (e2e4 e7e5 f1c4 b8c6 d1h5 g8f6 h5f7) and confirming the result banner, move list, and board reset on New game
- [x] 2.6 Update the Android section of `docs/app-projects.md` to describe the playable game (run/install steps unchanged) and verify the doc no longer describes Android as a placeholder

## 3. Cross-platform integration

- [x] 3.1 Run the same scripted game (1.e4 e5 2.Nf3 Nc6 3.Bb5 a6 4.Ba4 Nf6 5.O-O Be7, including the capture-free castle) on both platforms and verify both move lists are identical (UCI strings, same order)
- [x] 3.2 Clean-rebuild from scratch: delete `ios/Frameworks/`, `android/app/src/main/jniLibs/`, `target/uniffi/`, run `./scripts/build-ios.sh` and `./scripts/build-android.sh`, build both apps, then run all three `tests/validate-*.sh` scripts; verify every command exits 0 and all validators print ALL CHECKS PASSED
- [ ] 3.3 Commit the change work and push to `origin main` (repo `plaintextchess`); verify the `build-validation` Actions run goes green (validate, build-ios, build-android, quality jobs)
