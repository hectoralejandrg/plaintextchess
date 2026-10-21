# Deploying the server with Docker

Production deployment for a single basic server: one authoritative game server
(`TIER=all` monolith) plus one TLS-terminating reverse proxy (Caddy). There is
no Redis on this path.

- Compose file: `docker-compose.prod.yml` (project name `plaintextchess-prod`)
- Proxy config: `config/Caddyfile`
- Environment template: `.env.example`

For the local multi-node (split-tier) development stack, see
`docker-compose.yml` and `config/nginx-lb.conf` — that is a different stack and
is not used in production.

## 1. Prerequisites

- Docker Engine with the Compose plugin on the server.
- Ports 80 and 443 reachable from the internet (80 is used for the ACME
  challenge and the HTTP→HTTPS redirect; 8080 is an optional test path).
- A public DNS record pointing at the server, **only if** you want automatic
  certificates. Without a domain, use the `localhost`/8080 paths below.

## 2. Configure

```sh
cp .env.example .env
```

Edit `.env`:

- `DOMAIN` — a real public domain for automatic TLS, or `localhost` before you
  have one. Do **not** use a real-but-unowned placeholder domain
  (`example.com`): ACME issuance fails at boot and can hit rate limits.
- `DATABASE_URL` — leave as `sqlite:/data/app.db` so persistence lands on the
  `server-data` volume.
- `RECONNECT_GRACE_SECS`, `SESSION_TTL_SECS`, `RUST_LOG` — tune as needed.

`.env` is git-ignored. Keep any secret there, never in the image.

### Sizing for the box

Check the box first:

```sh
make deploy-check     # prints host cpus/RAM and the effective limits
```

The server itself adapts to the machine at runtime: the async runtime starts
one worker thread per CPU core, and every accepted socket gets `TCP_NODELAY`
(so small move frames are not delayed by Nagle). Nothing in the code needs to
change per box.

What does **not** adapt is the memory and file-descriptor envelope, so it is
environment-driven (`SERVER_MEM_LIMIT`, `CADDY_MEM_LIMIT`, `NOFILE` in `.env`).
Pick the row that matches the box:

| Box RAM | vCPU | `SERVER_MEM_LIMIT` | `CADDY_MEM_LIMIT` | `NOFILE` | Concurrent players (order of magnitude) |
|---------|------|--------------------|-------------------|----------|----------------------------------------|
| 1 GB | 1–2 | `384m` | `256m` | `4096` | ~2,000 |
| 2 GB | 2 | `768m` | `512m` | `8192` | ~4,000 |
| 4 GB | 4 | `1g` | `1g` | `16384` | ~8,000 |
| 8 GB+ | 8+ | `1g` | `1g` | `65536` | ~8,000 (proxy RAM-capped) |

The defaults shipped in `.env.example` are the 4 GB+ row, so a bigger box needs
no change. On a 1–2 GB box, copy the row into `.env` before the first deploy.

Keep the **sum of the two memory limits comfortably below the box RAM** — they
are ceilings, not reservations, and the OS plus the Docker daemon need the
rest. A memory spike then costs the proxy (reconnect) instead of the whole box
(OOM kill).

Where the numbers come from: the proxy holds ~120 KB per concurrent connection
(measured: ~1.7 GiB at 14k connections) and the game server ~19 KB per open
room (measured: 261 MiB at 14k rooms), so the proxy hits the RAM wall first and
sets the concurrent-player ceiling. `NOFILE` is a second, independent ceiling:
the server spends one file descriptor per client connection.

### What to expect on a small box

- **Live games and moves are fine.** They are the cheap path.
- **Room creation in bursts is the sharp limit.** A burst of thousands of
  concurrent `CreateRoom`s starves the accept/setup path of the event loop:
  the reference profile (2 vCPU / 2 GB) measured 24.7% setup failures at
  10,000 rooms in ~8 s, with `/healthz` unreachable during the burst.
- **A burst does not take the service down.** The compose healthcheck tolerates
  several consecutive failures and the proxy needs a failure threshold before
  it drops the upstream, so a starved `/healthz` produces a few slow or failed
  requests rather than a flapping proxy.
- **`LOGIC_LANES` does not help here.** It only applies to the split tiers
  (`TIER=logic` / `TIER=combined`); the monolith has no lanes, and raising it
  is not a safe default (see `openspec/changes/tune-logic-lane-defaults`).

