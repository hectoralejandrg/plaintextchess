# Tasks

## 1. Server: clean-architecture restructure (behavior-preserving)

- [x] 1.1 Create the layered module tree (`domain/`, `application/`, `infrastructure/`, `interface/`) and move the existing modules into it (`app.rs` → `infrastructure/server.rs`, `room.rs` actor → `application/room_actor.rs` with room/seat types → `domain/room.rs`, `rating.rs` policy → `domain/rating.rs`, `protocol.rs` → `interface/protocol.rs`, `config.rs`/`ws.rs` → `infrastructure/`, `main.rs` unchanged), with no behavior change; verify `cargo build -p chess-server && cargo test -p chess-server` stays green with all 44 existing tests, including the pinned `wire_form_is_stable`
- [x] 1.2 Define the ports (`application/ports.rs`: `ChessEngine` wrapping the core session, `TimeSource` for monotonic now) and wire concrete implementations in `infrastructure/` (`engine.rs`, default time source), so `domain/` and `application/` import no `tokio`/`axum`/`serde`; verify with `cargo test -p chess-server` green plus a grep confirming no `tokio`/`axum`/`serde` imports in `domain/` and clippy clean
- [x] 1.3 Run a smoke E2E (server up on 8765, `online_buddy` create + join + short forced checkmate) to confirm the restructured server behaves identically on the wire; verify the buddy logs the same terminal result as before the restructure

## 2. Server: Fischer clock + flag fall

- [x] 2.1 Implement `domain/time_control.rs` (the five presets 15+10, 10+0, 5+0, 3+2, 1+0; `TimeControl` with base ms + increment ms; `"m+i"` parse/format; reject non-preset values); verify unit tests pass for all five parse/format round-trips and rejection of e.g. `12+3`
- [x] 2.2 Extend the wire (`interface/protocol.rs`): `create_room` gains `time_control` (missing → default 15+10 for old clients), `State` gains `time_control`/`white_time_ms`/`black_time_ms`, `Status` gains `TimedOut { winner }` serialized as `{"timed_out": {"winner": ...}}`; extend the pinned `wire_form_is_stable` to assert every new field; verify `cargo test -p chess-server` green, including the extended pin and round-trip tests
- [x] 2.3 Implement `domain/clock.rs` (timestamp-based Fischer: settle elapsed, add increment on move completion, clamp at 0, deadline + flag detection) and `domain/material.rs` (insufficient-material-to-mate: no pawns and ≤1 minor, or two same-color bishops); verify unit tests with an injected fake `TimeSource` cover tick math, increment on completion, flag at exactly 0, 0:00 handoff on zero-increment, and the material cases
- [x] 2.4 Wire the clock into the room actor: 200 ms deadline tick + exact deadline check on move apply, Fischer settle on accepted moves, flag fall ending the game as `TimedOut { winner: opponent }` or `Drawn` on insufficient material, ratings updated, `game_over` for late moves; verify integration tests: a 1+0 room where the mover stalls ends `timed_out` with the opponent as winner on both clients' final snapshots and further moves are rejected, and a stalled K vs K position ends `Drawn`
- [x] 2.5 Implement disconnect-aware timing: clocks keep counting while a seat is disconnected, a flag that fell during the disconnect is settled on re-attach or at grace expiry (whichever comes first), snapshots always carry the reduced remaining times; verify integration tests (with `RECONNECT_GRACE_SECS=15`): reconnect after some play shows reduced `*_time_ms` with no time granted, a 1+0 flag during disconnect ends `timed_out` on re-attach, and a flag past the grace expiry ends `forfeited`

## 3. iOS client

- [x] 3.1 Extend `OnlineProtocol.swift` to send `time_control` on `create_room` and to decode `time_control`/`white_time_ms`/`black_time_ms` and the `timed_out` status from snapshots; verify the iOS codec test suite passes with new round-trip/decode cases for the new fields and the new status
- [x] 3.2 Add the time-control preset chips to the create sub-screen (15+10 pre-selected) and send the choice with `create_room`; show the chosen control next to the room code in the waiting state (both for creator and joiner); verify on the simulator: creating a room shows the code together with the selected control, and joining a room created at a different control shows that control
- [x] 3.3 Build the clock row (top/bottom clocks following the board orientation, `m:ss`, active-clock highlight, red at ≤ 10 s) driven by snapshot times with ~10 Hz local interpolation and re-sync on every snapshot; lobby shows base time inactive; verify on the simulator against a live server: both clocks count down, the side-to-move clock is emphasized, the board keeps its exact full-width size (board-measurement check across waiting/playing/reconnecting states)
- [x] 3.4 Show the on-time game-end wordings ("You won on time." / "You lost on time." / "Draw.") for `timed_out` and flag-fall draws, with the updated-ratings footer; verify on a 1+0 E2E run where the other side stalls: the modal text and ratings match the server's final snapshot

## 4. Android client (mirror of 3.1–3.4)

- [x] 4.1 Extend `OnlineProtocol.kt` to send `time_control` and decode the new `State` fields and `timed_out` status; verify the Android codec test suite passes with new round-trip/decode cases
- [x] 4.2 Add the time-control preset selector to the create sub-screen and include the choice in `create_room`; show the chosen control next to the room code in the waiting state; verify on the emulator (uiautomator dump shows the control next to the code for both create and join)
- [x] 4.3 Build the clock row with active-clock highlight, red at ≤ 10 s, snapshot-driven interpolation and re-sync; lobby shows base time inactive; verify on the emulator: both clocks count down, the active clock is highlighted, and the board bounds are unchanged (uiautomator board bounds check across waiting/playing states)
- [x] 4.4 Show the on-time game-end wordings and the updated-ratings footer for `timed_out` and flag-fall draws; verify on a 1+0 E2E run where the other side stalls

## 5. Cross-platform E2E (local server on 127.0.0.1:8765; iOS simulator + Android emulator at ws://10.0.2.2:8765/ws)

- [x] 5.1 Timed game at 3+2: iOS creates, Android joins, play a forced checkmate line; verify both clients' clocks count down with the active highlight following the turn, both modals show the result with the ratings line, the move lists are identical, and the final snapshot times in the server log are plausible (base + increments − elapsed)
- [x] 5.2 Flag fall at 1+0: iOS (White) creates and plays nothing; after ~60 s Android (Black) shows "You won on time." with the ratings line and iOS shows "You lost on time."; verify the server log records the `timed_out` terminal and the room is removed when both disconnect
- [x] 5.3 Reconnect with time at 5+0: drop the Android connection mid-game (disable/enable emulator wifi), re-attach within the grace window; verify the Android clocks resume from the reduced remaining times in the re-attached snapshot, the move list is intact, and no time was granted on re-attach
- [x] 5.4 Regression on both platforms: local two-player and CPU-medium anchor game still plays the known exact sequence (`e2e4 g8f6 d2d4 f6e4 g1f3 b8c6 c2c4 e7e6`), New game stays disabled mid-online-game, and the board-measurement check passes with the clock row visible in all online states

## 6. Wrap-up

- [x] 6.1 Run `openspec validate add-online-time-controls --strict` and the full suites (server `cargo test`, iOS test suite, Android test suite); verify everything passes and `cargo clippy -p chess-server` is clean
- [ ] 6.2 Commit and push the change to `main`; verify the CI run for the pushed commit is green (`build-server` and all `quality` jobs)
