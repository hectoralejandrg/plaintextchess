# deployment Specification

## Purpose

Define how the game server ships to a single basic server: a Docker production
topology (authoritative monolith plus TLS-terminating proxy), a TLS-only public
surface for real players, durable persistence across container replacement, and
boot/latency behavior so starting or restarting the deployment never costs
players connection latency.

## Requirements

### Requirement: Single-Server Production Topology
The production deployment MUST run one authoritative server process
(`TIER=all`) and one reverse proxy on the same box, and Redis MUST NOT be a
required dependency of the production topology.

#### Scenario: The production stack boots without Redis
- **WHEN** the production compose file is started on the box
- **THEN** no Redis container or other message-fabric service is required for the server to serve rooms, moves, and ratings

#### Scenario: The local multi-node stack is untouched
- **WHEN** a developer starts the existing local/CI compose stack
- **THEN** it behaves as before (Redis plus tiered nodes), because production is a separate compose file

### Requirement: TLS-Only Public Surface
The public entrypoint MUST serve the game protocol over `wss://` terminated by
the reverse proxy, and a plaintext WebSocket connection MUST NOT be served: an
HTTP request without TLS is either redirected to HTTPS or refused, and no
`ws://` upgrade reaches the game server.

#### Scenario: A client connects over TLS
- **WHEN** a client opens `wss://<domain>/ws`
- **THEN** the proxy terminates TLS and forwards the connection to the game server

#### Scenario: Plaintext cannot carry a session
- **WHEN** a requester sends an HTTP or WebSocket request that is not secured by TLS at the proxy
- **THEN** it is redirected to HTTPS or refused, but never proxied to the game server as an established `ws://` session

#### Scenario: No domain yet (pre-launch)
- **WHEN** the box has no public domain and ACME certificates cannot be issued
- **THEN** the deployment still starts using a locally-trusted internal TLS mode (or an explicitly HTTP-only test profile) and never requests certificates for a placeholder domain

### Requirement: Persistent State Survives Restarts
The server's SQLite database MUST live on a persistent, named Docker volume
whose path is configurable, so ratings, game records, accounts, and sessions
survive the container being recreated or upgraded, and no state is written into
the image or an anonymous layer.

#### Scenario: Container replacement keeps players' data
- **WHEN** the server container is destroyed and recreated on the box
- **THEN** the new process opens the same database volume and the previous ratings, accounts, and sessions are still served

#### Scenario: First boot initializes state
- **WHEN** the deployment starts on a box with an empty volume
- **THEN** the server creates and migrates the database itself and reports ready

### Requirement: Startup Is Bounded and Gated
The server MUST complete initialization (database migration, ratings and
authentication load) before accepting connections, and the reverse proxy MUST
not route live traffic to a process that is still initializing: traffic during
a boot is refused by the server or marked unhealthy by the proxy, never served
by a partially initialized process. With no Redis dependency, time before the
ready signal is bounded by database work plus state load only.

#### Scenario: A proxy request arrives during boot
- **WHEN** the proxy forwards traffic while the server is still initializing
- **THEN** the client receives an explicit failure (connection refused or 5xx) rather than a slow, partially working session, and a retry after the ready signal succeeds

#### Scenario: Restart costs no data loss or seat migration
- **WHEN** the server restarts cleanly
- **THEN** in-memory rooms are rebuilt empty by design while persisted ratings and accounts resume as before

#### Scenario: Health checking does not degrade under load
- **WHEN** the server is serving a concurrent creation burst and its health endpoint is transiently slow to answer
- **THEN** the proxy keeps routing to it (the health check tolerates a transient miss) rather than marking the deployment unhealthy, while a genuinely failed process is still retired

### Requirement: Runtime Configuration Is Environment-Driven
The deployment MUST expose every tunable it relies on as documented
environment variables with committed examples, and the shipped image MUST NOT
bake in secrets or environment-specific values.

#### Scenario: An operator reconfigures the port
- **WHEN** the operator changes the documented environment file and redeploys
- **THEN** the server (and the proxy's upstream) follow the new value without any image rebuild

#### Scenario: No secrets in the repository
- **WHEN** the deployment is deployed from a fresh checkout
- **THEN** no credential or deploy-time value is read from files tracked by version control

### Requirement: Operational Lifecycle
The deployment MUST define its container restart behavior, a stop grace period
long enough for in-flight requests and a clean shutdown, and a documented
rollback path to the previously running image, plus backup and restore
instructions for the persistent volume that account for SQLite's WAL mode.

#### Scenario: The box reboots
- **WHEN** the host restarts
- **THEN** the server and proxy come back up automatically and accept players again

#### Scenario: A bad release is rolled back
- **WHEN** the operator redeploys the previous image
- **THEN** the deployment returns to the prior behavior with the persistent volume intact

#### Scenario: Database backups run without data loss
- **WHEN** the operator takes a backup while sessions were active
- **THEN** the documented procedure produces a complete database image (checkpointed or snapshot-complete), not a file missing WAL contents

### Requirement: Release Clients Reach the Deployment
Release builds of iOS and Android MUST carry a default server URL pointing at
the deployed TLS endpoint, so online play works out of the box without a debug
override, while debug overrides continue to work for development.

#### Scenario: A release build connects out of the box
- **WHEN** a player installs a release build at the documented domain
- **THEN** online play (create/join rooms, log in, tournaments) works without any per-device configuration

#### Scenario: No configured endpoint degrades gracefully
- **WHEN** no server endpoint is configured in a build
- **THEN** online play is unavailable but the rest of the app works as today
