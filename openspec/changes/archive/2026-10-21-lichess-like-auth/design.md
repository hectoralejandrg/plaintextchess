# Design

## Context

The account system already exists and works: `register`, `login`, `logout`, and
`set_profile` are client messages on the existing WebSocket protocol
(`server/src/interface/protocol.rs`), the authority is an in-memory store behind
three ports (`server/src/application/ports.rs`: `Accounts`, `Sessions`,
`ProfileStore`), the password is hashed with argon2id in a blocking task
(`server/src/infrastructure/password.rs`), and the durable side is mirrored to
SQLite by an `AuthRecorder` (`server/src/infrastructure/auth_recorder.rs`).

What is missing is everything around it. The identity is a username only
(`server/src/domain/account.rs`: 3-24 characters of `[A-Za-z0-9_-]`, unique
case-insensitively through `username_key`), so there is no way to recover a
forgotten password and no way to reach the player by mail. The session token
lives only in the client's memory — iOS keeps the device id in `UserDefaults`
and Android in `SharedPreferences`, but neither stores the token — so the
player signs in on every launch. The iOS sign-in view shows the raw token with
a copy button (`ios/app/PlainTextChess/ContentView.swift`). Neither client has
a URL scheme registered, so there is nowhere for an emailed link to land. The
server has no notion of a client address (`axum` is served without
`ConnectInfo`), so nothing can be rate limited, and the server is now reachable
from the internet.

Two constraints shape the approach. The deployment has **no domain yet** — the
box runs with Caddy's internal certificate and `DOMAIN=localhost` — so nothing
may depend on a public hostname, a mail provider, or DNS records. And the
server runs on a basic box (2 vCPU, 2 GB), where every argon2 verification
costs real CPU.

## Goals / Non-Goals

**Goals:**

- One identity the player can actually recover: an email address, private and
  never disclosed, with a password-reset flow that never reveals whether an
  address is registered.
- A sign-in screen that reads like a product, and a session that survives an
  app restart.
- An architecture where switching from development to production is a
  configuration change: the provider credential, the sending domain, and the
  public base URL are environment values, not code.
- Authentication that cannot be used to burn the box's CPU, and that cannot be
  used to enumerate accounts.

**Non-Goals:**

- Two-factor authentication, OAuth or social sign-in, and tournament or
  anti-cheat work. All are separate changes.
- Sending real email during development, and every consequence that follows
  (a mail provider account, DNS records, a sending domain, a bounce and
  complaint policy).
- A web landing page for the reset link, and universal links. Both need a
  domain; the design leaves the link's shape configurable so they can be added
  without touching the flows.
- Changing the room, game, clock, or rating behavior. The protocol version
  stays `1`.

## Decisions

### D1 — The email is the account's private identity; the username is a public handle

The registration takes a username, an email address, and a password. The
username is what other players see and what the player signs in with; the email
is never reported in a profile, never written to a log line, and is unique
across accounts.

*Alternative considered:* keeping the username as the only identity and adding
the email as an optional attribute. Rejected: an optional email cannot carry a
password-reset flow, and the flow is the point of the change.

### D2 — The sign-in request carries an identifier, and the old field still works

The wire gains an optional `identifier` on `login`; the server resolves
`identifier` and falls back to the legacy `username` field. `register` keeps
`username` and gains a required `email`; a request without one is answered with
a validation error naming the email field.

*Alternative considered:* bumping the protocol to version 2. Rejected: every
deployed client would have to be replaced at once, for no functional gain.
Keeping version 1 means an old client keeps playing as a guest and keeps
signing in with `username`; only registration from an old client stops working,
which is the intended break.

### D3 — Capability discovery instead of version negotiation

The readiness endpoint reports which authentication features the running server
implements, as a list of stable names: `password_reset`, `email_verification`,
`email_change`, `breached_password_refusal`, `session_resumption`. A client
reads it with an HTTPS GET to `/healthz` at the same address as the game
connection, with a short timeout; if the request fails or times out, the client
assumes the server supports no optional feature. A client that finds the reset
feature missing hides that path and says the server must be updated, instead of
sending a message the server would reject by closing the socket with the
protocol close code.

