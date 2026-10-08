# Design: Player Accounts, Login, and Sessions

## Context

See `proposal.md` — Why, for the motivation. What matters for *how*:

- The server has **no auth surface at all**. `player_id` is a self-asserted `String` carried on `CreateRoom`/`JoinRoom` (`server/src/interface/protocol.rs:118-156`), used verbatim as the seat key, the one-room-per-device key (`App.players`, `server.rs:66-67`), and the persisted rating key (`players.device_id`).
- Persistence exists and is settled (`add-sqlite-persistence`): one `SqlitePool` with `max_connections(1)`, WAL, `busy_timeout(5s)`, `create_if_missing(true)`, migrations embedded via `sqlx::migrate!("./migrations")` and run in `App::init` (`server.rs:115-173`). `SqliteGameRecorder::connect` is currently the only thing that can produce that pool, and it is a method on the recorder.
- The established pattern is **in-memory runtime authority + async durable recorder**: `RatingStore` (in-memory, seeded at startup) next to `GameRecorder`/`SqliteGameRecorder`; the recorder's failure is logged and never blocks the player (design D7). Auth follows the same shape so the spec rule "a failed authentication write MUST NOT block play" falls out of the architecture instead of needing error handling at every call site.
- `App::new` (no database) must keep working: the spec requires the server to start without one, and the whole test suite uses it.
- Adding a field to `RoomServices` (`ports.rs:90-97`) fans out to `App::services` plus every test that builds it.
- The router is duplicated in three places (`main.rs:46-49`, `tests/integration.rs:26-32`, `tests/persistence.rs:48-51`). Adding HTTP routes means editing all three; adding WS message types does not.
- Dependencies today: `axum`, `tokio`, `serde`, `serde_json`, `tracing`, `uuid`, `rand` 0.8, `sqlx`. **No crypto or auth crate.**
- `Config::from_env` (`config.rs:25-47`) silently falls back to defaults on unparseable values. For authentication that fallback is a security problem, so auth config gets its own validating parser.

## Goals / Non-Goals

**Goals**
- Registration, login, logout and session validation as new WS messages, with no change to any existing message, field, or the protocol version.
- Credentials and tokens never recoverable from the database or the logs.
- Identity (not an asserted string) decides who may take a seat, and one account cannot hold two rooms.
- Auth works with a database and keeps working without one.
- Zero data migration for existing ratings and game history.

**Non-Goals (design-level)**
- No client-side code. No iOS/Android spec delta: guest behavior is unchanged, so nothing in those specs changes.
- No HTTP/REST auth surface, no cookies, no `Authorization` header, no CORS.
- No email, password reset, account deletion, avatar, friends, or matchmaking.
- No rate limiting, lockout, or brute-force throttling (see Risks — recorded as a follow-up, not silently dropped).
- No move of ratings to `account_id`.

## Decisions

### D1. A separate `account_id` key; `device_id` stays the rating key
Three new tables get their own identity: `accounts(account_id)` is the login identity, `profiles(device_id)` links a device to an account, and `players/games/rating_history` keep `device_id` untouched.

- **Alternative rejected**: migrate `players`, `games.white_device/black_device`, and `rating_history.device_id` to `account_id`. This is the better end state (one rating per human), but it rewrites three tables, the `App::init` load query, and `SqliteGameRecorder::commit`, and it needs a backfill that maps each existing device to an account. That is a separate change with its own rollback story.
- **Alternative rejected**: add `username`/`password_hash` to `players` and treat `device_id` as the account. One table fewer, but every device becomes its own account: signing in on a tablet gives the tablet a fresh 1500 rating and an empty history, which defeats the purpose of having accounts.
- **Trade-off accepted**: until a follow-up moves ratings, one human playing on two devices has two ratings. The *profile* (identity, display name, room limit, re-attach) already crosses devices, which is the user-visible part of this change.

### D2. Schema: one additive migration, no data rewrite
`server/migrations/0002_player_auth.sql`, auto-picked-up by the existing `sqlx::migrate!` and run by `App::init` — no new migration runner.

