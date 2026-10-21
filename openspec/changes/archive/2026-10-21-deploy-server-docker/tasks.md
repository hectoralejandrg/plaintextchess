# Tasks

## 1. Production Compose

- [x] 1.1 Write `docker-compose.prod.yml`: `ws-server` service (`TIER=all`, `DATABASE_URL=sqlite:/data/app.db`, named volume `server-data:/data`, `restart: unless-stopped`, `healthcheck` on `/healthz` with a generous `start_period` **and a multi-failure `retries`/`interval` tolerance** so a starved `/healthz` under a setup burst does not mark the container unhealthy — see design D3, initial `mem_limit` ~1 GB and `ulimit nofile: 65536` per design D7) plus the proxy service publishing 80/443 and its certificate volume; **no Redis service** (spec "Single-Server Production Topology"). Verify: `docker compose -f docker-compose.prod.yml config` renders without errors and lists exactly two services plus the volume; the `healthcheck` shows a `retries` > 1.

## 2. TLS Proxy (Caddy)

- [x] 2.1 Write `config/Caddyfile`: site address from `$DOMAIN`, reverse proxy to `ws-server:3000`, WebSocket passthrough (upgrade handled natively; no read timeout cutting idle sockets), upstream health check (`health_uri /healthz`, short `health_interval` **plus a failure threshold so a transiently starved `/healthz` does not flap the upstream** — design D3), ACME via Caddy's automatic HTTPS, and HTTP→HTTPS redirect (spec "TLS-Only Public Surface", design D3/D2). Verify: `caddy validate --config config/Caddyfile` passes and the health check block declares a fault-tolerance setting beyond the interval.
- [x] 2.2 Encode the two no-domain paths: `$DOMAIN=localhost` (Caddy locally-trusted internal TLS) and an HTTP-only `:8080` test path; add a comment that ACME must never be pointed at a placeholder public domain (design D2 "No-domain mode"). Verify: both paths are stated in the file and in `.env.example` in group 4.
- [x] 2.3 Keep ACME/TLS state on a named compose volume so certificates survive container replacement (spec "Persistent State Survives Restarts", scenario "Container replacement keeps players' data" applies symmetrically). Verify: volume declared in group 1's compose and mounted by the proxy service.

## 3. Image Hardening

- [x] 3.1 Runtime stage of `Dockerfile` for prod: drop the dead `REDIS_URL` default (misleading under `TIER=all`), keep non-root user, minimal packages, and cache-friendly layer order; keep `EXPOSE`/`CMD` aligned with `docker-compose.prod.yml` (design D1, spec "Single-Server Production Topology"). Verify: `docker build` succeeds; `docker inspect` env contains no `REDIS_URL`; `docker run --rm <img> id` shows `appuser`.

## 4. Environment and Tooling

- [x] 4.1 Add `.env.example` documenting every prod variable (`TIER`, `BIND`, `PORT`, `DATABASE_URL`, `RECONNECT_GRACE_SECS`, `SESSION_TTL_SECS`, `RUST_LOG`, `DOMAIN`) including the no-domain note (spec "Runtime Configuration Is Environment-Driven"). Verify: every variable referenced by `docker compose -f docker-compose.prod.yml config` exists in `.env.example`, and no credential or deploy-time value is present.
- [x] 4.2 Add `Makefile` targets `deploy-up`, `deploy-down`, `deploy-logs`, `deploy-ps`, each wrapping `docker-compose.prod.yml` in the style of the existing `multi-node-*` targets. Verify: `grep -c prod Makefile` shows the four targets and each round-trips (`make deploy-up` up / `make deploy-down` down) on a test box or equivalent dry run.

## 5. Runbook

