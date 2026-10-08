# Tasks

Implementation order follows the dependency chain: the data layer lands before the domain and ports, the domain before the protocol, and the identity/actor work before the HTTP-facing behavior. Each group carries its own tests and docs; the final group is integration only.

## 1. Dependencies, config, and schema

- [x] 1.1 Add `argon2` to `server/Cargo.toml` and verify `cargo build` in `server/` succeeds and the crate's default features include argon2id
- [x] 1.2 Add `session_ttl_secs: Duration` and an `Argon2Params` (memory_kib, time_cost, parallelism) to `Config` in `server/src/infrastructure/config.rs`, read from `SESSION_TTL_SECS` / `ARGON2_MEMORY_KIB` / `ARGON2_TIME_COST` / `ARGON2_PARALLELISM` with built-in defaults, and verify `cargo test config` in `server/` passes for both set and unset cases
- [x] 1.3 Reject a below-minimum session lifetime or argon2 parameter in `Config::from_env` by returning an error that stops startup, and verify with a test that `SESSION_TTL_SECS=1` and `ARGON2_MEMORY_KIB=1024` both fail validation with an explicit message while the defaults pass
- [x] 1.4 Create `server/migrations/0002_player_auth.sql` with `accounts`, `profiles`, and `sessions` exactly as in design D2 (including the unique index on `username_key` and on `token_hash`), and verify `openspec` aside that the file compiles into the migrator by running the existing `cargo test` persistence test against a fresh database and confirming the three tables exist

## 2. Domain records and password/session primitives

- [x] 2.1 Add `Account`, `Profile`, `Session`, and `Identity` (`Guest`/`Account`, with `key()`) records under `server/src/domain/`, and verify with unit tests that `Identity::key()` returns the account id when authenticated and the device id when not
- [x] 2.2 Implement username validation (3-24 chars, letters/digits/underscore/hyphen, case-folded `username_key`) and password validation (8-128 chars) as pure functions, and verify with unit tests covering boundary lengths, rejected characters, whitespace-only input, and that the fold makes `Ana` and `ana` produce the same key
- [x] 2.3 Implement display-name validation (trim, 1-32 chars, no control characters) as a pure function, and verify with unit tests for empty-after-trim, exactly 32, 33 chars, and an embedded newline
- [x] 2.4 Implement argon2id hashing and verification behind a `PasswordHasher` that runs inside `tokio::task::spawn_blocking`, and verify with tests that a hash verifies against the right password, fails against a wrong one, is not equal to the plaintext, and that a rejected-length password never reaches the hasher
- [x] 2.5 Implement token generation (32 `rand` bytes, base64url no padding) and token hashing (SHA-256), and verify with tests that tokens are unique across a batch, the hash differs from the token, and hashing is deterministic for the same input

## 3. Ports and in-memory adapters

- [x] 3.1 Add `Accounts`, `Sessions`, `ProfileStore`, and the async `AuthRecorder` (with `RecorderError`) ports to `server/src/application/ports.rs`, and verify `cargo build` in `server/` still succeeds
- [x] 3.2 Implement the in-memory `PlayerAuth` (satisfying `Accounts + Sessions + ProfileStore`) covering account-by-username/account-by-id lookups, account insert, device lookup and linking, session issue/lookup/revoke, and expired-session purge, and verify with unit tests for each operation including the case-insensitive username lookup
- [x] 3.3 Enforce the uniqueness and single-account-per-device rules inside `PlayerAuth` (a duplicate `username_key` and an insert for an existing `account_id` are both refused), and verify with unit tests
- [x] 3.4 Implement session expiry as a check against `expires_at_ms` at lookup time rather than a stored flag, and verify with tests that an expired session is reported as absent while a revoked session is reported as revoked
- [x] 3.5 Add `NoopAuthRecorder`, and verify with a test that a `PlayerAuth` paired with it behaves identically across insert, link, issue, and revoke (no persistence, same outcomes)

## 4. SQLite adapter

- [x] 4.1 Make the pool construction reusable so auth and the game recorder share one `SqlitePool` (extract the connection options from `SqliteGameRecorder::connect` without changing its behavior, keeping `max_connections(1)`, WAL, and the busy timeout), and verify the existing persistence tests still pass unchanged
- [x] 4.2 Implement `SqliteAuthRecorder` account insert and display-name update, and verify with a test against a temporary database that the account row carries the argon2 hash (never the plaintext) and that a repeated insert of the same `username_key` fails
- [x] 4.3 Implement device-link and session issue/revoke persistence, and verify with a test that a stored `sessions` row contains only the token hash and that querying by hash returns the right session
- [x] 4.4 Implement startup loading of `accounts`, `profiles`, and non-expired `sessions` plus the expired-row purge, and verify with a test that writes rows, reopens against the same file, and observes the loaded state and the purge

## 5. Wire protocol

- [x] 5.1 Add `ClientMessage::{Register, Login, Logout, SetProfile}` and `ServerMessage::{Session, SessionOk, ProfileUpdated}` with the field shapes from design D11, keeping `VERSION` at 1, and verify `cargo build` in `server/` succeeds
- [x] 5.2 Add the four `ErrorCode`s (`invalid_credentials`, `username_taken`, `not_authenticated`, `invalid_display_name`) with their wire names and generic credential-failure message, and verify the unknown username and wrong password cases produce an identical code and message
- [x] 5.3 Extend `KNOWN_TYPES` with the four new client message types, and verify a decode test accepts each new type and still rejects an unknown one
- [x] 5.4 Extend the `wire_form_is_stable` test in `server/src/interface/protocol.rs` with the new message shapes and the required optional fields, and verify the test passes without relaxing any existing assertion

