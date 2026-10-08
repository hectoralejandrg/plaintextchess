# Player Authentication / Profiles Spec (Delta)

## Purpose
A new capability for persistent player profiles tied to the same `device_id` already used for Glicko-2 ratings. The profile survives server restarts through the SQLite persistence layer (`add-sqlite-persistence`).

## Requirements

### Requirement: Persistent player profile
The server MUST maintain a persistent profile for each `device_id` (the same identifier used for ratings). The profile MUST include at minimum: the `device_id`, a `display_name` (optional, default empty), and a `created_at` timestamp (wall-clock ms at creation). The profile MUST be stored in the SQLite database (`profiles` table, with `device_id` as PRIMARY KEY) and loaded at server startup into memory, so that reconnecting players are recognized by their profile, not treated as unknown devices.

When a player connects (`Connect` message with `player_id` = `device_id`), the server MUST load or create the profile for that device. The profile MUST survive server restarts: the database file (`DATABASE_URL`) must contain the `profiles` table, and `App::init` must load it at startup. The profile creation/update MUST be committed durably (through the same idempotent SQLite transaction mechanism as finished games, or through a separate durable adapter that shares the database pool). A failed profile commit MUST be logged but MUST NOT block the player's connection or the room actor (design principle carried from D7).

### Requirement: Profile creation on first connection
- **WHEN** a device identifier connects for the first time (no existing `profiles` row for that `device_id`)
- **THEN** the server creates the profile with the default `display_name` (empty string) and stamps `created_at` at the current wall-clock ms, commits it, and continues with the connection

### Requirement: Profile reload across restarts
- **WHEN** the server restarts against the same `DATABASE_URL`
- **THEN** the `profiles` table is loaded at startup, and reconnecting players receive their existing profile (not a new one)
