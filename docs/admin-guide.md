# Admin Guide: Build Operations

Operations documentation for managing the cross-platform build system: CI, quality metrics, configuration ownership, and maintenance.

## Configuration Ownership

| File | Owner | Change procedure |
| --- | --- | --- |
| `config/shared-build.yaml` | Build owner | Requires review; every target/variable here applies to both platforms |
| `config/ios-build.yaml` | iOS owner | Must keep `build_targets` a subset of shared targets |
| `config/android-build.yaml` | Android owner | Must keep `build_targets` a subset of shared targets |

Configuration drift is detected automatically by `./tests/validate-cross-platform.sh` (and in CI by the `validate` job). Never edit a script to compensate for a config problem — fix the config.

## CI/CD

The pipeline lives in `.github/workflows/build-validation.yml`:

| Job | Runner | Purpose |
| --- | --- | --- |
| `validate` | ubuntu-latest | Runs all three validation test scripts |
| `build-ios` | macos-latest | Runs `./scripts/build-ios.sh`, uploads XCFramework + metrics |
| `build-android` | ubuntu-latest | Runs `./scripts/build-android.sh`, uploads jniLibs + metrics |
| `quality` | ubuntu-latest | Downloads both metric files, prints a quality report, fails on `status: failed` |

Triggered on push to `main`, on PRs, and manually. Concurrency cancels superseded runs per ref.

### CI Expectations
- The `validate` job must pass before any build job runs (it is a prerequisite of the workflow graph by job ordering; gate merges on it).
- Build jobs report elapsed time; compare against the performance targets in `config/shared-build.yaml` (5 min iOS, 8 min Android).

## Online Server (`server/`, chess-server)

Runtime environment variables for the online multiplayer server. All are read at startup; an unreachable database fails startup with a non-zero exit.

| Variable | Default | Purpose |
| --- | --- | --- |
| `BIND` | `0.0.0.0` | Interface the server listens on |
| `PORT` | `8765` | TCP port for `GET /healthz` and `/ws` |
| `RECONNECT_GRACE_SECS` | `120` | Grace window before a disconnected player's seat is forfeited |
| `DATABASE_URL` | `sqlite:./data/chess-server.db` | Persistence database location (SQLite). The parent directory (default `data/`) is created automatically if missing |

The database holds the per-device Glicko-2 rating state and the finished-game history; ratings are loaded from it at startup and committed at each game's terminal result. The server is single-node by design (one connection pool, WAL mode, 5 s busy timeout): on a hosting platform, `data/` (or the directory your `DATABASE_URL` points at) must be a **persistent volume** so ratings and game history survive container restarts.

### Authentication settings

Player accounts are optional: a client that sends no `register`/`login` message plays as a guest under its `device_id`, exactly as before. An operator tightens or loosens the authentication behavior with these four variables:

| Variable | Default | Minimum | Purpose |
| --- | --- | --- | --- |
| `SESSION_TTL_SECS` | `2592000` (30 days) | `60` | How long an issued session token keeps authenticating |
| `ARGON2_MEMORY_KIB` | `19456` (19 MiB) | `8192` | Argon2id memory cost per password hash |
| `ARGON2_TIME_COST` | `2` | `1` | Argon2id passes over memory |
| `ARGON2_PARALLELISM` | `1` | `1` | Argon2id lanes |

Unlike the variables above, these four are parsed strictly: a non-numeric value, or one below its minimum, **fails startup** with an explicit configuration error and a non-zero exit instead of quietly serving with weaker security. `ARGON2_MEMORY_KIB` is the knob to raise on a beefier host; each login costs one hash at these settings, so the time cost is what a bulk of concurrent logins spends.

### Authentication tables

The same SQLite file gains three tables (migration `0002_player_auth.sql`), all created by the same startup migration mechanism as the ratings schema:

| Table | Key | Holds |
| --- | --- | --- |
| `accounts` | `account_id` (UUIDv4) | `username`, the case-folded `username_key` (unique index, so `Ana` and `ana` are one account), the argon2id `password_hash`, and an optional `display_name` |
| `profiles` | `device_id` (primary key) | The device-to-account link. The primary key is what makes "a device belongs to at most one account" a database guarantee; a device that signs in as somebody else has its row repointed |
| `sessions` | `session_id` (UUIDv4) | `account_id`, the `device_id` that logged in, the SHA-256 `token_hash` (unique index), `expires_at_ms`, and a nullable `revoked_at_ms` |

Only `token_hash` is stored: the plaintext token is returned to the client once and exists nowhere in the database. Accounts, links, and live sessions are loaded into memory at startup, and expired sessions are purged then, so a token issued before a restart still authenticates afterwards.

Nothing here migrates ratings or game history: those stay keyed by `device_id`, so `players`, `games`, and `rating_history` are untouched by the account tables and a guest's history is unaffected by whether the player ever signs in.

A failed account, link, or session write is logged (`failed to persist an authentication change`) and applied in memory only; the connection stays usable and the player keeps playing as a guest.

### Authentication wire protocol

