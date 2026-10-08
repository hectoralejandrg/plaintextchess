# Tasks

## 1. Server: assign a random color at room creation

- [x] 1.1 Add an injectable color-choice seam to the server (`RoomServices`), defaulting to random in production and exposing a deterministic hook for tests; verify `cargo test --manifest-path server/Cargo.toml` still compiles and passes
- [x] 1.2 Randomize the creator's seat in `RoomActor::handle_connect` and generalize the seat logic: `started()` becomes "both seats occupied", the joiner takes the empty seat, and the hijack guard keys off the seat a device names rather than `seats[BLACK]`; verify the room-creation, join, and re-attach unit tests
- [x] 1.3 Update the `room_actor` unit tests that assume the creator is White to pin the deterministic seam, and add a test that enables randomness and asserts both colors occur across repeated creations; verify `cargo test --manifest-path server/Cargo.toml`
- [x] 1.4 Update the socket integration tests (`server/tests/auth.rs`, `clock.rs`, `persistence.rs`, `integration.rs`) so the helper that moves as White is selected by each connection's reported `your_color` instead of assuming the creator is White; verify `cargo test --manifest-path server/Cargo.toml`

## 2. Clients

- [x] 2.1 iOS: surface the assigned color ("You are White" / "You are Black") on the online waiting screen; verify `xcodebuild -project ios/app/PlainTextChess.xcodeproj -scheme PlainTextChess -sdk iphonesimulator build` succeeds and the screen shows the reported color
- [x] 2.2 iOS DEBUG: make the `onlinecreate` / `onlinejoin:<code>` script tokens read the assigned color instead of assuming the creator is White; verify with a scripted simulator run against the local server
- [x] 2.3 Android: surface the assigned color on the online waiting screen; verify `./gradlew :app:assembleDebug` (from `android/`) succeeds and the screen shows the reported color
- [x] 2.4 Confirm the board is oriented with the player's own color for both a Black creator and a Black joiner; verify by force-selecting each color through the deterministic seam and checking the rendered orientation

## 3. Verification

- [x] 3.1 End-to-end with randomness on: create a room from Android, join from iOS (and the reverse), play a move from each side, and confirm the server assigns the colors and each board orients to the player's own color; record the observed colors and moves
- [x] 3.2 Sync the delta specs into the main specs and validate; verify `openspec validate --specs` reports no errors