```sql
CREATE TABLE accounts (
    account_id       TEXT PRIMARY KEY,     -- UUIDv4, server-generated
    username         TEXT NOT NULL,       -- as entered, for display
    username_key     TEXT NOT NULL,       -- case-folded; UNIQUE (see D3)
    password_hash    TEXT NOT NULL,       -- argon2id PHC string
    display_name     TEXT,                -- NULL means "use username"
    created_at_ms    INTEGER NOT NULL
);
CREATE UNIQUE INDEX accounts_username_key_idx ON accounts (username_key);

CREATE TABLE profiles (
    device_id        TEXT PRIMARY KEY,     -- the existing client identifier
    account_id       TEXT NOT NULL REFERENCES accounts (account_id),
    created_at_ms    INTEGER NOT NULL,
    updated_at_ms    INTEGER NOT NULL
);
CREATE INDEX profiles_account_idx ON profiles (account_id);

CREATE TABLE sessions (
    session_id       TEXT PRIMARY KEY,     -- UUIDv4, for logs
    account_id       TEXT NOT NULL REFERENCES accounts (account_id),
    device_id        TEXT NOT NULL,       -- device that logged in
    token_hash       TEXT NOT NULL,       -- SHA-256 of the token; never the token
    created_at_ms    INTEGER NOT NULL,
    expires_at_ms    INTEGER NOT NULL,
    revoked_at_ms    INTEGER
);
CREATE UNIQUE INDEX sessions_token_hash_idx ON sessions (token_hash);
CREATE INDEX sessions_account_idx ON sessions (account_id);
```

`profiles` PK stays `device_id`, which is what makes "a device belongs to at most one account" a database guarantee rather than an application convention. `sessions.token_hash` is unique so a token lookup is an indexed point read. No column of `0001_initial.sql` is altered, so the migration is forward-only and reversible by dropping three tables (Migration Plan below).

### D3. Case-insensitive username uniqueness via an explicit `username_key`
Uniqueness is enforced by `accounts.username_key`, computed by lowercasing the ASCII letters of the entered username (Unicode `to_lowercase()` where it changes the string). `accounts.username` keeps the entered casing for display.

- **Alternative rejected**: rely on SQLite's `COLLATE NOCASE`. It folds ASCII only, so `Änne` and `änne` would be two different accounts to the index while a client comparing them as strings may reasonably assume they collide. An explicit fold makes the rule visible in the schema and portable to the eventual Postgres cutover the `0001` header anticipates.
- The fold is a lookup key only. Validation (length 3-24, charset) runs on the *entered* username, so a rejection message quotes what the player typed.

### D4. Ports mirror `Ratings`/`GameRecorder`: in-memory authority + async durable recorder
Three new traits in `application/ports.rs`, all with a no-database implementation:

- `Accounts` — synchronous reads/writes against in-memory state (`account_by_username`, `account_by_id`, `insert_account`, `account_id_of_device`, `link_device`, `touch_account`).
- `Sessions` — `session_by_token_hash`, `insert_session`, `revoke_session`, `purge_expired`.
- `ProfileStore` — `profile_of(device_id)`, `create_profile`.

Plus one async `AuthRecorder` port (`record_account_created`, `record_device_linked`, `record_session_issued`, `record_session_revoked`) returning `Result<(), RecorderError>`, implemented by a new `SqliteAuthRecorder` and a `NoopAuthRecorder`.

- **Why this shape**: it is the same split `add-sqlite-persistence` already uses for ratings, and it makes three spec requirements structural instead of incidental — "a failed authentication write MUST NOT block play" falls out because the actor awaits a recorder future whose error is only logged; "the server MUST start without a database" falls out because the in-memory adapters need no pool; and the token hash never leaves the `Sessions` port, so no caller can accidentally log it.
- **Why not a single `AuthStore` port with the pool inside**: it would put `await` and SQL in the actor's critical path and give every caller an error path to forget to swallow.
- **Why three ports instead of one**: the session token hash is a secret; keeping `Sessions` separate means `RoomServices` consumers that only need profile or room-occupancy data are never handed a token lookup by accident.

`RoomServices` gains a single `auth: Arc<dyn PlayerAuth>` field, where `PlayerAuth: Accounts + Sessions + ProfileStore + Send + Sync` — one field instead of three, so the `RoomServices` fan-out (production plus the test constructions) is edited once. `App::new` supplies an in-memory `PlayerAuth`; `App::init` additionally attaches the SQLite recorder.

### D5. Argon2id for passwords, hashed off the async runtime
Add the `argon2` crate (argon2id, PHC string format, `password-hash` re-exported). Defaults are the OWASP baseline: 19456 KiB memory, 2 iterations, parallelism 1, overridable by `ARGON2_MEMORY_KIB`, `ARGON2_TIME_COST`, `ARGON2_PARALLELISM`. Startup rejects `memory_kib < 8192`, `time_cost < 1`, `parallelism < 1` with an explicit error.

Password length is validated (8-128) **before** any hashing, which is what satisfies the spec's "length-bounded before it is hashed". Hashing and verification run inside `tokio::task::spawn_blocking`, never inline in an async task.

