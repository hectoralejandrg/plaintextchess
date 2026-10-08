# Design: Player Authentication / Profiles

## Context
The server (`add-sqlite-persistence`) already has a durable SQLite database for ratings (`players`, `games`, `rating_history`) and a `SqliteGameRecorder` adapter that shares a single-connection SQLite pool. The new `player-auth/profiles` capability reuses that same database and connection pool: profiles are stored in a new `profiles` table (`device_id PRIMARY KEY`, `display_name TEXT`, `created_at_ms INTEGER`), loaded at `App::init` startup (alongside ratings), and updated durably at profile creation/update.

## Goals / Non-Goals
- **Goals:** A persistent player profile (tied to the existing `device_id`) survives server restarts, is loaded at startup, and is recognized on reconnect without treating the player as a new unknown device.
- **Non-Goals:** No new FFI surface, no wire-protocol change (`v:1` unchanged), no OAuth/API tokens, no deployment configuration changes.

## Design Decisions

### D8. Profile table in the existing SQLite database
The `profiles` table lives in the same SQLite file as ratings (`DATABASE_URL`). The schema is managed by the same embedded `sqlx::migrate!` mechanism (`0002_profiles.sql` added to `server/migrations/`), running at `App::init`. Loading profiles follows the same pattern as loading ratings: `SELECT` at startup → seed into an in-memory `ProfileStore` adapter.

### D9. In-memory `ProfileStore` adapter (runtime authority)
Similar to `RatingStore`: the database is the durable source; the in-memory adapter (`ProfileStore`) is the runtime authority. `App::new` (tests) seeds an empty store; `App::init` loads from DB. The adapter exposes `profile_of(&str) -> Option<Profile>` and `create_profile(&str, Profile)` which writes to the database via the shared pool.

### D10. Reuse the `SqliteGameRecorder` pool for profile writes
`ProfileStore::create_profile` uses the same `SqlitePool` (cloned from the `App`) and commits through a durable adapter (`ProfileRecorder`) or through the same `SqliteGameRecorder` mechanism (`record_profile_change`). To keep the design minimal, `ProfileStore` writes through the `SqlitePool` directly (a new `ProfileStore::commit_profile` method), sharing the single-connection pool but keeping profile commits separate from finished-game commits (no intermixing of profile and game transactions, avoiding complex multi-table transactions unless needed).

### D11. Profile creation on first `Connect`
In `handle_connect`, before seating the player (for both `code: None` — creator — and `code: Some(...)` — join/re-attach), the actor checks `ProfileStore::profile_of(&player_id)`. If `None`, it creates the profile (`ProfileStore::create_profile`) and awaits the database commit. On `Err`, it logs and continues (same D7 principle: never block the connection for DB failure). The profile's `created_at` is stamped at commit time.

### D12. Profile reload at startup (same as D1 for ratings)
`App::init` loads both `players` and `profiles` at startup: `SELECT device_id, display_name, created_at_ms FROM profiles`. The `ProfileStore` adapter seeds them. If the database file is missing or the table doesn't exist yet (`App::init` runs migrations first, creating it), the load returns no rows — which is fine (`ProfileStore` is empty, profiles are created on first connect).
