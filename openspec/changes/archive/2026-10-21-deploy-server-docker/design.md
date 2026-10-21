# Design

## Context

The repo already ships Docker artifacts, but all of them are shaped for
local/CI work: `Dockerfile` (release build of `ws-server`, `debian:bookworm-slim`
runtime, non-root user), `docker-compose.yml` (`redis` + `logic` + 2 transport
nodes + nginx — the Phase 2 split-tier lab), and `config/nginx-lb.conf`
(plain-HTTP load balancer). There is no production compose, no TLS, no
`.env.example`, and no deploy docs. Server config is env-driven
(`server/src/infrastructure/config.rs`) and the boot path is serial:
`App::init` (`server/src/infrastructure/server.rs`) creates the data dir, opens
SQLite (WAL, single connection, `busy_timeout` 5 s), runs embedded migrations,
loads ratings into memory, purges expired sessions, loads accounts — and only
then does `ws-server.rs` bind the listener (with `TCP_NODELAY` set per
connection). The client side has a DEBUG-only server URL: iOS
`OnlineConfig.defaultURL` is `""`, Android reads only the `online_url` intent
extra in debuggable builds. See proposal.md for the deferred-deployment history.

Constraint from the requester: one **basic** server, and latency must not
regress at startup or during the first interactions.

## Goals / Non-Goals

**Goals:**
- A production compose file that runs the monolith plus a TLS proxy on the
  same box, with the database on a named volume.
- The shortest possible per-move and per-boot network path given one box.
- Config the operator can change from one documented env file.
- Release clients able to reach the deployment with no launch-arg tricks.

**Non-Goals:**
- Multi-node production (split tiers, Redis fabric, sharding) — the existing
  local/CI stack covers development of that path.
- HA/failover, external databases (Postgres cutover), CDN, or monitoring
  systems beyond logs and `/healthz`.
- Changing the wire protocol, game logic, ratings, or any spec-existing
  server behavior.
- You-Do-It-Yourself CI deploys/servers provisioning tooling (see
  `openspec/plan.md:222` "DevOps" hints) — this change only ships artifacts.

## Decisions

### D1 — Monolith (`TIER=all`) is the production topology

**Choice**: deploy `ws-server` with default `TIER=all`; the compose file
contains no Redis service.

**Why**: on a single basic box, the split tier has no upside but a guaranteed
cost. `TIER=all` runs rooms, core validation, persistence, auth, and the WS
endpoint in one process, so every move is an in-memory hop. Any Redis-based
topology adds a pub/sub round trip per move, a Redis container competing for
CPU/RAM, and a `depends_on: redis healthy` wait on the boot path.

**Measured basis** (`docs/support/qa-performance-findings.md`, round 6
baseline, 2000 games, saturating): `all` **57,484 moves/s** (p95 40 ms),
`combined` 58,700 (p95 44 ms), `split` 47,615 (p95 63 ms). The fabric crossing
costs ~19% throughput, and QA's own caveat is that replica variance reaches
±33–50%, so `all` and `combined` are statistically tied. Avoiding Redis does
not lose throughput on one box; it removes the one path that measurably loses
it. (Round 6 finding R6-A adds that fabric sharding produced no gain anyway,
0.85x at 3 shards, with the control plane pinned to one instance.)
**Alternative considered**: `TIER=combined` — same in-process shortcut for
colocated rooms but still requires Redis for non-colocated matches; keep it
for a future multi-box move, not for the first deployment.

### D2 — Caddy terminates TLS with automatic ACME

**Choice**: a `caddy` service in the prod compose (ports 80/443), proxying to
`ws-server:3000`; site address driven by a `DOMAIN` env var.

**Why**: asks the least of the operator (no certbot dance, no manual renewal)
and handles WebSocket upgrades natively. Caddy also gives HTTP→HTTPS redirect
and reasonable TLS defaults (session resumption cache limits first-connection
handshake cost, which is the "latency at start" part of the requester
constraint).

