# Design

## Context

The online server (`server/`, single Rust process) already runs a clean
architecture: `domain/` (pure: clocks, material, time control, room codes)
← `application/` (ports + room actor) ← `infrastructure/` (App, engine,
ratings, config, ws) ← `interface/` (wire + pin test). Ratings live in an
in-memory `RatingStore` behind the `Ratings` port, backed by per-device
Glicko-2 sessions (`RatingSession` port → core `GameSession`). The core's
`RatingManager` already serializes its full state
(`rating:<r>,deviation:<d>,volatility:<v>`) but exposes it only through an
internal `GameSession::serialize()` that mixes in the board FEN. Terminal
transitions (move/resign/leave/flag/forfeit) all funnel through the room
actor's single `apply_terminal_ratings(winner_idx)` helper, which has the
room code, time control, move list, status, and both seats in hand.
See proposal.md for motivation.

## Goals / Non-Goals

**Goals:**
- Ratings (full Glicko-2 state) and finished-game history survive restarts.
- Zero hot-path impact: no database work on the move/clock/snapshot path.
- Exactly-once rating application per finished game, and idempotent,
  crash-tolerant commits.
- The schema and data access stay portable to PostgreSQL (later, same
  `sqlx` API).
- Today's test suite and in-memory behavior stay byte-identical when
  persistence is disabled.

**Non-Goals:**
- Persisting/resuming in-progress games (active room state, clocks, move
  logs) — a later phase; a restart still drops in-progress games.
- Multi-node operation, sticky routing, or the Postgres cutover itself.
- Admin/HTTP read APIs over the history; client-visible changes (no wire
  change); device authentication (spoofable `player_id` remains a known
  limitation); hot-path lock optimizations from the capacity analysis.

## Decisions

### D1 — In-memory stays authoritative; SQLite is a durable write-behind copy
The `RatingStore` keeps owning the runtime rating state (the room actor's
`rating_of` calls and snapshot ratings never touch the DB). The database is
touched at exactly two moments: **startup** (load persisted states into the
in-memory store) and **game end** (commit the finished game).
*Rationale:* the hot path is latency-critical and already O(rooms) woken
every 200 ms; durability work at the two rare, well-defined boundaries keeps
latency, testability, and the existing architecture intact.
*Alternatives:* read-through from the DB per snapshot (adds a DB hop to every
snapshot, plus the known `rating_of` write-lock issue — rejected); a fully
DB-backed rating store (same, plus more lock surface — rejected for phase 1).

### D2 — Persist the full Glicko-2 state, not just the rating
Persist `rating`, `rating_deviation`, `volatility` per device. A rating-only
copy would silently reset deviation/volatility to defaults on every restart
(Glicko-2 math degrades: RD stays pinned at 200). The core already knows its
full state (`RatingManager::serialize/deserialize`), so the core gains two
**crate-level, non-FFI** APIs on `GameSession`:
- `get_rating_state(&self) -> RatingSnapshot { rating, deviation, volatility }`
- `new_game_session_from_rating_state(snapshot) -> Arc<GameSession>`
(crate-level constructor; internally `new_game_session` + a rating-only
restore so the board starts fresh).
The UniFFI surface is untouched → no client rebuild, no `ffi-contracts`
change. *Alternatives:* parse the opaque `serialize()` string in the server
(couples the server to an internal string format — rejected); extend the FFI
surface (client-visible change with zero client need — rejected).