- **Why argon2id over bcrypt**: bcrypt silently truncates at 72 bytes, so a long passphrase loses its tail — bad for a feature whose whole point is accepting arbitrary passphrases. bcrypt also needs a hand-rolled pre-hash to avoid truncation. Argon2id is memory-hard, the current recommendation, and has one crate covering both hashing and verification.
- **Why argon2id over scrypt**: comparable safety; scrypt's cost is expressed as a single `log N` knob that is easy to mis-set into an unsafe configuration. Argon2id separates memory, iterations, and parallelism, so the minimum checks in D5 map one-to-one onto config keys.
- **Why `spawn_blocking`**: argon2 at the default cost takes on the order of tens of milliseconds. Run inline in a Tokio worker it would stall unrelated tasks — including other players' WebSocket reads — and with `max_connections(1)` any accidental overlap with pool work would compound the stall. `spawn_blocking` keeps both the runtime and the pool free, and it also means a slow hash never holds the single connection.

### D6. Opaque bearer tokens, stored as SHA-256
A session token is 32 bytes from the existing `rand` dependency, encoded base64url without padding (43 chars). Only `SHA-256(token)` is persisted.

- **Why not store the token itself**: a database leak would then be a set of immediately usable live sessions. Hashing costs nothing here because the token has 256 bits of entropy — there is no search to slow down, which is exactly the argument that *does* apply to passwords and justifies argon2 there and SHA-256 here.
- **Why not a JWT**: the token would need a signing secret, adding a new secret-management requirement to a server that today has no secrets at all; and revocation without extra server state is not possible, whereas revocation is an explicit spec requirement (`logout`) and an obvious user expectation. The database is right there.
- **Why not argon2 the token**: it would make every authenticated `join_room` pay a memory-hard hash, putting ~10s of ms of CPU on the room path for no security gain at this entropy.

### D7. Lazy expiry, evaluated at use; opportunistic cleanup at startup
`expires_at_ms = created_at_ms + SESSION_TTL_SECS * 1000`, evaluated on every lookup. `SESSION_TTL_SECS` defaults to 30 days, and startup rejects a value below 60. Expired rows are deleted once during `App::init` startup rather than by a background sweeper.

- **Why no sweeper task**: a periodic task is a second lifecycle to shut down, it needs the `TimeSource` decision below anyway, and it can only clean rows that nothing is reading. Lazy evaluation is restart-safe, needs no timer, and cannot disagree with the database about what is expired. The cost is that the table can hold dead rows between restarts, which the startup delete bounds.
- **Why wall-clock `expires_at_ms` and not the existing `TimeSource`**: `TimeSource` (`ports.rs:82-88`) is documented as monotonic with only differences meaningful — clock arithmetic for the game. Session expiry has to survive a restart and be comparable with a stored column, so it uses `SystemTime` unix milliseconds like `ended_at_ms` already does in `SqliteGameRecorder` (`recorder.rs:158-165`). Two time sources with two documented jobs is clearer than one that is monotonic and cannot express an absolute expiry.

### D8. Identity resolution produces a resolved key; seats match on it
New value object in `application/`:

```rust
enum Identity {
    Guest   { device_id: String },
    Account { account_id: String, device_id: String },
}
```

with `fn key(&self) -> &str` returning the `account_id` or the `device_id`. `Identity` is resolved once per room-affecting action in `ws::handle_text` from the connection's session, then handed to the actor. `ws::Conn` gains the current `Identity`; `Move`/`Resign`/`Leave`/`Detach` keep reusing it rather than re-resolving per frame (they already do this with `conn.player_id`).

- **Why resolve per action instead of per frame**: a `login` can arrive mid-connection, and the spec requires the account to be adopted for the rest of that connection. Re-resolving at the two entry points (`create_room`, `join_room`) and caching in `Conn` picks that up without a per-frame lookup or a race with the actor.

### D9. A seat is *upgraded*, never swapped — this is what closes the hijack hole
`Seat` keeps its `device_id` and gains an optional `account_id`. Matching (`seat_index_of`) is: the seat's `account_id` equals the connection's account **if both have one**, otherwise the seat's `device_id` equals the connection's `device_id`.

- When a connection with a session occupies a seat that was taken under its own `device_id`, the actor fills in the seat's `account_id` — the seat becomes account-owned, so a later re-attach from a *different* device matches on the account.
- When a connection with a session presents a `device_id` that is **not** the seat's, it can only match a seat already owned by its account. Otherwise the join falls through to the existing `room_full` path.

