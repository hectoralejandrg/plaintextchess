# Proposal

## Why

The server can only be run by a developer with a local toolchain: deployment of
the server was explicitly deferred at `add-online-multiplayer` ("follow-up ops
change", `proposal.md:14`) and never created, so `openspec/specs/` has no
deployment capability, there is no production compose, no TLS surface, and iOS/
Android release builds cannot reach any server at all (iOS
`OnlineConfig.defaultURL` is `""`, Android has no release URL — debug intent
extra only). A basic-server Docker deployment also has not been evaluated for
latency, while the repo already carries Docker artifacts (`Dockerfile`,
`docker-compose.yml`, `config/nginx-lb.conf`) that were shaped for local/CI
multi-node testing, not for hosting.

## What Changes

- Add a production Docker deployment for a **single basic server**: one
  authoritative `ws-server` (`TIER=all`, monolith — no Redis on the boot path),
  one Caddy reverse proxy terminating TLS (`wss://`) with automatic ACME, and a
  named volume for the SQLite database.
- **Latency at start** (requester constraint): no Redis dependency is added;
  the server binds only after migrations + ratings/auth load complete; Caddy
  health-checks `/healthz` so boots serve no warm-up traffic; the proxy sets
  no read timeout on the WebSocket path and relies on TLS 1.3 + session
  resumption for handshake cost.
- Add `docker-compose.prod.yml`, `config/Caddyfile`, `.env.example`, deploy
  runbook (`docs/deploy-docker.md`), and `Makefile` deploy targets.
- Harden the runtime image (prod stage): non-root, minimal packages, no dead
  `REDIS_URL` default under `TIER=all`.
- **Release client endpoint becomes reachable**: add a `wss://<DOMAIN>/ws`
  default URL to iOS (`OnlineConfig.defaultURL`, currently `""`; today online
  play is unavailable in release) and to Android (currently debug-only, release
  gets `null`; the module does not enable `BuildConfig`).
- Not changing: `docker-compose.yml` (local/CI multi-node split-tier stack
  stays as is), the split-tier server behavior itself, the wire protocol, or
  any game/rating logic.

## Capabilities

### New Capabilities

- `deployment`: The production hosting capability — how the server ships
  (single-box Docker), how clients reach it (TLS-only public surface), where
  state lives (persistent volume), how boots behave (readiness gating), and
  how config/secrets/lifecycle are handled.

### Modified Capabilities

None. The `android` and `ios` specs do not currently carry a *server URL
constant* requirement — the URL constant behavior exists in code by change
arrows (`add-online-multiplayer` D9) without a capability requirement, so the
release default URL lands as an implementation detail of this change and of
`deployment`'s client-endpoint requirement. If review prefers a delta, move it
into `android`/`ios` deltas.

## Impact

- **New ops files**: `docker-compose.prod.yml`, `config/Caddyfile`,
  `.env.example`, `docs/deploy-docker.md`; `Makefile` gains `deploy-*`
  targets; `Dockerfile` gains a prod runtime stage.
- **Client code**: `ios/app/PlainTextChess/OnlineProtocol.swift`
  (`OnlineConfig.defaultURL`, empty string today → real default) and
  `android/app/src/main/java/com/hectoralejandrg/plaintextchess/MainActivity.kt`
  (release URL added beside the debug-only intent extra).
- **System ergonomics/compatibility**: `wss://` requires no iOS ATS exception
  and no Android cleartext flag (an improvement over the current `ws://` debug
  path); the server adds no runtime dependency and Redis becomes *not required*
  for the shipped deployment (it was already optional for `TIER=all`).
- **Risk**: ACME issuance needs a publicly resolvable domain and reachable
  ports 80/443; until a domain exists, TLS-only requirements are verified as a
  manual gate with `DOMAIN=localhost` (Caddy internal TLS) or an HTTP-only test
  profile — never a placeholder public domain against ACME.