### D3 — `GameRecorder` port; commit inline at the terminal funnel
New application port `GameRecorder` with one async method, `record_finished_game(&FinishedGame)`;
new domain record `FinishedGame { game_id, room_code, time_control,
white_device, black_device, status, move_count, white_rating_before,
black_rating_before, white_state, black_state }` (post-game `RatingState`s,
both colors). The actor's single `apply_terminal_ratings` funnel becomes:
capture pre-ratings → in-memory `apply_result` → read both post-game
`RatingState`s via a new `Ratings::rating_state` method → build
`FinishedGame` (`game_id` = UUIDv4) → `await recorder.record_finished_game`
→ then send the final snapshot as today. `RoomServices` gains
`recorder: Arc<dyn GameRecorder>`; persistence-off wiring (tests,
`Config::for_test`) uses a `NoopGameRecorder` — a required field plus a Noop
keeps the actor branch-free and makes "recording happened" a one-line test
assertion. The terminal handlers (`handle_move`'s terminal branch,
`handle_resign`, `handle_leave`'s resign branch, `settle_flag`,
`handle_grace_expiry`, and transitively `tick_flags`) become `async`; the
`select!` loop awaits them. The 200 ms tick and grace timers are unchanged.
*Alternatives:* fire-and-forget `tokio::spawn` with retries (decouples more,
but the commit lands *after* the client sees the result — a crash in that
window loses a shown result, and retry bookkeeping adds state; rejected);
making `Ratings::apply_result` itself async and decorating the store (moves
recording into infrastructure and loses the room context the record needs —
rejected).

### D4 — One transaction, idempotent, stale-guarded
`SqliteGameRecorder` commits, in a single transaction:
1. `INSERT INTO games (...) ON CONFLICT (game_id) DO NOTHING` — the unique
   `game_id` is the idempotency key (a retried/replayed commit is a no-op).
2. `INSERT INTO players (...) ON CONFLICT (device_id) DO UPDATE SET ...
   WHERE players.updated_at_ms < excluded.updated_at_ms` — a stale commit can
   never overwrite a newer persisted state (guards the near-impossible
   interleave of two same-player finishes committing out of order).
3. `INSERT OR IGNORE INTO rating_history (device_id, game_id, before, after)`
   per player (PK `(device_id, game_id)` → replay-safe).
`ended_at_ms` is stamped by the recorder at commit time (wall clock, unix
ms), keeping the domain record free of timestamps. Exactly-once is structural
(the terminal transition fires exactly once per room — every entry point
checks `is_terminal` first) with the unique key as the crash-retry backstop.
*Alternatives:* a separate "applied ratings" ledger table (extra indirection
for a window that cannot occur in phase 1 — deferred with active-game
persistence); per-statement commits (three points of partial failure —
rejected).

### D5 — `sqlx` + embedded migrations, written to stay Postgres-portable
Dependency: `sqlx` with `runtime-tokio`, `sqlite`, `migrate`. Schema lives in
`server/migrations/0001_initial.sql`, embedded at compile time via
`sqlx::migrate!("migrations")` — no runtime file lookups (PaaS single-binary
friendly, matches the "env-config only" deployment story). The pool is a
single connection (`max_connections(1)`) with `journal_mode=WAL` and
`busy_timeout=5000`: there is exactly one writer (this process) and SQLite
WAL then serves readers without blocking it. Table/SQL choices stay standard
(`TEXT`/`REAL`/`INTEGER`, `ON CONFLICT` — valid in both SQLite and
PostgreSQL) so the later Postgres move is: new driver feature + `DATABASE_URL`
+ possibly a dialect-specific migration — not a rewrite.
*Alternatives:* `rusqlite` + blocking pool (sync driver in an async app, and
no shared path to Postgres — rejected); raw SQL in `#[sqlx::query]`
micro-migrations (compile-time-checked but pinned to one DBMS's exact types —
plain `query`/`execute` keeps the swap cheap; rejected for portability).

### D6 — Configuration and startup
`Config` gains `database_url: Option<String>`:
- `from_env` — `DATABASE_URL` env var, default
  `sqlite:./data/chess-server.db`; the parent directory is created at
  startup (`std::fs::create_dir_all`).
- `for_test` — `None` (persistence disabled) → every existing test keeps
  byte-identical behavior.
New async `App::init(config) -> Result<App, String>`: open pool → run
migrations → `SELECT device_id, rating, rating_deviation, volatility FROM
players` → seed the `RatingStore` with `new_rating_session_state` per row →
attach the `SqliteGameRecorder`. `App::new` (sync, no DB) stays for tests.
**Fail-fast:** a configured database that cannot be opened or migrated fails
`App::init`; `main.rs` logs a clear error and exits non-zero. No silent
fallback to in-memory: a deployment that asked for persistence must not lose
ratings unnoticed.