- **Alternative rejected**: drop the `device_id` from seats entirely and key seats on the account only. Then a guest who later logs in mid-game would need the seat re-keyed, and a guest seat has no account to key on — the upgrade-in-place rule is strictly simpler and keeps the guest path byte-identical.
- **Why not verify a device claim cryptographically**: that is what the session is for. Without one, the server has no evidence about who is presenting a `device_id`, and today's string equality is exactly the bug.

### D10. Room occupancy is tracked per device *and* per account
`App.players: HashMap<device_id, room_code>` stays as it is (the guest rule is untouched). A second map `App.account_rooms: HashMap<account_id, room_code>` is added and consulted **only** when the connection is authenticated. Both `create_room` and `join_room` check the device map always and the account map additionally; both `already_in_room` outcomes use the existing error.

`RoomRegistry::untrack_player_if` becomes `untrack_if(&Identity, &code)`: it releases the device entry, and the account entry when the identity has one, each only if the stored code matches — preserving today's "don't untrack a player who moved on" behavior.

- **Why two maps instead of one keyed by `Identity::key()`**: a single map would change what today's guest games match on, and `players` is read by the existing `already_in_room` path and its tests. Two maps keep the guest rule literally the same code.

### D11. New WS messages; `VERSION` stays `1`
Client: `Register { v, username, password }`, `Login { v, username, password }`, `Logout { v }`, `SetProfile { v, display_name }`. Server: `Session { v, account_id, username, display_name, token, expires_at_ms }`, `SessionOk { v, account_id, username, display_name, expires_at_ms }` (`SessionOk` for logout, so the client learns the session is gone), and `ProfileUpdated { v, account_id, display_name }`. Four new `ErrorCode`s: `invalid_credentials`, `username_taken`, `not_authenticated`, `invalid_display_name`. `KNOWN_TYPES` (`protocol.rs:198-200`) gains the four new names.

- **Why not bump `VERSION` to 2**: the decoder rejects any message whose `v` differs (`protocol.rs:216-218`) and the socket is closed with 4000 — a bump would disconnect every currently deployed client. The change is purely additive and follows the existing `time_control: Option<String>` precedent.
- **Why WS messages instead of `POST /auth/*` plus a bearer header**: it reuses the only channel the clients already speak, needs no body parsing, no CORS, no second router to keep in sync with three duplicated test routers, and leaves the existing `tokio_tungstenite` test harness and `examples/online_buddy.rs` working unmodified. A header on the WS upgrade was the runner-up; it would force both platform clients to change how they open the socket even to do nothing new.
- Auth messages are dispatched in their own match arm **before** the room dispatch in `ws::handle_text`, so an auth error can never fall through into `create_room`/`join_room` handling, and the existing room error paths are untouched.
- The `wire_form_is_stable` test (`protocol.rs:322-457`) is **extended** with the new shapes, not relaxed — it exists precisely to force a conscious decision on the wire format.
- Client mirrors (`ios/.../OnlineProtocol.swift:268-335`, `android/.../OnlineProtocol.kt:256-286`) are intentionally left alone in this change: they decode the message set they know, and a decode failure today closes the socket with 4000, so nothing in their behavior changes.

### D12. Auth failures never gate gameplay
Because `Identity` resolution cannot fail in a way that has no fallback (unknown/expired/revoked token → `Identity::Guest`), `handle_text`'s room path is unchanged: a bad token is logged and treated as a guest. Registration and login rejections answer with an error and leave the connection open. The one hard failure is a below-minimum configuration value, which stops the process at startup rather than degrading.

### D13. `display_name` is a writable account column, not a derived one
The spec carries a display name that defaults to the username when unset, so the schema's nullable `accounts.display_name` must be writable — otherwise it is a column no code path can ever fill and the "differs from the username" half of the contract is unimplementable. So `SetProfile { v, display_name }` is in scope of this change: the server validates the name (trim; 1-32 chars; reject control characters) **before** any write, then updates the column and answers `ProfileUpdated`. The write goes through the same `AuthRecorder` split, so a failed display-name commit is logged and does not break the connection.

The value belongs to the **account**, not the device, which is why it lives on `accounts` (D2) and not on `profiles`: a player who sets a name on a phone must see it on a tablet.

- **Alternative rejected**: deriving the display name only from the username (a column nothing writes). It makes the profile feature indistinguishable from showing the username, and the requirement to *set* a name is the minimum that makes "perfil de jugador" mean anything.
- **Alternative rejected**: an HTTP `PATCH /profile`. Same reason as D11 — it would add a second router and body parsing for one field.