**No-domain mode** is explicit, not implicit: with `DOMAIN=localhost`, Caddy
issues a locally-trusted internal certificate so the local test still
exercises TLS; an HTTP-only `:8080` profile covers proxy-only testing. ACME is
**never** pointed at a placeholder public domain — a `.env` with a real but
unowned `DOMAIN` would fail issuance at boot and risk rate limits.
**Alternatives considered**: nginx + certbot (matches the existing
`nginx-lb.conf` style, but manual renewal and config work); terminating TLS
inside the Rust process (would add dependencies and header/secret plumbing for
no benefit at this scale).

### D3 — Readiness is enforced by bind-order plus a proxy health check

**Choice**: rely on the existing boot order (listen *after* `App::init`
finishes), configure a compose `healthcheck` on `/healthz` with a generous
`start_period`, and give Caddy an upstream health check (`health_uri` +
`health_interval`). Caddy routes only to a checked-healthy upstream, so boots
surface as brief 502s, not slow or half-initialized games.

**Tolerance is not optional**: the monolith profile
(`docs/support/qa-performance-findings.md`, 2026-10-13) measured `/healthz`
**unreachable** during a setup burst (10,000 concurrent `CreateRoom`s on
2 vCPU / 2 GB, event-loop starvation in accept/setup). A health check with a
short interval and no failure tolerance would mark the server down under
exactly the load the requester cares about, turning a transient stall into a
proxied outage. The Caddy `health_uri` MUST therefore carry a multi-failure
threshold (and the compose `healthcheck` a matching `retries`), sized so a
starved `/healthz` does not flap the upstream, while a genuinely dead process
is still retired quickly.

**Why**: the expensive part of a cold start is not the bind but
migrations + persisted-state load, and the current code already waits for it
before bind — duplicating that gate in application code would change server
behavior for no observed gain. The health checks make the behavior visible to
the operator (`docker compose ps`) and reproducible in a smoke test.
**Alternative considered**: a `READY` file both processes poll — rejected as
stateful and easy to desync from the actual listener.

### D4 — One env file, compose interpolation

**Choice**: `docker-compose.prod.yml` expands every variable from `.env`
(`$TIER`, `$DATABASE_URL`, `$DOMAIN`, …) with defaults in `env_file`/`;`
`.env.example` committed. No secret is tracked.

**Why**: compose interpolation keeps `Caddyfile` and image env in one place.
**Alternatives considered**: docker secrets (armored but overbuilt for a box
with one deployer), baked-in values in the image — rejected (violates the
"no image-specific config" requirement).

### D5 — Persistent state as one named volume

**Choice**: `DATABASE_URL=sqlite:/data/app.db` with `server-data:/data` in the
compose file. The server already creates parent dirs and the WAL files live
next to the DB.

**Why**: a named volume survives `docker compose down` (without `-v`) and
container replacement; keeping all mutable state in one directory makes backup
one operation. Note (`docs/deploy-docker.md`): WAL mode means a filesystem
copy of only `app.db` can be short of the last transactions; the runbook must
checkpoint (e.g. `PRAGMA wal_checkpoint(TRUNCATE)` via a one-off container) or
use a snapshot-complete copy — no timeline-based assumptions.
**Alternative considered**: bind mounts (operator-profile dependent, fine for
dev but a footgun for uid/gid).

### D6 — Client defaults are additive, TLS-forward

**Choice**: set iOS `OnlineConfig.defaultURL` to the deployed URL (placeholder
until documented) and give Android a release-side default URL next to the
existing debug intent extra path (`MainActivity.kt`). Until a real domain
exists, defaults stay inert so nothing misconnects; then they point at
`wss://<DOMAIN>/ws`.

**Why**: `wss://` needs no iOS ATS exception and no Android cleartext flag, so
the TLS-first path removes config burden rather than adding it. Release
behavior with no configured URL continues to be "online play unavailable" per
the existing code paths.