## 3. Build and start

```sh
make deploy-up          # docker compose -f docker-compose.prod.yml up -d --build
make deploy-ps          # service + health status
make deploy-logs        # follow logs
```

The server binds its listener only after database migration and the load of
ratings/accounts/sessions finishes. Caddy health-checks `/healthz`, so a
request during a boot is not sent to a half-initialized process.

### Time-to-ready (what is normal)

There is no Redis on the boot path, so the wait before `/healthz` answers is
bounded by: opening SQLite + running the embedded migrations + loading persisted
ratings and authentication into memory. On a small/fresh database this is a
couple of seconds; it grows with the number of persisted players, accounts, and
sessions. During a restart you will briefly see the proxy return `502` until the
health check passes — that is the gate working, not a failure.

Observe it:

```sh
docker compose -f docker-compose.prod.yml logs ws-server | grep listening
# Through the proxy (TLS). With DOMAIN=localhost the certificate is internal,
# so use -k. Replace localhost with your domain once DNS points here.
curl -fk https://localhost/healthz   # expect {"protocol_version":1,"status":"ok"}
```

The `http://:8080` test path is **not published by default**, so a request to
`http://127.0.0.1:8080/healthz` fails on a stock deploy. Publish the port (see
section 4) before using it.

## 4. TLS

- **With a real domain** (`DOMAIN=play.tudominio.com`): Caddy obtains and renews
  the certificate automatically. Verify:

  ```sh
  curl -fsS https://play.tudominio.com/healthz
  ```

  To set an ACME contact email, uncomment the `email` line in
  `config/Caddyfile` and rebuild the `caddy` service.

- **Without a domain** (`DOMAIN=localhost`): Caddy issues a locally-trusted
  internal certificate. Browsers/`curl` need the internal CA
  (`caddy` stores it in the `caddy-data` volume), or hit the HTTP-only test
  path instead.

- **HTTP-only test path**: the `http://:8080` site in `config/Caddyfile` serves
  the same proxy without TLS. It is **not published by default** (that would
  expose a plaintext surface); to use it before a domain exists, uncomment the
  `8080:8080` port line in `docker-compose.prod.yml` and re-run `make deploy-up`.

### Plaintext policy (verify after deploy)

Automatic HTTPS redirects plain HTTP on port 80 to HTTPS, so a plaintext
`ws://` upgrade is never served:

```sh
curl -sI http://play.tudominio.com/healthz        # expect 301/308 to https
```

## 5. Roll back a bad release

Images are built locally by Compose. Tag the running image before each deploy,
then restore it to roll back:

```sh
# Before a deploy: keep the current image as the rollback point.
docker image tag plaintextchess-prod-ws-server:latest plaintextchess-ws-server:previous

# After a bad deploy: restore the previous image and restart without rebuilding.
docker image tag plaintextchess-ws-server:previous plaintextchess-prod-ws-server:latest
docker compose -f docker-compose.prod.yml up -d --no-build
```

The `server-data` volume is untouched by a rollback.

## 6. Backup and restore

The database is a SQLite file in WAL mode. Copying only `app.db` while the
server runs can miss the last transactions still in the WAL. Back up with the
writer stopped so the copy is snapshot-complete:

```sh
# Backup (brief downtime)
docker compose -f docker-compose.prod.yml stop ws-server
docker run --rm \
  -v plaintextchess-prod_server-data:/data:ro \
  -v "$PWD/backups:/backup" \
  alpine tar czf /backup/server-data-$(date +%F-%H%M).tgz -C /data .
docker compose -f docker-compose.prod.yml start ws-server
```

If a `sqlite3` client is available, `sqlite3 /data/app.db ".backup /backup/app.db"`
(checkpoint-aware) is an online alternative — but the shipped image does not
include `sqlite3`, so the stop-copy-start above is the default.

Restore:

```sh
docker compose -f docker-compose.prod.yml stop ws-server
docker run --rm \
  -v plaintextchess-prod_server-data:/data \
  -v "$PWD/backups:/backup" \
  alpine sh -c 'rm -rf /data/* && tar xzf /backup/server-data-<stamp>.tgz -C /data'
docker compose -f docker-compose.prod.yml start ws-server
```

## 7. Mail (password reset and email verification)

