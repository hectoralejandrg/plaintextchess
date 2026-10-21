# Proposal

## Why

The authentication flow is a developer-grade stub: one screen with two buttons, a
username with no email behind it, a session that is never stored (so the player
signs in on every launch), the raw session token displayed in the UI, and no
rate limiting on the argon2-backed login and registration paths — while the
server is now publicly reachable over TLS.

Players expect the account experience of lichess: sign in once and stay signed
in, an email-backed identity with password recovery, and a login screen that
reads like a product rather than a protocol demo. The server-side contract for
all of this was never given a main spec (the two changes that introduced it were
archived without one), so the behavior is currently pinned only by code
comments.

## What Changes

- **New capability `player-auth`** — the account identity and credential
  contract. Registration and login accept a username *or* an email as the
  identifier; a username policy (length, charset, lowercase, uniqueness, and a
  reject list) applies to new accounts while existing accounts keep theirs; the
  email is the account's private identity and is never published or logged; a
  password can be recovered by email with a single-use, expiring, hashed token;
  a new account is verified by email before it counts as verified; a password
  that appears in a known breach corpus is refused; login and registration are
  rate limited per client address and per device; and email delivery is a
  pluggable transport so the server works before a provider or a domain exists.
- **`auth-ui` (modified)** — the client-side sign-in experience becomes a
  lichess-style flow: one identifier field that accepts a username or an email,
  a primary sign-in action with a separate create-account path, guest play as
  a first-class alternative, password reset by email, email-verification
  prompts, inline error reporting, and a session that survives an app restart
  (stored in the platform's secure storage). The raw session token is never
  displayed, and a client that meets an older server degrades to the flows that
  server actually supports instead of failing.
- **`server` (modified)** — the server's configuration and persistence surface
  grows the authentication settings (email transport and sender, the provider
  key's location, the public base URL used in emailed links, the breach-check
  switch, and the rate limits), the readiness endpoint advertises which
  authentication features the running server supports, and the database gains
  the account email, the email verifications, and the password-reset tokens.
- **Non-goals** — two-factor authentication, OAuth or social sign-in, the
  credentials of a real email provider (development ships the log transport),
  universal links and the web landing page for a reset link (both need a domain
  that does not exist yet), and any change to the room, game, clock, or rating
  behavior.

## Capabilities

### New Capabilities

- `player-auth`: the account identity and credential contract — registration
  and login, username and email policy, session issuance and lifetime, device
  linkage and profile, password reset, email verification, breach checking,
  email delivery, and delivery-rate limiting.

### Modified Capabilities

- `auth-ui`: the client sign-in experience — one identifier field, the
  lichess-style screens, a session that survives relaunch, the password-reset
  and email-verification flows, inline errors, and the rule that a raw session
  token is never shown.
- `server`: the server's configuration and persistence surface — the new
  authentication environment variables, the feature advertisement on
  `/healthz`, and the new account, email, and reset-token tables.

## Impact

- **Server (Rust)** — a new `EmailSender` port with a log transport and a
  provider transport; new configuration fields; four new protocol messages
  (`forgot_password`, `reset_password`, `change_email`, `verify_email`) and
  their error codes; a new SQLite migration adding the account email, the
  email verifications, and the password-reset tokens without touching the
  existing rows; and two small new dependencies (a rustls-based outbound HTTPS
  client for the email transport and the breach check, and SHA-1 for the
  breach check's k-anonymity prefix). The protocol version stays `1`, and an
  existing client keeps playing as a guest exactly as before; only its
  registration stops working, because registration now requires an email
  address.
- **iOS (Swift)** — session storage in the Keychain, the new sign-in,
  registration, password-reset, and verification screens, deep-link handling,
  and a registered URL scheme in the Xcode project.
- **Android (Kotlin)** — session storage in `EncryptedSharedPreferences` (new
  `androidx.security:security-crypto` dependency; the project's `minSdk` 24 is
  compatible), the new screens, and a deep-link intent filter.
- **Deployment** — new environment variables in `.env.example` and
  `docker-compose.prod.yml`, a mounted secret file for the provider key, and a
  runbook section covering the transports, the development flow, and the
  switch to a real provider.