The names are part of the contract, not a detail of the body: a client branches
on them, so they cannot change once published.

*Alternative considered:* the client sending the new message and treating the
close as "server too old". Rejected: the close also fires for malformed
frames, so it cannot be read as a feature answer, and a failed request would
look like a network fault.

### D4 — Email is a port with pluggable transports

A new `EmailSender` port in `server/src/application/ports.rs` carries a
`EmailMessage` (recipient, subject, text body, HTML body) and returns
`Result<(), EmailError>`, with `EmailError::NotConfigured` for a transport
that is selected but has no credential. Three adapters live in
`server/src/infrastructure/email/`: `log` (the default, and what development
and CI use), `resend` (the production path), and `none` (tests). The
`resend` adapter is written in this change even though no credential exists
yet: it returns `NotConfigured` until it can read one.

*Alternative considered:* sending mail only from the client, or skipping mail
until a provider exists. Rejected: the reset flow is server-driven, and a flow
that cannot be exercised is a flow that will be broken when it finally matters.

### D5 — The provider credential is read from a file

`EMAIL_API_KEY_FILE` points at a mounted secret; `EMAIL_API_KEY` is accepted as
a fallback for CI. A key in the container's environment is visible in
`docker inspect` to anyone in the `docker` group; a mounted file is not.

*Alternative considered:* Docker secrets. Rejected as overkill for one box —
a `chmod 600` file owned by root is the same protection with less machinery.

### D6 — The link the mail carries is built from the public base URL

`PUBLIC_BASE_URL` plus the reset path produces the link, so the same server
build can emit a `plaintextchess://` link today and an `https://` link once a
domain exists. The token inside the link is the same in both cases, so links
already delivered keep working.

*Alternative considered:* hard-coding the scheme. Rejected: it would make the
switch a code change and would strand every link already sent.

### D7 — Rate limiting happens before the password is hashed

The server keeps an in-memory window of recent attempts per client address and
per device. An attempt over the limit is refused with a dedicated error
carrying when to retry, and the refusal happens **before** argon2 runs, which
is the whole point: the cost of a guess must not be paid on a rejected guess.

The client address comes from the `X-Forwarded-For` header the reverse proxy
sets, and the header is only honored from a peer that one of the configured
trusted proxy networks covers (`AUTH_TRUSTED_PROXY_CIDRS`). From any other peer
the server uses the peer's own address. The server is published only through
the proxy, so this is the only way to tell players apart; a peer that is not
the proxy has its header ignored.

*Alternative considered:* rate limiting by the asserted device id alone.
Rejected: the device id is client-asserted, so it is trivially bypassed.

### D8 — The breach check is k-anonymity, fail-open, and switchable