### D7 — Resource envelope for a basic box

**Choice**: size in the compose (`server` memory limit ~1 GB proposed, `nofile`
65536, `restart: unless-stopped`), and document why: the repo's own load test
measured per-connection RSS < 500 KB and FD < 2.5 (archive
`2026-10-09-split-realtime-architecture/tasks.md` task 5.2), so thousands of
**live** connections fit on 2 vCPU / 2 GB with SQLite's single connection
(`recorder.rs` `max_connections(1)`) and one logic lane (`config.rs`
`DEFAULT_LOGIC_LANES=1`).

**Scope this claim to steady state, not bursts**: those numbers describe open
connections, not concurrent *seating*. The monolith profile
(`docs/support/qa-performance-findings.md`, 2026-10-13) measured 10,000
concurrent `CreateRoom`s on 2 vCPU / 2 GB at **24.7% setup failures** (7,531 ok
/ 2,469 failed), with the move phase degraded to ~940 moves/s (ack p95 493 ms)
and `/healthz` unreachable while the burst ran. The cause is event-loop /
scheduler starvation in the accept+setup path — the same family as QA finding
R6-D — and it is a property of the monolith on that box, not of Docker or the
proxy. So the deployment can hold many live games comfortably, but it must not
be documented or marketed as seating a 10k-per-second room-creation burst on a
basic box.

**Why**: numbers, not adjectives, prevent re-design — actual values get set in
the tasks and can be tuned after a load smoke on the real box (a moderate
creation-burst smoke is an explicit task so the ceiling is known, not
discovered in production).

## Risks / Trade-offs

- [ACME issuance fails (no domain, DNS wrong, ports blocked)] → documented
  `DOMAIN=localhost` fallback and a boot error that names what Caddy could not
  solve; the runbook covers it before it bites.
- [SQLite WAL on a single volume] → single source of truth and easy backup;
  the documented backup procedure must be checkpoint-aware (see D5).
- [No Redis today walls out a future multi-node move] → config is unchanged;
  `TIER=combined`/split remain supported for a later upgrade (opposite direction
  of `2026-10-09-split-realtime-architecture`, which kept `TIER=all` for exactly
  this).
- [Caddy health check flaps the upstream under a load burst] → the monolith
  profile measured `/healthz` unreachable during a setup burst, so `health_uri`
  carries a multi-failure threshold (see D3), not a short interval alone; a
  starved health endpoint then cannot take the deployment down, while a dead
  process is still retired.
- [Basic box hit by a room-creation burst is mistaken for "broken"] → the
  measured setup ceiling (24.7% failures at 10k concurrent creates on 2 vCPU)
  is documented in D7 and exercised by a moderate burst smoke task, so the
  limit is known and sized, not discovered in production.
- [`UI`-side release URL pointing at a box that later changes] → the env var is
  a build constant in clients; release rollouts must redeploy clients when the
  domain changes — documented in the runbook, not solved in this change.
- [Caddy vs nginx config drift] → `config/Caddyfile` is the only production
  proxy config; `config/nginx-lb.conf` stays with the local split stack and a
  comment in each file says they do not meet in production.

## Migration Plan

1. Land ops files (compose, Caddyfile, `.env.example`, runbook, Makefile
   targets, image hardening). Nothing here touches game behavior, so no
   server/client migration is needed.
2. Land client defaults gated on the actual domain; until the domain exists,
   defaults stay empty (release-inert).

Rollback strategy: the prod compose is a separate deployment surface —
`deploy-down` on the prod file stops it; the DB volume survives, and any
pre-deployment box state is unaffected. Each published image is tagged, so a
bad release is rolled back by redeploying the previous tag.

## Open Questions

- Exact memory limit and `nofile` for the box (D7 gives the reasoning; the
  task sets initial values, tunable on the real box).
- Whether release client defaults point at `wss://<DOMAIN>/ws` in the same
  change or wait until the domain exists (D6: defaults stay empty until then).