### D14. `App::init` load order and `App::new`
`App::init` order becomes: open pool → run migrations → load `players` rows into `RatingStore` → load `accounts`/`profiles`/`sessions` into the auth store → purge expired sessions → attach `SqliteAuthRecorder`. Load-before-accept is what the existing persistence requirement already demands for ratings, and accounts/sessions join the same guarantee (a session issued before a restart still authenticates). `App::new` keeps its no-database signature and builds the in-memory auth store plus `NoopAuthRecorder`; `Config::for_test` is unchanged, so `database_url: None` still works.

## Risks / Trade-offs

- **[Argon2 CPU under load; no rate limiting]** → Password length is bounded before hashing (D5) and the cost is configurable downward for constrained hosts, but repeated login attempts are not throttled: credential stuffing and user-enumeration timing are possible. Argon2id's cost makes each attempt expensive, which raises the floor but is not a defense. Rate limiting is recorded as a follow-up rather than smuggled in here.
- **[A slow hash blocking the single-connection pool]** → Hashing never runs inside a transaction or while a pool handle is held, and runs in `spawn_blocking` (D5). The `sessions` lookups on the room path are indexed point reads.
- **[Ratings split per device]** → Documented in D1; the follow-up that moves ratings to `account_id` is a separate change with its own migration and spec delta.
- **`max_connections(1)` contention]** → Auth writes share the recorder's pool with game commits, so a session insert can briefly delay a game's commit. Both are short, and neither blocks a player (D7 of the persistence design, extended in D12).
- **`sessions` row growth]** → Expired rows are deleted at startup (D7); between restarts the table only grows by sessions actually issued.
- **A mistake in seat matching hands a player someone else's game** → This is the highest-consequence line of the change, so D9 gets dedicated unit tests in `room_actor.rs` plus an end-to-end test that replays another player's `device_id` against an occupied seat. It is also the one place where the current behavior genuinely changes, which is why it is called out in the `server` delta rather than left implicit.
- **Three new `RoomServices` construction sites' worth of fan-out** → A single `auth` field (D4) and an in-memory default in `App::new` keep it to one field.
- **No client implementation means the feature is not yet reachable by a player** → Intentional (Non-Goals); the WS contract and its tests are the deliverable.

## Migration Plan

1. Add `argon2` to `server/Cargo.toml`; add `0002_player_auth.sql`. Both are additive — no existing table or column is touched, and `sqlx::migrate!` embeds the file at compile time, so the schema and the binary ship together and there is no "migrator ahead of binary" window.
2. `App::init` runs the migration before any load, so a fresh database and an existing `0001` database converge on the same schema with no backfill step.
3. Deploy the server. Guests are unaffected end to end: no existing message, field, error code, rating, or persisted row changes meaning, so a mixed fleet of old and new clients keeps working against the new server.
4. Clients adopt `register`/`login`/`logout` in a later change.
5. **Rollback**: stop the binary and use the previous one. The three new tables are orphaned by the old code (it never queries them) and can be left in place; if they must go, `DROP TABLE sessions; DROP TABLE profiles; DROP TABLE accounts;` is safe because nothing in `0001` references them. No rating or game data is at risk at any point, which is the payoff of D1.

## Open Questions

- Whether ratings move from `device_id` to `account_id`, and if so whether the backfill makes each historical device its own account or merges them. Deferred: it changes the `server` spec and is its own change.
- Whether the eventual Postgres cutover wants a `BLOB`/binary token hash instead of hex text. Deferred: it is a driver-and-encoding change confined to the `Sessions` adapter.

Both are answerable later without changing the specs, the approach, or the task breakdown in this change.

### D15. Guest connection lifecycle (`seated` flag) (`ws.rs:62-64`, `ws.rs:80-82`, `ws.rs:90-92`)

**Motivation**: A guest connection that later authenticates must not trigger an abrupt socket reset (pre-existing `room_full` refusal path, `room_actor.rs:31`) when the room ends. The `seated` flag distinguishes a connection that has never been seated (`!seated`) from one that has (`seated`).

**Behavior**:
- `seated` starts `false`. It becomes `true` when the server sends `RoomReady` or `State` (`ws.rs:70-75`).
- `None` outbound message when `!seated` → connection stays open (`ws.rs:80-82`).
- `None` outbound message when `seated` → `break` (graceful close handshake, `ws.rs:90-92`).

**Effect**: Guest play (`device_id`) unchanged; authenticated sessions (`register`/`login` after joining) gain a stricter lifecycle that requires graceful close.
