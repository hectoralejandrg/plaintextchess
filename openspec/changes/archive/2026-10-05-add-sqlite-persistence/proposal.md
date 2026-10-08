# Proposal

## Why

Online ratings and finished games are lost when the server restarts — the
documented "in-memory MVP" limitation. Going to production needs durable
per-device Glicko-2 state (rating, deviation, volatility) and an auditable
finished-game history. SQLite via `sqlx` is the right first step: zero-ops
local durability with no CI service, and the same `sqlx` API lets the project
move to PostgreSQL later by changing the database URL, not the code.

## What Changes

- The server persists, in a local SQLite database (WAL mode), the full
  per-device Glicko-2 state and a record of every finished online game
  (result, winner, time control, device ids, move count, ratings before/after,
  rating history).
- Ratings are loaded from the database at startup; the in-memory store stays
  the runtime authority and the move/clock hot path is untouched — durability
  commits happen once, at game end, in a single idempotent transaction.
- New application port `GameRecorder` plus a `SqliteGameRecorder`
  infrastructure adapter; a `NoopGameRecorder` keeps tests and in-memory runs
  behaving exactly as today.
- New `DATABASE_URL` environment setting (default
  `sqlite:./data/chess-server.db`, parent directory auto-created); schema is
  managed by embedded `sqlx` migrations and written to stay portable to
  PostgreSQL.
- The Rust core exposes its Glicko-2 rating snapshot (get/restore) at crate
  level so the server can persist and restore the full rating state. The UniFFI
  surface exposed to the mobile clients is unchanged.
- No wire protocol change (`v:1` unchanged, no client work). No breaking
  changes: today's in-memory behavior remains available when persistence is
  disabled (the test path).

## Capabilities

### New Capabilities

- `rust-core`: crate-level exposure of the Glicko-2 rating snapshot
  (read/restore) for server-side persistence; the FFI surface stays unchanged
  (complements the existing `architecture.md`).

### Modified Capabilities

- `server`:
  - "Server Online Rating" — ratings MUST survive server restarts instead of
    resetting to 1500.
  - "Server Health and Configuration" — new `DATABASE_URL` environment
    setting; the "starts without a database" sentence changes (persistence is
    on by default; an unreachable configured database fails startup).
  - New "Server Persistence" requirement — durable finished-game + rating
    state recording, startup load, idempotent single-transaction commit,
    and commit-failure semantics.

## Impact

- **core** (`core/src/lib.rs`, `core/src/domain/rating.rs`): crate-level
  (non-FFI) rating-snapshot APIs + unit tests.
- **server** (`server/`): new `sqlx` dependency (sqlite, tokio runtime,
  migrate) and `server/migrations/0001_initial.sql`; new ports
  (`RatingState`, `RatingSession::state`, `ChessEngine::new_rating_session_state`,
  `Ratings::rating_state`, `GameRecorder`) and domain `FinishedGame` record;
  `RoomServices` gains a recorder and the terminal paths become async;
  `App::init` (async startup: pool, migrations, rating load, recorder),
  `Config.database_url`, `main.rs` wiring; integration tests for
  restart-persistence and idempotency.
- **CI / deployment**: no new services (embedded SQLite); only a note that a
  hosting platform should give the process a persistent volume for `data/`.
- **Not changed**: wire protocol, iOS/Android apps, room/clock behavior,
  reconnect grace, active-game state (still in memory — out of scope).
