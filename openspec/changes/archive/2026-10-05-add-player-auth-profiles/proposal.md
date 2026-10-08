# Proposal: Player Authentication and Profiles (New Capability)

## Why
The online server already persists per-device Glicko-2 ratings (`device_id`) and finished-game history in SQLite (`add-sqlite-persistence`). The next step is adding persistent player profiles: a player creates a profile (registration/login tied to their device identifier), and the server maintains that profile across restarts with the same `device_id`. This aligns with the user's clarification: "autenticación/perfiles de jugador (nuevo capability): registro/login con dispositivo/perfil persistente".

## What Changes
- A new `player-auth/profiles` capability (spec delta) defining the profile contract: `device_id` as the persistent key, a `profile` record with a `created_at` timestamp and optional `display_name`, and the profile must survive server restarts (loaded at startup from SQLite, committed at profile creation/update).
- A new application-level port `ProfileStore` (similar to `Ratings`) exposed through an updated `RoomServices` so the room actor can read/write profile data when a player connects.
- A `ProfileRecorder` adapter using the existing SQLite connection (reuse the `SqliteGameRecorder`'s pool or share it) so profile operations are durable.
- The profile is linked to the device identifier already used for ratings; no new wire-message format is needed (the existing `player_id` in `ClientMessage` maps to `device_id` in the profile table).
- The `App::init` startup path loads profiles (if any exist) into memory, and `App::new` continues the persistence-free test path with an empty profile store.

## Scope (Phase for this proposal only — no implementation here)
- Planning artifacts only: proposal, spec (`specs/player-auth/profiles/spec.md`), brief design (`design.md`), and tasks (`tasks.md`).
- Implementation awaits an explicit `/opsx-apply add-player-auth-profiles` request.

## Non-Goals (explicit exclusions for this proposal)
- No new FFI surface (UniFFI unchanged; mobile clients unaffected).
- No new wire-protocol messages or versions (`v:1` unchanged).
- No deployment changes (Docker, hosting configs) in this proposal.
- No admin APIs or device authentication tokens (OAuth/API keys) — profile is tied to the existing `device_id` used for ratings.
