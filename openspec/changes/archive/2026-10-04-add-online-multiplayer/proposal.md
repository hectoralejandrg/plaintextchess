# Proposal

## Why

The app is currently local-only (two players on one device, or human vs CPU). Players cannot challenge each other remotely. Adding online play — a server-authoritative Rust backend plus an "Online" mode in both apps — lets two devices play a real game over the network while reusing the existing core, board rendering, move list, and game-end dialog.

## What Changes

- **New `server/` crate**: a Rust (axum + tokio + tokio-tungstenite) WebSocket server that uses `chess-core` (native `rlib` path dependency) as the authoritative game engine. Hosts in-memory two-player rooms with 6-character join codes, validates every move against the core, broadcasts full state snapshots to both players, enforces one active room per device, handles disconnect/reconnect with a grace window and forfeit-on-timeout, and keeps a per-device Glicko2 rating (core `RatingManager`) that is updated when a game ends by checkmate, resignation, or forfeit. No database: rooms and ratings are in-memory and reset on server restart (documented limitation).
- **Shared online protocol**: versioned JSON messages over WebSocket (`v: 1`), a structured error envelope with stable error codes, and a full-state snapshot contract so clients can always resync.
- **iOS "Online" mode**: third option in the new-game sheet (create room / join with code), a connection state machine (connecting → waiting for opponent → in game → reconnecting), a locally-mirrored `GameSession` that renders server-confirmed moves through the existing board/move-list/status UI, move submission from the existing tap/drag/promotion flow, a turn indicator, the game-end modal with player-perspective wording, Undo disabled, Resign available at any point during the game, and a reconnect banner with automatic re-attach. A persistent anonymous device ID carries the rating. DEBUG-only server-URL override.
- **Android "Online" mode**: exact mirror of the iOS online mode (same protocol, same behavior, OkHttp WebSocket).
- **CI**: new `build-server` job (cargo build + test for `server/`) added to `build-validation.yml`.
- **Out of scope** (future changes): accounts/login, matchmaking queues, turn timers, rematch-with-same-opponent, persistent rating storage, deployed production URL (the client compiles with a server-URL constant + DEBUG override; deployment of the server is a follow-up ops change).

## Capabilities

### New Capabilities

- `server`: the online game server — room management, server-authoritative games, disconnect/reconnect/forfeit, per-device online rating, and health/configuration.

### Modified Capabilities

- `ios`: "iOS Game Mode Selection" gains the Online option; "iOS Game Controls" gains online availability rules (Undo unavailable, Resign available at any point during the game); "iOS Game-End Dialog" gains online result wording; new "iOS Online Multiplayer" requirement.
- `android`: mirror of the iOS modifications and new "Android Online Multiplayer" requirement.
- `shared`: new "Online Multiplayer Protocol" requirement defining the message contract both clients and the server implement.

## Impact

- **New code**: `server/` crate (protocol, rooms, engine wiring, rating, tests); new networking + online-state code in `ios/app/PlainTextChess` (new files + edits to `GameViewModel.swift`/`ContentView.swift`) and the Android `com.hectoralejandrg.plaintextchess` package (`GameViewModel.kt`, `MainActivity.kt` + new files); new `build-server` CI job.
- **Core**: no changes to `core/src` or the UniFFI surface; the server consumes the existing Rust API (`new_game_session`, `play_move`, `get_board_state`, `update_player_rating`, …).
- **Dependencies**: server crate gains `axum`, `tokio`, `tokio-tungstenite`, `serde`/`serde_json`, `tower`/`tower-http` (testing), `uuid`/`rand` (device ID / room codes), `tracing`; Android app gains OkHttp; iOS uses the system `URLSessionWebSocketTask` (no new pod/SPM dependency).
- **Compatibility**: two-player and CPU modes are unchanged; the single-active-game gate applies to online games too. **No breaking changes** to the FFI or existing local play.