- [x] 5.1 Write `docs/deploy-docker.md`: build, up, TLS issuance with a real domain, `/healthz` smoke, logs, rollback (previous image tag), and volume backup/restore that is **checkpoint-aware** (run `PRAGMA wal_checkpoint(TRUNCATE)` in a one-off container or use a snapshot-complete copy; note that copying bare `app.db` can miss WAL contents) (spec "Operational Lifecycle" incl. "Database backups run without data loss"). Verify: every step runs against the test box as written; the backup command round-trips using the documented restore.
- [x] 5.2 Document time-to-ready expectation in the runbook (boot is bounded by DB open + migrations + state load because there is no Redis on the path — design D3): the operator can read "what is normal" during a restart (spec "Startup Is Bounded and Gated"). Verify: the runbook states the bound and the `/healthz`-gated behavior.

## 6. Client Release Endpoint

- [x] 6.1 iOS: set `OnlineConfig.defaultURL` in `ios/app/PlainTextChess/OnlineProtocol.swift:13` (currently `""`) to the deployed URL constant `wss://<DOMAIN>/ws`, gated so that without a domain it stays empty (online unavailable, degraded scenario preserved); keep the DEBUG launch-arg override (spec "Release Clients Reach the Deployment", design D6). Verify: `swift build`/project compiles; a release-style run with a placeholder uses the degraded path; the override still wins in DEBUG.
- [x] 6.2 Android: add the release default URL next to the debug-only `online_url` path in `android/app/src/main/java/com/hectoralejandrg/plaintextchess/MainActivity.kt` (the module does not enable `BuildConfig`; keep the debuggable gate for the extra), so release builds resolve the default while the debug intent extra still wins (spec "Release Clients Reach the Deployment", design D6). Verify: `./gradlew assembleDebug` succeeds and an instrumented/manual check traces both the release default and the debug extra.

## 7. Integration Verification

- [x] 7.1 On the test box: `docker compose -f docker-compose.prod.yml up --build`, then assert over plain HTTP that `/healthz` reports ready and that a full create → join → move → finish flow works through the proxy (spec "Startup Is Bounded and Gated", scenario "A proxy request arrives during boot" — a retried request after ready succeeds; no partially initialized session serves traffic, since the listener binds only after `App::init`).
- [x] 7.2 Restart durability: `docker compose -f docker-compose.prod.yml restart` then `down`/`up` — persisted ratings/accounts/sessions survive via `server-data` volume (spec "Persistent State Survives Restarts", scenario "Container replacement keeps players' data").
- [x] 7.3 Plaintext policy: request HTTP on :80 (redirect, no session) and confirm no `ws://` upgrade reaches the server; record the result in the runbook (spec "TLS-Only Public Surface", scenario "Plaintext cannot carry a session"). Manual gates bound to the future domain (`wss://` end to end) remain open work in `## Workflow follow-up`.
- [x] 7.4 Creation-burst smoke on the target box (design D7): run a moderate concurrent `CreateRoom` burst (well under the 10k/8s profile that produced 24.7% failures on 2 vCPU) and record seat success rate, ack latency, and whether `/healthz` stays reachable; confirm the Caddy upstream is not flapped during the burst (spec "Startup Is Bounded and Gated", scenario "Health checking does not degrade under load"). The recorded ceiling goes into `docs/deploy-docker.md` as the documented box capacity.

## 8. Box Sizing

- [x] 8.1 Make the resource envelope environment-driven (`SERVER_MEM_LIMIT`, `CADDY_MEM_LIMIT`, `NOFILE` in `.env`) and document a sizing table per box size in `docs/deploy-docker.md`, so the same compose file fits a 1 GB and an 8 GB box without editing the file (design D7). The shipped defaults keep the 4 GB+ row unchanged. Verify: `docker compose -f docker-compose.prod.yml config` renders the defaults and the overridden values; `make deploy-check` prints the host cpus/RAM and the effective limits.

## Workflow follow-up

- Archive the change after the project's review requirements are satisfied.
- Once a real domain points at the box: rerun TLS issuance, flip client defaults to the real domain, and verify the manual `wss://` gates in 2.1 and 7.3 end to end.
