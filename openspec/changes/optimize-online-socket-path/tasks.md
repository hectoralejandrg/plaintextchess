# Tasks

## 1. Protocol: the incremental update message

- [x] 1.1 Add `ServerMessage::Update { v, uci: Option<String>, side_to_move, status, white_rating, black_rating, opponent_online, white_time_ms, black_time_ms }` to `server/src/interface/protocol.rs`, extend the pinned `wire_form_is_stable` test and add a round-trip test; verify `cargo test --manifest-path server/Cargo.toml --lib interface::protocol`
- [x] 1.2 Add the `update` server message to the iOS and Android codecs (`OnlineProtocol.swift` / `OnlineProtocol.kt`) with decode tests; verify the protocol test suites pass

## 2. Server: emit updates, keep snapshots for connect/re-attach

- [x] 2.1 Emit `Update` (with the applied move, or none for terminal results) from `handle_move`/`handle_resign`/`settle_flag`/forfeit, and keep a full `State` for `room_ready` and re-attach; verify the `room_actor` tests assert the update contents and that a re-attach still sends a full snapshot
- [x] 2.2 Confirm a connecting or re-attaching player always receives the full snapshot (board, full move list, clocks) so a missed update cannot desync; verify with the reconnect integration test

## 3. Clients: apply incremental updates

- [x] 3.1 iOS: handle `Update` in `GameViewModel.applyOnlineSnapshot`-adjacent path — replay `uci` into the mirror, append to the move list, update side to move, clocks, status, and ratings, without using `board_fen` per move; verify `xcodebuild … build` and a scripted online run shows the move and clocks
- [x] 3.2 Android: mirror 3.1 in `GameViewModel`; verify `./gradlew :app:assembleDebug` (from `android/`) and a scripted/manual online run

## 4. Server: bounded outbound, split read/write, keepalive

- [x] 4.1 Bound each connection's outbound buffering with coalescing of state-class frames (newest wins) while preserving one-shot control frames in order; verify a test where a client stops reading keeps the pending buffer bounded and later converges to the newest state
- [x] 4.2 Split the connection into a reader and a writer so a slow write cannot stall inbound framing; verify a test where a stalled write still lets the connection process an inbound frame
- [x] 4.3 Send periodic keepalive pings and treat a peer that stops responding as a disconnect via the existing `Detach` path; verify a test that an unresponsive peer is detached before the grace window elapses

## 5. Server: cheaper snapshot and reactive timer

- [x] 5.1 Compute the shared snapshot fields once per broadcast and clone per seat (only `your_color`/`opponent_online` differ); verify both seats receive an equivalent snapshot and `cargo test --manifest-path server/Cargo.toml`
- [x] 5.2 Replace the fixed 200 ms deadline interval with a timer armed only while a clock is running and cancelled on a terminal result or for rooms without a time control; verify the `clock` tests stay green

## 6. Clients: decode off the main thread

- [x] 6.1 iOS: decode incoming frames on a background queue and publish on the main thread; verify the build and that the UI stays responsive while updates stream
- [x] 6.2 Android: decode on OkHttp's executor and hop to `Dispatchers.Main` only to publish; verify `./gradlew :app:assembleDebug`

## 7. End-to-end verification and specs

- [x] 7.1 Play a full online game iOS↔Android over the incremental path (with a slow-link simulation on one side) and confirm colors, clocks, moves, and game end stay correct, and that payload/memory no longer grow with move count
- [x] 7.2 Sync the `server`/`ios`/`android` deltas into the main specs and validate; verify `openspec validate --specs` reports no errors
