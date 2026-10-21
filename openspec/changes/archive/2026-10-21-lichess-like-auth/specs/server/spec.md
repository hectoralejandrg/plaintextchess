# Spec Delta: server

## MODIFIED Requirements

### Requirement: Server Persistence
The server MUST persist the per-device Glicko-2 rating state and a record of
every finished online game in a local SQLite database (local file, WAL mode).
The database location MUST come from the `DATABASE_URL` environment variable
(`sqlite:...`) with a built-in default (`sqlite:./data/chess-server.db`); the
default's parent `data/` directory MUST be created at startup. The database
schema MUST be managed by versioned migrations that run at startup. Before
accepting any connection, the server MUST load the persisted rating states into
the in-memory rating store.

#### Scenario: A finished online game survives a server restart
- **WHEN** the server stops and restarts against the same `DATABASE_URL` file
- **THEN** the persisted `players` table reloads both devices' full Glicko-2 state into the in-memory rating store, and the new game starts from those values (not from the 1500 default)

### Requirement: Server Health and Configuration
The server MUST expose an HTTP health endpoint (`GET /healthz`) that answers
200 while the server is ready to accept WebSocket connections, and MUST serve
the online protocol on a WebSocket endpoint. The bind address, port, reconnect
grace window (`RECONNECT_GRACE_SECS`), and persistence database location
(`DATABASE_URL`, default `sqlite:./data/chess-server.db`) MUST come from
environment configuration with safe built-in defaults, so the same build can run
locally and on a hosting platform. The server MUST start without a database and
terminate cleanly on SIGTERM.

#### Scenario: The health endpoint reports readiness
- **WHEN** a client requests `GET /healthz`
- **THEN** the server answers 200 with a machine-readable body while it is ready, and the WebSocket endpoint accepts new connections

#### Scenario: Configuration comes from the environment
- **WHEN** the server starts with BIND, PORT, or RECONNECT_GRACE_SECS set
- **THEN** it binds to that address and port and uses that grace window, and without them it uses the built-in defaults

## ADDED Requirements

### Requirement: Server Authentication Persistence
The same database MUST also hold the account data the authentication flows
need: the account's email address, its email verification state, the email
verification tokens, and the password-reset tokens. A migration that adds
these tables MUST preserve every existing account, rating, game, and device
link.

#### Scenario: A database written before the email existed keeps its accounts
- **WHEN** the server starts against a database written before the account email existed
- **THEN** the migration completes, every existing account is still there and can sign in with its username, and no account is left without a usable identity

### Requirement: The Account Email Is Unique
The account email MUST be unique across accounts regardless of letter case, so
the stored address is normalized or the index compares case-insensitively.

#### Scenario: The account email is unique
- **WHEN** two accounts hold the same email address in different letter case
- **THEN** the database refuses the second one, and the first account keeps its address

### Requirement: The Email Tokens Are Stored as Hashes
A verification token and a reset token MUST each be stored only as a one-way
hash, MUST expire, and MUST be usable once. A verification token MUST confirm
the address it was sent to and no other.

#### Scenario: A token is stored only as a hash
- **WHEN** the server issues a verification or a reset token
- **THEN** the stored record holds a hash of it, and no stored field equals the token the player received

#### Scenario: A token confirms only the address it was sent to
- **WHEN** a player confirms with a token that was sent to a different address
- **THEN** the server refuses the confirmation, and no address is replaced

### Requirement: The Mail Settings Come From The Environment
The email transport (`EMAIL_TRANSPORT`), the sender address and name
(`EMAIL_FROM`, `EMAIL_FROM_NAME`), the public base URL that emailed links are
built from (`PUBLIC_BASE_URL`), and whether the server runs in production mode
(`DEPLOYMENT_MODE`) MUST come from environment configuration with safe
built-in defaults.

#### Scenario: The mail settings come from the environment
- **WHEN** the server starts with the mail variables set
- **THEN** it uses them, and without them it uses its built-in defaults

### Requirement: The Authentication Limits Come From The Environment
Whether the breached-password check runs (`HIBP_ENABLED`), the sign-in and
registration rate limits (`AUTH_RATE_LIMIT_LOGIN_PER_MIN`,
`AUTH_RATE_LIMIT_REGISTER_PER_HOUR`), and the proxies whose forwarding header
may be trusted (`AUTH_TRUSTED_PROXY_CIDRS`) MUST come from environment
configuration with safe built-in defaults.

#### Scenario: The authentication limits come from the environment
- **WHEN** the server starts with the limit variables set
- **THEN** it uses them, and without them it uses its built-in defaults

### Requirement: The Provider Credential Is Read From A File
The provider's credential MUST be readable from a file (`EMAIL_API_KEY_FILE`),
so it never has to sit in the container's environment; an inline value
(`EMAIL_API_KEY`) is accepted as a fallback. A transport that is selected but
not configured MUST leave the server running with the flows it can still
serve.

#### Scenario: The credential is read from a file
- **WHEN** the server starts with the credential's location pointing at a readable file
- **THEN** the server uses that credential, and never needs it in its own environment

#### Scenario: A transport that is not configured leaves the server running
- **WHEN** the server starts with a transport selected that has no credential
- **THEN** the server starts, reports that the transport is not configured, and the flows that do not need it still work

### Requirement: The Health Endpoint Reports The Authentication Features
The health endpoint MUST report which authentication features the running server
supports, as a list of feature names, so a client can offer only the flows the
server actually implements. The names MUST be stable: `password_reset`,
`email_verification`, `email_change`, `breached_password_refusal`, and
`session_resumption`. A feature that is turned off MUST NOT appear in the list.

#### Scenario: The health endpoint reports the authentication features
- **WHEN** a client requests `GET /healthz`
- **THEN** the body names the authentication features the running server supports, so a client can tell an older server from a current one

#### Scenario: The feature names are stable and a disabled feature is absent
- **WHEN** a client reads the feature list
- **THEN** every name it holds is one of the fixed names, and a feature that is turned off does not appear in the list