A password is checked against a known-breach corpus by sending only the first
five characters of its SHA-1 (the corpus's k-anonymity API), so the corpus
learns nothing about the password. When the corpus is unreachable the check is
skipped and the attempt continues; `HIBP_ENABLED` turns it off entirely. A
skipped check is recorded in the server log, so an operator can see that the
check was not applied.

*Alternative considered:* blocking on an unreachable corpus. Rejected: an
outage of a third-party service would stop every registration and every
password change.

### D9 — The stricter username rules apply to new accounts only

New usernames are 2-20 characters of lowercase letters, digits, underscore,
and hyphen; they start with a letter, end with a letter or digit, and carry
no repeated or edge separator. The player may type the name in any letter case;
what is stored is lowercase, and the format rules apply to the stored name.
Uniqueness stays case-insensitive through the existing `username_key`, so a new
account cannot take a legacy name that differs only in case.

Accounts that already exist keep the username they have. The alternative —
tightening the rules for everyone — would invalidate live accounts, and there
is no way to rename a user across the deployed client base.

*Alternative considered:* applying the rules to everyone and renaming the
offenders. Rejected for the same reason.

### D10 — A curated reject list lives in the repository

A small list of usernames the server refuses, the way lichess refuses "lame
names". The list is content, and it is kept deliberately small and reviewed.

*Alternative considered:* an external service. Rejected: an extra dependency
and an extra failure mode for a check that is a policy, not a security
boundary.

### D11 — The session token is stored in the platform's secure storage

iOS stores it in the keychain; Android stores it in encrypted shared
preferences (the project already depends on the AndroidX libraries, and
`minSdk` 24 is compatible). A "keep me logged in" choice decides whether the
token is kept at all.

*Alternative considered:* plain preferences. Rejected: the token is a bearer
credential for the account.

### D12 — An unverified email does not block play

Registration creates the account and sends the verification message; the
account reports that its address is unverified, and the client shows a prompt.
Every game feature stays available. The verification state is what a later
feature (rated play, tournaments) can key off, and it is reported in the
session so the client does not have to ask separately.

*Alternative considered:* refusing play until the address is confirmed.
Rejected: it would turn a slow mail delivery into a lockout, and the
deployment has no domain to send from yet.

### D13 — The reset flow reveals nothing

A reset request is answered identically for a registered and an unregistered
address, the token is stored only as a hash, it expires, and it works once.
Completing a reset changes the password and issues no session.

### D14 — The archived `player-auth/profiles` capability is folded in

The two changes that introduced accounts were archived without ever creating
their main specs, so the contract lived only in deltas. This change creates
the `player-auth` capability with the full contract, and the profile
requirements are folded into it rather than creating a near-duplicate
`player-auth/profiles` capability.

### D15 — Two small new Rust dependencies

A rustls-based outbound HTTPS client (for the mail transport and the breach
check) and SHA-1 (for the breach check's k-anonymity prefix). The server has
no HTTP client today, and the runtime image is a slim Debian with no OpenSSL,
so the client must bring its own TLS.

*Alternatives considered:* `reqwest` (larger, slower to compile in the image);
`ureq` (blocking, so it would need `spawn_blocking` anyway and the server
already has that pattern for argon2). The choice is recorded in the task list
as the one place a different client can be dropped in.

### D16 — A stored session is resumed with an explicit message

The client stores its token and, on the next connection, asks the server to
resume the session with it. The server answers with the session and the
profile when the token is still usable, and with the matching authentication
error when it is not.

The answer is a message of its own (`session_resumed`) and not the usual
`session` message: resuming issues no token, and a message that always
carries one would have to carry an empty string to say so.

*Alternative considered:* letting the token ride on `create_room` and
`join_room` as it does today. Rejected: the client then cannot learn who it is
signed in as until it enters a room, so the app could not show the account's
name or its verification state on the home screen, and a stale token would
only be discovered when a room action failed.

### D17 — Every refusal carries a stable code and a machine-readable field

The error message gains an optional `field`, so a client can mark the input
that was refused instead of parsing the message text. The set of codes is
fixed and documented, and the same refusal always carries the same code.

*Alternative considered:* parsing the message text on the client. Rejected: the
message is written for a person, and the wording is exactly what the design
elsewhere forbids depending on.

### D18 — An unknown identifier costs the same as a wrong password

Today the login path returns before hashing when the identifier names no
account, which makes the response time a reliable oracle for whether an
account exists. The path now performs a verification against a fixed dummy
hash when the identifier is unknown, so both cases do the same work.

*Alternative considered:* hashing the password before the lookup. Rejected: it
would put the cost of a password check in front of every request, including
the ones that never need it.

### D19 — The development log transport is refused in production mode

The log transport writes the whole message, including the link, because that
is what makes the flow testable without a provider. `DEPLOYMENT_MODE` marks a
deployment as production, and the server refuses to start with the log
transport in that mode, so the token cannot reach a production log by
misconfiguration.

*Alternative considered:* masking the link in the log. Rejected: the link is
the payload the development flow needs to read.

### D20 — The mail-domain check is advisory

A registration checks that the address's domain can receive mail, and the
check is skipped (and logged) when it cannot be performed. The requirement is
therefore advisory, not absolute, because the deployment has no domain of its
own yet and a DNS failure must not stop a registration.

*Alternative considered:* dropping the check. Rejected: it is what catches the
commonest typo, and it is the check lichess makes.

### D21 — A reset request is answered before the message is delivered

The answer to a reset request is the same for a registered address and for an
unregistered one, and it must also take the same time: the delivery runs in a
background task, so a provider that takes seconds does not turn the request
into an oracle for which addresses exist.

### D22 — A reset token is spent only after the new password passes the policy

The token is looked up (not consumed) while the password is checked, and it is
spent only once the checks pass. A player whose password was refused — because
it is the username, or because it appears in a breach corpus — keeps the link
they were sent.

### D23 — The attempt limits are namespaced per action and bounded

The keys carry the action, so a player who signs in often does not fill the
registration window. Each key keeps only its most recent attempts, and the key
map is trimmed when it grows, so a client that rotates its device id cannot
grow the limiter without limit.

### D24 — The production defaults cannot write a token into a log

The production stack defaults to a provider transport and to production mode,
and production mode refuses the log transport at startup. A deployment that was
never configured therefore cannot write a reset or verification token into its
own log; the log transport is a development choice, made explicitly.

### D25 — The forwarding header is replaced by the proxy, and the server reads the last entry

The reverse proxy overwrites `X-Forwarded-For` with the address it saw, so a
client that sends its own header never reaches the server. The server reads the
**last** entry of the chain — the proxy's own reading — and only when the peer
is a configured trusted proxy, so a forged header cannot rotate the rate-limit
bucket.

### D26 — A confirmation token is bound to the address it was sent to

The token carries the address it was mailed to, and it confirms that address
and no other. A token sent to the old address therefore cannot confirm a new
one, and a resend goes to the address that is actually being confirmed.

### D27 — The requests that send a message are limited, and they do not wait for the provider

A change of address and a resend of the confirmation mail are limited like the
other message-sending requests: without a limit they are a way to mail anyone.
The delivery runs in the background, so the answer does not wait for a provider
that may take seconds.

### D28 — A revocation is persisted before the answer

The logout writes the revocation before it answers. A crash between the answer
and the write would leave the session revoked in memory but alive in the
database, and it would authenticate again after a restart.

## Risks / Trade-offs

- **Argon2 is expensive on a 2 vCPU box.** → The rate limit runs before the
  hash, so a rejected attempt costs a map lookup. The default limits are
  conservative and configurable.
- **The client address is only as trustworthy as the proxy.** → The forwarding
  header is honored only from the proxy's own address, and the deployment
  publishes the server only through the proxy. A server published directly
  would rate limit by the direct peer.
- **The log transport writes the reset token into the log.** → That is the
  point in development, and the transport is refused at startup when the
  deployment is in production mode, so the token cannot reach a production log
  by misconfiguration.
- **Email addresses are personal data in the database.** → They are never
  logged or reported, they live in the same encrypted-at-rest volume as the
  rest of the state, and the backup procedure already covers them.
- **Tightening the username rules leaves two rule sets in force.** → The
  legacy set only applies to accounts that predate the change, and uniqueness
  is enforced by one key across both.
- **Registration from an old client stops working.** → Accepted: the old
  client can still play as a guest, which is the path the requirement protects.
- **The breach check depends on a third party.** → Fail-open and switchable.
- **The Android client gains a dependency.** → `androidx.security:security-crypto`
  is the platform's own library, and `minSdk` 24 already satisfies it.

## Migration Plan

1. The schema change is additive: a new column on the accounts table, and new
   tables for the verifications and the reset tokens. It runs at startup with
   the other migrations and preserves every existing account, rating, game, and
   device link.
2. Accounts that predate the change have no email address. They keep signing in
   with their username, and the client offers them the email-addition path.
3. Deploy order: the server first (it accepts the old clients), then the
   clients. The server advertises its features, so an old server simply hides
   the new paths in a new client.
4. Rollback is redeploying the previous image: the previous binary ignores the
   new tables, and the new columns are nullable, so no data has to be
   unwound.

## Open Questions

- Which mail provider and sending domain will be used in production. The
  transport and the credential's location are already decided, so this is a
  configuration answer, not a design one.
- Whether the reset link should point at a web landing page once a domain
  exists. The link is built from the base URL, so the answer is a base-URL
  change.
- The exact contents of the username reject list.