The account flows need a way to deliver two messages: the confirmation of an
email address and the password-reset link. The transport is selected by
configuration, so the deployment works before a provider or a domain exists.

| `EMAIL_TRANSPORT` | What it does | When |
|-------------------|--------------|------|
| `log` | Writes the whole message to the server log | Development and CI. The log holds the link, so the flow is complete and testable. |
| `resend` (the production default) | Sends the message through the provider's API | Production, once a credential and a sending domain exist. |

`EMAIL_TRANSPORT=log` is **refused at startup** when `DEPLOYMENT_MODE=production`:
the message carries a one-time token, and a token in a production log is a
credential in a log.

### Development (no provider, no domain)

```sh
# .env — the development pair
EMAIL_TRANSPORT=log
DEPLOYMENT_MODE=development
PUBLIC_BASE_URL=plaintextchess://app
```

The production defaults are `EMAIL_TRANSPORT=resend` and
`DEPLOYMENT_MODE=production`: a deployment that was never configured cannot
start with the log transport, so a token cannot reach a production log by
accident.

The link the player would receive is in the log:

```sh
docker compose -f docker-compose.prod.yml logs ws-server | grep -A3 'email body'
```

The app opens the same link through its own scheme (`plaintextchess://app/...`),
so a reset can be completed on a device without any server setup.

### Production (a real provider)

```sh
cp secrets/resend_key.example secrets/resend_key
# put the provider's credential in it
chmod 600 secrets/resend_key
```

```sh
# .env
EMAIL_TRANSPORT=resend
DEPLOYMENT_MODE=production
EMAIL_FROM=no-reply@play.tudominio.com
PUBLIC_BASE_URL=https://play.tudominio.com
```

The credential is read from the file `EMAIL_API_KEY_FILE` points at, which the
compose mounts at `/run/secrets/resend_key`. It never sits in the container's
environment, so `docker inspect` does not show it. The inline `EMAIL_API_KEY`
is accepted as a fallback, for CI.

The sending domain needs its DNS records (SPF, DKIM, and the provider's
return-path record) before the messages are delivered. Until then, the provider
accepts the request and the mail lands in spam.

### The trusted proxy

The rate limit keys on the client's address, which the reverse proxy reports in
`X-Forwarded-For`. That header is a claim, so it is used only when the peer is
one of the configured proxies:

```sh
# .env: the docker network the proxy lives in
AUTH_TRUSTED_PROXY_CIDRS=172.16.0.0/12
```

With no trusted proxy configured, the server keys every attempt on the peer's
own address, which behind the proxy is the proxy's.

The attempt limits are configured too, and they apply per address *and* per
device, so a shared network does not lock everybody out:

```sh
# .env
AUTH_RATE_LIMIT_LOGIN_PER_MIN=10
AUTH_RATE_LIMIT_REGISTER_PER_HOUR=20
```

`0` means no limit. The limits are in memory: a restart clears them.

## 8. Box capacity (record on first deploy)

The deployment holds many **live** games comfortably. Concurrent **room
creation** is the sharper limit: the monolith profile measured 24.7% setup
failures at 10,000 concurrent `CreateRoom`s per ~8 s on 2 vCPU / 2 GB. Record
this box's own ceiling with a moderate burst (well under that profile) and note
whether `/healthz` stays reachable and Caddy does not flap the upstream.

| Date | Box (vCPU / RAM) | Burst (rooms / window) | Seated OK | `/healthz` stayed up |
|------|------------------|------------------------|-----------|----------------------|
| 2026-10-15 | 12 / 32 GB (macOS + Docker Desktop, single client source) | 14,000 / ~13 s | 14,000/14,000 (100%) | yes (200 on every probe; 1×2 s timeout at the peak) |

Recorded curve: 100% seated at 150/300/600/1200/2400/5000/10000/12000/14000
rooms, roughly linear (~0.7–0.85 ms per room). Server RSS peaked at 261 MiB of
its 1 GiB limit; no restarts, never unhealthy.

Caveat on the ceiling: above 14,000 the measurement stopped being about the
server. A single client source tops out at ~16,384 concurrent sockets (ephemeral
port range), which is what the 20,000/40,000 runs hit. Measure the real ceiling
on the production Linux box with multiple client sources. Caddy was capped at
1 GiB by `docker-compose.prod.yml` after this run observed ~1.7 GiB unconstrained
at 14k connections.