Client-facing messages are JSON with `"v":1` and a `"type"` discriminator (the names serialize from the message enums in `server/src/interface/protocol.rs`, all `snake_case`). Guests never see these beyond `not_authenticated`: a connection that sends no `register`/`login` plays under its `device_id` with no session.

| Client message | Fields | Success reply | Error codes |
| --- | --- | --- | --- |
| `register` | `username`, `password`, optional `device_id` | `session` | `username_taken`, `invalid_request` |
| `login` | `username`, `password`, optional `device_id` | `session` | `invalid_credentials`, `invalid_request` |
| `logout` | `token` | `session_ok` | `not_authenticated`, `session_expired` |
| `set_profile` | `display_name`, `token` | `profile_updated` | `not_authenticated`, `session_expired`, `invalid_request`, `invalid_display_name` |

Replies: `session` carries the newly issued `token`, `account_id`, `username`, `display_name`, and `expires_at_ms` — the token is returned exactly once (only its hash is stored). `session_ok` confirms a revocation and never carries a token. `profile_updated` echoes the new `display_name`.

Auth-specific `error` codes (wire name → meaning; messages in `protocol.rs::ErrorCode::message`):

| Code | Meaning |
| --- | --- |
| `invalid_credentials` | Username/password mismatch — a single generic message for both, so callers cannot learn which usernames are registered |
| `username_taken` | Username already registered, compared case-insensitively (`username_key` is the lower-cased unique index) |
| `not_authenticated` | The connection has no usable session |
| `session_expired` | The presented session's lifetime elapsed; distinct from `not_authenticated` so the client knows it signed in but aged out |
| `invalid_request` | A field failed its format check; the message names the offending field |
| `invalid_display_name` | Display name outside 1–32 trimmed characters or containing a control character |

### Client navigation (Login → Home → Game)

Both clients open on a Login screen and use explicit navigation into Home and the game table; authentication is no longer an option inside the "New game" sheet.

| Screen | Purpose | Entry points |
| --- | --- | --- |
| Login | Identity gate: `username`/`password` fields, Register and Login actions, "Play as guest", and a generic auth error area | App start; Home → "Login / Register" (guest) or Logout |
| Home | Session state (signed-in user vs guest), mode selection (two players, CPU with difficulty, online create/join with room code and time control), profile update (`display_name`) and Logout when signed in | After Login/Register success or guest entry |
| Game | The existing table: board, status row, clocks, move list, Undo/Resign/Flip, New game and game-end dialog — behavior unchanged | Home → start a game in the chosen mode |

Navigation and guards:

- **Android** (`MainActivity.kt`, `AppNavHost`): a `NavHost` with `startDestination = "login"` and destinations `login`/`home`/`game`. Login success and "Play as guest" navigate to `home` with `popUpTo("login") { inclusive = true }`; Logout and "Login / Register" navigate back to `login` with `popUpTo("home") { inclusive = true }`. A single activity-scoped `GameViewModel` carries session and game state.
- **iOS** (`ContentView.swift`): a `NavigationStack(path:)` whose root is `LoginView` and whose typed `Route` enum holds `home` and `game`. `onAuth`/`onGuest` append `.home`, `onStartGame` appends `.game`, `onLogout` calls `doLogout` then clears the path, and `onGoToLogin` clears the path. `onChange(of: vm.authToken)` appends `.home` when a token arrives.

The wire protocol is unchanged: both clients reuse WS v1 `register`/`login`/`logout`/`set_profile`, and guest play still runs under the device's `device_id` with no token.

## Build Quality Metrics

Every build writes a metrics file (schema in `docs/support/build-metrics.md`):

- `target/build-metrics/ios-last.json`
- `target/build-metrics/android-last.json`
- `target/build-metrics/coordination-last.json` (written by `scripts/build-coordination.sh`)

The CI `quality` job publishes these as the `build-quality-report` artifact. Review the report after each pipeline run; sustained regressions against the targets should be tracked as issues (see maintenance schedule).

## Release Operations

| Step | iOS | Android |
| --- | --- | --- |
| 1. Coordinated release build | `BUILD_ENVIRONMENT=production BUILD_TYPE=release ./scripts/build-coordination.sh` | same command |
| 2. Validate | `./tests/validate-cross-platform.sh` | same |
| 3. Package | `xcodebuild archive` + export IPA from the Xcode project | `gradle :app:bundleRelease` |
| 4. Sign | App Store / enterprise profile | Play signing key / internal keystore |
| 5. Distribute | App Store Connect | Play Console (AAB) / internal track (APK) |

## Error Handling Summary

| Situation | Behavior | Recovery |
| --- | --- | --- |
| Missing spec/config file | Script fails fast before compiling | Restore the file from VCS; run the validator |
| Missing toolchain | `[ERROR] cargo not found…` | `docs/environment-setup.md`; re-run |
| iOS build fails (sequential) | Coordinated run stops | Fix cause, re-run `./scripts/build-ios.sh` |
| Both fail (parallel) | All failures reported, non-zero exit | Fix each; re-run with `--parallel` |
| Config drift | Validator / CI `validate` job fails | `docs/support/troubleshooting.md` § Config drift |

Full failure catalogue: [Troubleshooting](./support/troubleshooting.md).
