# Proposal: Player Accounts, Login, and Sessions

## Why

The online server identifies every player by a self-asserted `device_id` string that the client invents locally and the server never validates (`server/src/interface/protocol.rs`: `CreateRoom { player_id }` / `JoinRoom { player_id }`). That identifier is the sole key for the seat, the one-room-per-device rule, and the persisted Glicko-2 rating (`players.device_id`). Two consequences block any real account work:

1. **Any client can steal a seat.** `handle_connect` re-attaches by string equality on `player_id` (`room_actor.rs`: `seat_index_of` → `seat.out = out`), so replaying another player's UUID in a `join_room` frame takes over their seat, their clock, and their game.
2. **A player's identity is one device.** Ratings are keyed by `device_id`, so a player who reinstalls the app, switches phones, or wants to play from an iPad starts again at 1500 with an empty history, and there is nowhere to keep a display name.

This change introduces optional player accounts: a player registers a username and password once, logs in to obtain a session token, and that session follows the account across devices. Guest play (today's behavior) keeps working unchanged for every client that does not log in.

## What Changes

- **New capability `player-auth`** covering four contracts:
  - **Accounts**: registration creates an `account` keyed by a server-generated `account_id`, with a case-insensitively unique `username` and an argon2id `password_hash`. Credentials are only ever stored hashed; the plaintext password is never persisted, logged, or echoed.
  - **Sessions**: a successful login issues an opaque bearer token. Only a hash of the token is stored. Sessions expire after a configurable window, can be revoked explicitly, and are re-mapped to the connecting device on use.
  - **Device-to-account linkage**: a `profiles` table binds each `device_id` to an `account_id`, so a device that logs in adopts the account's display name and the account becomes reachable from any device. A device never belongs to two accounts at once.
  - **Guest fallback**: a connection with no valid session keeps today's `device_id` identity and gameplay untouched.
- **New WebSocket messages**, additive to the existing protocol: client `register`, `login`, and `logout`; server `session` (the issued token and the profile) and a `session_ok` acknowledgement. `KNOWN_TYPES` grows to match. `VERSION` stays `1` — following the existing `time_control: Option<String>` precedent, every new field is optional or lives in a new message type, so existing clients are unaffected.
- **Seat and room occupancy become session-aware.** An authenticated player is recognized by their account, so re-attaching to an occupied seat from a *different* device is allowed for that account and refused for everyone else; the one-room-at-a-time rule is enforced per device *and* per account.
- **Password work needs a new dependency**: `argon2` (plus `password-hash` re-exports). Passwords are bounded to a sane length before hashing to bound CPU cost.
- **New configuration**: `SESSION_TTL_SECS` (token lifetime) and `ARGON2_*` cost knobs, with built-in defaults and a minimum enforced at startup.
- **Two new tables** (`accounts`, `sessions`) plus one new table (`profiles`) in the existing SQLite file, added by migration `0002_player_auth.sql` and loaded at `App::init`, reusing the existing single-connection WAL pool.

**Not modified on purpose:** ratings keep their existing `device_id` key. `players`, `games`, and `rating_history` are left byte-for-byte as they are, so there is **no data migration** of the existing rating history. A player's rating therefore stays per-device for now; keying it by `account_id` is a deliberate follow-up (listed under Non-Goals).

## Capabilities

### New Capabilities
- `player-auth`: Player accounts (registration with unique username and hashed password), login issuing opaque bearer sessions with expiry and revocation, device-to-account profile linkage, and the guest fallback that keeps unauthenticated `device_id` play working. Covers the new `accounts`, `profiles`, and `sessions` tables, the session-aware identity rules, and the `register`/`login`/`logout`/`session` wire messages.

### Modified Capabilities
- `server`: Three requirements change at the spec level.
  - **Server Room Management** — "at most one room at a time" currently keys on the device only; it now also applies to the authenticated account, so one account cannot hold two rooms through two devices.
  - **Server Disconnect, Reconnect, and Forfeit** — re-attachment is currently "matched by the player's device identifier". It becomes identity-based: the same account may reclaim its seat from another device, and a session token (not a replayed `player_id`) is what proves the player is who the server expects.
  - **Server Health and Configuration** — adds `SESSION_TTL_SECS` and the argon2 cost configuration to the environment contract, with safe defaults and a validated minimum.
  - **Server Persistence** — the schema it owns now includes the `accounts`, `profiles`, and `sessions` tables, and auth failures follow the same log-and-continue rule as failed game commits.

`Server Online Rating` is deliberately **not** modified: it already keys ratings by the device identifier, and this change does not move that key.

## Impact

**Server (Rust)**
- `server/src/interface/protocol.rs` — new `ClientMessage::{Register, Login, Logout}`, new `ServerMessage::{Session, SessionOk}`, `KNOWN_TYPES` entries, three new `ErrorCode`s (`invalid_credentials`, `username_taken`, `not_authenticated`). The `wire_form_is_stable` test must be extended, not relaxed.
- `server/src/application/ports.rs` — new `Accounts` and `Sessions` ports plus a `ProfileStore`, and a new `RoomServices` field (also touching `App::services` and every test that builds `RoomServices`).
- `server/src/domain/` — new `Account`, `Session`, `Profile` records and a `Credentials`/password-policy value object.
- `server/src/infrastructure/` — a new SQLite-backed adapter reusing `SqliteGameRecorder::connect`'s pool (it is currently private to that type and must become shareable), a `SystemTimeSource`-backed session clock, `App::init` loading auth state, and `App::new` gaining a no-op default.
- `server/src/infrastructure/ws.rs` — dispatch the three new client messages; identity resolution moves to accept a server-verified account where a session exists.
- `server/migrations/0002_player_auth.sql` — new.
- `server/Cargo.toml` — add `argon2`.
- `server/tests/` — integration coverage for register/login/logout, session expiry and revocation, cross-device re-attach, account room limits, and guest play being unaffected; `server/tests/persistence.rs` extends to the new tables.

**Clients (iOS / Android)**
- No code changes and **no spec changes** in this change: guest play stays behaviorally identical, so no `ios`/`android` requirement changes. The new messages are additive and unused by existing clients.

**Docs**
- `docs/admin-guide.md` — the new environment variables and the auth tables.

**Risks**
- Credential storage and password hashing are security-sensitive; the argon2 parameters and the token entropy are the parts most worth reviewing.
- Session-aware seat re-attach is the one place where a bug hands a player someone else's game, so it gets dedicated tests.
