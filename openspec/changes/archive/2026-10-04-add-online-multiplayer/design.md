# Design

## Context

See proposal.md — Why. Current state, from the repo:

- **Core** (`core/`, crate `chess-core`): `new_game_session(initial_rating) -> Arc<GameSession>`; each session owns `Mutex<BoardManager>` + `Mutex<RatingManager>` (Glicko2, `update_player_rating(opponent, result)` with 1.0/0.5/0.0); pure query `get_cpu_move`. `crate-type = ["rlib", "cdylib", "staticlib"]`, so a Rust process can depend on it natively. No global mutable state (pinned by the integration tests added in `enforce-single-active-game`).
- **iOS** (`ios/app/PlainTextChess/`): SwiftUI; `GameViewModel` owns one `GameSession` (mirror of the single-active-game work: `canStartNewGame` gate, `cpuGeneration`, session-identity guard, game-end modal bookkeeping). Board/move-list/status/animation all render from that session. New-game setup sheet already chooses Two players / CPU + difficulty. DEBUG launch-arg hooks exist (`-PLAINTCHESS_SCRIPT`, `-PLAINTCHESS_CPU_DELAY`, …).
- **Android** (`android/app/.../com/hectoralejandrg.plaintextchess/`): Compose; line-for-line mirror of the iOS VM (coroutines instead of GCD). Setup sheet with chips. DEBUG intent-extra hook exists (`cpu_delay_ms`, gated by `FLAG_DEBUGGABLE` because the module has no `BuildConfig`).
- **CI**: one workflow (`build-validation.yml`) with jobs `validate`, `build-ios`, `build-android`, `quality`; builds go through `scripts/build-*.sh`; `tests/validate-*.sh` check setups.
- No Cargo workspace at the repo root; `core/` is standalone (its own `target/`).

## Goals / Non-Goals

**Goals:**
- Two devices play a full game (moves, promotion, resign, checkmate/draw detection, ratings) against a server that is the single source of truth.
- Maximum reuse of the existing client stack: the board view, move list, status row, animation, promotion picker, flip, and game-end modal are driven by a locally-mirrored session, so online rendering is byte-identical to local rendering.
- The server is verifiable headlessly (`cargo test` incl. real-socket integration tests) and the full E2E is verifiable locally (server on the Mac + iOS simulator + Android emulator).

**Non-Goals:**
- Accounts/login, matchmaking queues, turn timers, rematch-with-same-opponent, persistent rating storage, a deployed production URL, spectator mode, and any change to the core's public FFI surface.

## Decisions

### D1 — Server crate: axum + tokio + tokio-tungstenite, core as native dependency
New `server/` crate (`chess-server`) with `chess-core = { path = "../core" }` (rlib). axum serves `GET /healthz` and upgrades `/ws` to WebSocket (tokio-tungstenite). The server owns one authoritative `Arc<GameSession>` per room.
*Alternatives*: Node/TS server (duplicate the engine, or ship the core as a WASM/native module to the server — worse for "one authoritative engine"); a room-actor-per-connection (overkill at this scale; the core already serializes its own state with internal mutexes, so a registry `Arc<RwLock<RoomRegistry>>` where each handler holds the room lock while calling the core is simple, testable, and correct).

### D2 — Protocol: versioned JSON, structured errors, full snapshots
Text frames only; every message `{"v": 1, "type": ...}`. Client→server: `create_room {player_id}`, `join_room {player_id, room_code}`, `move {uci}`, `resign`, `leave`. Server→client: `room_ready {room_code, your_color, state}`, `state {…snapshot}`, `error {code, message}`, `opponent_disconnected` / `opponent_reconnected` (carried inside snapshots as `opponent_online: bool` for simplicity). A snapshot carries: board FEN (via `get_board_state`), full move list, side to move, status (`playing`/`checkmated(winner)`/`drawn`/`resigned(winner)`/`forfeited(winner)`), `your_color`, both players' ratings, `opponent_online`.
Business-rule failures (`room_not_found`, `room_full`, `already_in_room`, `invalid_room_code`, `not_your_turn`, `illegal_move`, `game_over`, `not_connected`) answer with `error` and keep the socket open; only malformed JSON / unknown type / wrong version closes the socket (close code 4000).
*Alternatives*: binary/delta sync (fragile, no resync story), or let clients validate moves and send results (violates server-authoritative, would allow desync). Full snapshots are tiny (~1 KB), idempotent, and make reconnect trivial.