## 6. Identity and room occupancy

- [x] 6.1 Add the `auth: Arc<dyn PlayerAuth>` field to `RoomServices` and a `resolve_identity` helper that maps a presented token to `Identity` (falling back to `Identity::Guest` for missing, unknown, expired, or revoked tokens), and verify `cargo build` in `server/` succeeds
- [x] 6.2 Supply the in-memory auth store plus `NoopAuthRecorder` from `App::new` so the no-database path keeps working, and verify the existing in-memory room and game tests pass with no change to their bodies
- [x] 6.3 Add the `App.account_rooms` map and check it in `create_room` and `join_room` only for authenticated identities, keeping the device-keyed `App.players` check as the guest path, and verify with a test that one account signed in on two devices is refused a second room with `already_in_room` while two unrelated guests are unaffected
- [x] 6.4 Replace `RoomRegistry::untrack_player_if` with an identity-aware `untrack_if` that releases the device entry and, when present, the account entry, each only when the stored code matches, and verify with tests for both the matching and the moved-on cases
- [x] 6.5 Give `Seat` an optional `account_id` and change `seat_index_of` to match on the seat's account when both sides have one and on `device_id` otherwise, upgrading a seat's account when an authenticated connection occupies a seat taken under its own device id, and verify with unit tests covering same-device re-attach, cross-device re-attach by account, and the non-match case

## 7. Guest fallback and seat-hijack guard

- [x] 7.1 Implement the hijack guard in `handle_connect`: a connection with no session presenting another player's device id is refused with `room_full` and leaves the seated player and game untouched, and verify with an actor test that replays a seated player's device id and asserts the seat, clock, and status are unchanged
- [x] 7.2 Verify guest play is unaffected end to end by running the existing room, integration, and disconnect/reconnect suites unchanged, and confirm the same scenarios also pass with an authenticated identity
- [x] 7.3 Verify that a login issued mid-connection is adopted by the next room-affecting action (the account room limit applies and a seat occupied by that connection is upgraded), and confirm with a test through `ws::handle_text` that `Move`/`Resign`/`Leave` still reuse the connection's pinned identity

## 8. Auth message handling over the WebSocket

- [x] 8.1 Handle `Register` in `ws::handle_text` (validate username and password, reject a taken username, hash the password, create the account, link the device, issue a session, answer `Session`) and verify with an integration test over a real socket that a registration returns a usable token and that the stored password column is not the plaintext
- [x] 8.2 Handle `Login` (verify the password, issue a session, re-link the device when it belonged to another account, answer `Session`) and verify with an integration test that a wrong password and an unknown username both answer `invalid_credentials` with an identical message and issue no session
- [x] 8.3 Handle `Logout` (revoke only the presented session, answer `SessionOk`) and verify with an integration test that the revoked token stops authenticating while a second session for the same account still works
- [x] 8.4 Handle `SetProfile` (require an authenticated session, validate, persist, answer `ProfileUpdated`) and verify with an integration test that a guest is refused with `not_authenticated` and an invalid name is refused while the stored name is kept
- [x] 8.5 Verify that an auth rejection leaves the connection open and the player still able to create or join a room as a guest, and that auth messages are answerable before any room exists

## 9. Startup wiring, integration, and docs

- [x] 9.1 Wire the load order in `App::init` (pool → migrations → ratings → accounts/profiles/sessions → purge expired → attach `SqliteAuthRecorder`) and verify with a test that a session issued before a restart still authenticates after it
- [x] 9.2 Extend `server/tests/persistence.rs` to cover account creation, device relinking, session issue across a restart, and the guarantee that a failed auth write is logged and leaves the player able to play as a guest
- [x] 9.3 Add cross-device coverage: the same account re-attaches to a live game from a second device with its server-measured remaining time, while a third party presenting that account's device id is refused
- [x] 9.4 Document `SESSION_TTL_SECS` and the `ARGON2_*` variables and the three new tables in `docs/admin-guide.md`, and verify the documented names match `Config` exactly
- [x] 9.5 Run the full gate: `cargo test` in `server/`, `cargo clippy` with zero warnings, and `cargo build --release`; verify `openspec validate add-player-login-sessions --strict` passes and that no existing spec file was edited outside the two deltas in this change

## Deuda técnica / Mejora futura (verificación en emuladores)

- `mobile-mcp` (`mobile-next/mobile-mcp`) está disponible en el catálogo de herramientas y reconoce el simulador `iPhone 17 Pro` (`D610A679...`) como `online`. El `mobilecli` (`npm install -g @mobilenext/mobilecli@latest`) funciona para `io tap`, `apps launch` y `screenshot`. **Bloqueo pendiente**: `mobile_list_elements_on_screen` requiere el componente `agent` (`Agent is not installed on the device`), que no está disponible ni como subcomando `mobilecli agent install` ni como parte del cliente `mobilecli` actual. Para futuras pruebas de UI en emuladores, se necesita instalar/configurar el `agent` del `mobile-mcp` (posiblemente a través del plugin `.claude-plugin` o del archivo `mcp.json` del repositorio) antes de que `mobilecli` pueda hacer la inspección del árbol de accesibilidad por texto (`New game`).