### D7 — Failure semantics (documented loss window)
The commit is awaited **before** the final snapshot is sent, so a client only
ever sees a result the server has made durable — the best ordering without
blocking the game. If the commit fails (DB down/corrupt), the error is
logged and the final snapshot is still delivered with the in-memory ratings:
the game is never hostage to the database, and only that one result's
durability is lost. A process crash between the in-memory update and the
commit loses the same single result — and in-progress games are already lost
on restart by design (phase 1), so the loss class is unchanged.

## Schema

`server/migrations/0001_initial.sql` (SQLite):

```sql
CREATE TABLE players (
    device_id        TEXT PRIMARY KEY,
    rating           REAL NOT NULL,
    rating_deviation REAL NOT NULL,
    volatility       REAL NOT NULL,
    updated_at_ms    INTEGER NOT NULL
);

CREATE TABLE games (
    game_id             TEXT PRIMARY KEY,   -- UUIDv4, idempotency key
    room_code           TEXT NOT NULL,
    time_control        TEXT NOT NULL,      -- preset label, e.g. "15+10"
    white_device        TEXT NOT NULL,
    black_device        TEXT NOT NULL,
    status              TEXT NOT NULL,      -- terminal status wire name
    winner              TEXT,               -- 'white' | 'black' | NULL (draw)
    move_count          INTEGER NOT NULL,
    white_rating_before REAL NOT NULL,
    black_rating_before REAL NOT NULL,
    white_rating_after  REAL NOT NULL,
    black_rating_after  REAL NOT NULL,
    ended_at_ms         INTEGER NOT NULL    -- wall clock, unix ms
);
CREATE INDEX games_white_device_idx ON games (white_device, ended_at_ms);
CREATE INDEX games_black_device_idx ON games (black_device, ended_at_ms);

CREATE TABLE rating_history (
    device_id     TEXT NOT NULL,
    game_id       TEXT NOT NULL REFERENCES games (game_id),
    rating_before REAL NOT NULL,
    rating_after  REAL NOT NULL,
    PRIMARY KEY (device_id, game_id)
);
```

Growth note: ~1 game row + ~2 history rows per finished game (well under a
kilobyte each). Millions of games fit in low-GBs; retention/archival is a
post-MVP concern (a `DELETE FROM games WHERE ended_at_ms < ...` job, no
schema need).

## Risks / Trade-offs

- [Commit fails and the result's durability is lost] → logged with the full
  `FinishedGame` payload so an operator can re-apply by hand; the unique
  `game_id` makes any re-apply idempotent.
- [Crash between in-memory update and commit] → same loss class as today's
  "restart drops everything"; bounded to one finished game; closed by
  active-game persistence (out of scope).
- [sqlite compile-time cost: `sqlx` + migrations add build time] → one-time
  cost on cold builds; CI is already multi-minute and no service is needed.
- [`data/` on a PaaS must be a persistent volume] → documented deployment
  note (the server already assumes a writable CWD for nothing else; fail-fast
  at D6 makes a wrong volume obvious at boot, not at first game end).
- [Room code is not unique across time, so it cannot be an idempotency key]
  → `game_id` UUIDv4 instead; `room_code` kept on the row only for audit.
- [Two same-player games finish concurrently and commits interleave] →
  D4's `updated_at_ms` guard makes the stale commit a no-op; the in-memory
  order remains the source of truth within a run.

## Migration Plan

Pure addition: no data migration. Deploy = ship the new build; the migration
runs on first boot and creates the file. Rollback = deploy the previous
build (it ignores the database file entirely; no destructive rollback needed).
The only behavioral diff after rollout: ratings survive restarts.

## Open Questions

- None: the phase-2 questions (in-progress-game resume, move logging,
  Postgres cutover, retention policy) are explicitly out of scope and will
  get their own change.