### D3 — Rooms, codes, seats, device identity
Room code: 6 chars from the 32-char alphabet `ABCDEFGHJKLMNPQRSTUVWXYZ23456789` (no 0/O/1/I), drawn until unique among open rooms. Creator = White, joiner = Black. One room per device: `player_id → room` map; a second create/join returns `already_in_room`. `player_id` is the client's persistent anonymous device UUID (D8). Rooms are removed when both seats are gone (lobby leaver, or post-game when both connections close).

### D4 — Disconnect / reconnect / forfeit
On socket close the seat is marked disconnected and a tokio timer starts (grace window, default 120 s, `RECONNECT_GRACE_SECS` env var). Re-attach = same `player_id` joining the same room code before the timer fires: the connection is re-bound to the same seat, a fresh snapshot is sent, and the opponent is told via `opponent_online` in the next snapshot. Timer expiry during play → forfeit: the connected player wins, ratings update, final snapshot, room removed. Lobby-phase disconnects just drop the room (no result).
*Alternatives*: end immediately on disconnect (punishes flaky mobile networks — worst UX); auto-restart the game (violates the authoritative-history contract).

### D5 — Online ratings: core Glicko2, in-memory per device
The server keeps `HashMap<player_id, Arc<GameSession>>` of *rating sessions* (created with `new_game_session(1500.0)`, never played on). On a terminal result it calls `update_player_rating` on both rating sessions (winner vs loser rating with 1.0/0.0, both 0.5 on a draw) and reads the new ratings into the final snapshot. This reuses the exact Glicko2 logic of local/CPU mode (consistency) with zero new rating code. In-memory only: a server restart resets ratings to 1500 (spec-documented limitation).
*Alternatives*: SQLite/file persistence (out of scope; adds durability + locking complexity for a side-project MVP).

### D6 — Client mirror session (both platforms)
The client's existing `GameViewModel` gains an online mode where its `GameSession` becomes a **mirror**: it is only ever fed moves the server confirmed (same internal apply path that human moves use → board, move list, highlight, animation, status all come for free). Player input (two-tap or drag, incl. the promotion picker producing 5-char UCI) produces a UCI string that is sent to the server instead of applied locally; on `error` the selection is cleared and the server state stays. Input is accepted only when the snapshot says it is the player's turn and the game is in progress.
*Alternatives*: a separate online-only view model with custom rendering (duplicates board/move-list/promotion logic — rejected).

### D7 — Client online UX
- New-game sheet: third option **Online** (after CPU) → sub-screen with **Create game** (shows the room code, large, with copy-to-clipboard, and a waiting state) / **Join game** (6-char code entry, auto-uppercased, error messages for `room_not_found`/`room_full`/`invalid_room_code`).
- Turn indicator: status row shows "White to move"/"Black to move" plus a "Your move" emphasis when it is the player's turn (spec: "clearly indicate").
- Controls: Undo disabled for the whole online game; Resign enabled whenever the game is in progress (even on the opponent's turn — sends `resign`); Flip unchanged.
- Game-end modal: online wording "You won." / "You lost." / "Draw." (player perspective, incl. wins by forfeit); only a Done action (no Play again — a new room is required).
- Reconnect: on socket drop mid-game a banner ("Reconnecting…") overlays the board; auto-retry (exponential backoff, capped) for the duration of the server grace window; on success the snapshot resyncs everything; on `forfeited` the modal appears.

### D8 — Device identity
Persistent UUID v4: iOS in `UserDefaults`, Android in `SharedPreferences` (`device_id`), generated on first launch. Sent as `player_id` in every create/join. No PII; the rating follows this ID for the lifetime of a server run.

### D9 — Server URL: constant + DEBUG override
Each app compiles with a default URL constant (empty in this change — see Open Questions) and a DEBUG-only override: iOS launch arg `-PLAINTCHESS_ONLINE_URL ws://host:port`, Android intent extra `online_url` (gated by `FLAG_DEBUGGABLE`, exactly like `cpu_delay_ms`). Verification uses the local server (`ws://127.0.0.1:8765` from the simulator, `ws://10.0.2.2:8765` from the emulator).

### D10 — CI
New `build-server` job in `build-validation.yml`: ubuntu-latest, stable toolchain, `cargo build --release` + `cargo test` for `server/` (workspace-free invocation: `--manifest-path server/Cargo.toml`, matching how `core/` is treated). Added to `quality`'s `needs` so PRs gate on it; the quality report tolerates the missing metrics file (it already handles missing platform files).

### D11 — E2E verification plan (local, two real clients)
1. `cargo run` the server on the Mac with `PORT=8765` (and `RECONNECT_GRACE_SECS=15` for the forfeit test).
2. iOS simulator (online URL override) creates a room; Android emulator (online URL override `ws://10.0.2.2:8765`) joins with the code.
3. Play Scholar's Mate by tapping on both devices → checkmate modal on both ("You won." / "You lost."), screenshots.
4. Ratings: both clients show updated ratings in the final state.
5. Reconnect: kill the app process mid-game, relaunch within the grace window → re-attach banner + resume.
6. Forfeit: close one client, wait out the 15 s grace → the other shows the forfeit win.
7. Regression: local two-player + CPU still work; medium determinism anchor unchanged on both platforms.

## Risks / Trade-offs

- [In-memory state: rooms + ratings lost on restart] → Accepted for the MVP (spec-documented); the schema (room/seat/snapshot) was designed so persistence can be added later without protocol changes.
- [Plaintext `ws://` during verification] → Localhost-only; a deployed instance MUST be behind TLS (follow-up ops change updates the URL constant to `wss://`). No secrets or PII cross the wire beyond the anonymous device ID and moves.
- [Anyone holding a valid room code can join that game] → Inherent to code-based joining; 6 chars from a 32-char alphabet ≈ 2^30 spaces, fine for casual play. (Accounts would close this — explicitly out of scope.)
- [iOS simulator ↔ host networking] → The simulator shares the host network, so `ws://127.0.0.1` works; the Android emulator needs `10.0.2.2` (host loopback alias). Both handled by the DEBUG URL override; no code difference.
- [OkHttp dependency on Android] → ~1 MB; standard, well-maintained, and the pragmatic choice vs. rolling a raw `java.net` WebSocket with the thread-lifecycle pain.
- [Server CPU search never runs] → The server only validates/queries moves (no `get_cpu_move`), so room handlers are fast even with many rooms.
- [Client shows waiting state before the game] → The mirror session starts empty; the "Waiting for opponent" state is driven by the connection state machine, not by the session, so the existing board view needs only a status-line overlay.

## Migration Plan

No data migration; local two-player/CPU modes are untouched.
1. Ship the server (any host with a Rust runtime, or local for verification).
2. Update the client URL constant (or ship with DEBUG override only, as in this change).
3. Rollback: revert the client commit — the server is stateless toward local players; local play never touched the network.

## Open Questions

- Production hosting choice (Fly.io / Render / VPS) and the real `wss://` URL — intentionally deferred to the follow-up deployment change; the design is platform-agnostic (env-config + `/healthz`).
- Whether the server should ever persist ratings (file/SQLite) — deferred; the rating-session design keeps this additive.
