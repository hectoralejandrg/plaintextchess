# Tasks

## 1. Email Port and Transports

- [x] 1.1 Add `EmailMessage`, `EmailError`, and the `EmailSender` port to `server/src/application/ports.rs`, object-safe like the existing `AuthRecorder` port. Verify: a unit test holds the transport in a `Box<dyn EmailSender>` and compiles under `cargo test --manifest-path server/Cargo.toml`.
- [x] 1.2 Write `server/src/infrastructure/email/templates.rs` with pure functions that build the verification and password-reset messages (recipient, subject, text body, HTML body). Verify: unit tests assert the subject and that the body carries the expected link, with no network or clock dependency.
- [x] 1.3 Write `server/src/infrastructure/email/links.rs`, building the verification and reset links from the configured public base URL. Verify: unit tests cover the custom-scheme form and the `https` form, and that the token appears exactly once in the link.
- [x] 1.4 Write the log transport in `server/src/infrastructure/email/log.rs`, writing the whole message it was given — recipient, subject, and body, including the link — to the server log. Verify: a test asserts that a delivery through it succeeds and that the message body appears in the captured log output.
- [x] 1.5 Write the provider transport in `server/src/infrastructure/email/resend.rs`: `POST https://api.resend.com/emails` with the bearer credential, the mandatory `User-Agent` header, and `NotConfigured` when no credential can be read. Verify: unit tests assert the request body and headers are built as expected, and that a missing credential yields `EmailError::NotConfigured` without any network call.
- [x] 1.6 Add `server/src/infrastructure/email/mod.rs` with a factory that selects the transport from the configuration, refusing to start when the log transport is selected in production mode. Verify: a test asserts the configuration chooses the log transport by default, the provider transport when it is selected, and that production mode with the log transport fails startup naming the misconfiguration.

## 2. Server Configuration

- [x] 2.1 Add the authentication settings to `server/src/infrastructure/config.rs`: `EMAIL_TRANSPORT`, `EMAIL_API_KEY_FILE` with `EMAIL_API_KEY` as an inline fallback, `EMAIL_FROM`, `EMAIL_FROM_NAME`, `PUBLIC_BASE_URL`, `DEPLOYMENT_MODE`, `HIBP_ENABLED`, `AUTH_RATE_LIMIT_LOGIN_PER_MIN`, `AUTH_RATE_LIMIT_REGISTER_PER_HOUR`, and `AUTH_TRUSTED_PROXY_CIDRS`, each with a safe default. Verify: tests in the existing `env_guard` style assert the defaults and that an invalid value is refused at startup.
- [x] 2.2 Add the outbound HTTPS client and SHA-1 dependencies for the mail transport and the breach check, choosing a rustls-based client that needs no OpenSSL in the runtime image. Verify: `cargo build --release --manifest-path server/Cargo.toml` succeeds and the dependency tree contains no OpenSSL crate.
- [x] 2.3 Add the resolver needed to check whether a mail domain can receive mail, and make the check skippable when it cannot be performed. Verify: a unit test asserts that an address whose domain cannot be resolved leaves the flow continuing, and that the skip is recorded in the log.

## 3. Schema Migration

- [x] 3.1 Add the migration for the account email (nullable), the email verifications, and the password-reset tokens, with a unique index on the account email that holds regardless of letter case, and hash-only token columns. Verify: a test in `server/tests/persistence.rs` migrates a database written by the previous schema and asserts every existing account, rating, game, and device link is still present.
- [x] 3.2 Record the account's email verification state on the account and expose it through the in-memory authority, keeping the existing account rows usable without an email. Verify: a test asserts that an account created before the migration loads, signs in by username, and reports an unverified state.

## 4. Username, Email, and Password Rules

- [x] 4.1 Implement the username format rules for newly created accounts in `server/src/domain/account.rs`: the submitted name may be in any letter case, the stored name is lowercase, 2 to 20 characters of letters, digits, underscore, and hyphen, starting with a letter, ending with a letter or digit, with no repeated or edge separator. Verify: table-driven unit tests cover each rejection, the accepted forms, and the lowercasing.
- [x] 4.2 Enforce the username's uniqueness across accounts compared case-insensitively, refuse a username equal to the submitted password in any case, and refuse a username on the server's reject list. Verify: a unit test covers each of the three refusals, and an integration test asserts a new account cannot take a legacy account's name in any case.
- [x] 4.3 Apply the format rules to new accounts only, so an account registered before the change keeps its username. Verify: a test registers a legacy account with an uppercase username, signs in with it, and asserts the stricter rules did not apply to it.
- [x] 4.4 Implement the email validation: a well-formed address, plus an advisory check that the domain can receive mail that is skipped and logged when it cannot be performed. Verify: unit tests cover the malformed addresses, and a test asserts the flow continues when the mail-domain check cannot be performed.
- [x] 4.5 State the password policy in the domain module — 8 to 128 characters, content never inspected, length checked before hashing, and never equal to the username — and expose it to the registration path. Verify: unit tests cover both length boundaries, a passphrase with punctuation, and the password-equals-username refusal.

## 5. Registration, Sign-In, and Resumption

- [x] 5.1 Extend the wire messages so a registration carries a username, an email address, and a password, and add the `email_taken` error code. Verify: protocol tests assert the message shape and the error code's stable name, and that a registration without an email is refused naming the email field.
- [x] 5.2 Extend the sign-in message so it carries an identifier, keeping the legacy username field working, and resolve the identifier against the account's username or email. Verify: protocol and integration tests assert that a username signs in, an email signs in, and a legacy client that sends the username field still signs in.
- [x] 5.3 Add the session resumption message, so a client can present a stored token on a fresh connection and learn the account, the profile, and the expiry it is signed in as. Verify: protocol and integration tests assert that a usable token resumes the session without issuing a new token, that an unknown, expired, or revoked token is refused with the matching error, and that the connection stays open for guest play.
- [x] 5.4 Give every authentication refusal a stable code and a machine-readable field naming the offending field, and add the new codes. Verify: protocol tests assert the error shape carries an optional field, and that each refusal maps to its documented code.
- [x] 5.5 Make the sign-in path perform the same verification work when the identifier names no account as when the password is wrong, so the time an attempt takes does not reveal whether the account exists. Verify: a test counts the hashing work and asserts both attempts perform one verification and answer with the same error and message.
- [x] 5.6 Keep the email out of every profile report and out of the logs, and report the account's email verification state instead. Verify: a test asserts that a profile report holds no email address, and that the verification state is reported.
- [x] 5.7 Make the account's email unique across accounts, case-insensitively, and refuse a duplicate with its own error. Verify: an integration test registers the same address in two cases and asserts the second is refused while the first keeps the address.

## 6. Breached Password Refusal

- [x] 6.1 Implement the breach check: send only the first five characters of the password's SHA-1 to the corpus, with the `User-Agent` header the corpus requires, treat a hit as a refused registration or password change, skip the check and log it when the corpus is unreachable, and honor the configuration switch. Verify: unit tests assert the prefix computation, that a reported breach is refused, and that an unreachable corpus lets the registration through with the skip recorded.

## 7. Password Reset and Email Verification

- [x] 7.1 Implement the reset request: a single-use, expiring token stored only as a hash, answered identically for a registered and an unregistered address, with the message delivered through the email port. Verify: an integration test with a recording sender asserts the answers are identical, that the stored record holds no plaintext token, and that no message is sent for an unknown address.
- [x] 7.2 Implement the reset completion: a valid token changes the password, invalidates the token, issues no session, and leaves the account's other sessions working. Verify: an integration test asserts the new password signs in, the old one does not, the token cannot be reused, and the other session still works.
- [x] 7.3 Implement the email verification: a new account is unverified until its token is used, asking again delivers a new token and drops the previous one, and a failed delivery leaves the account intact. Verify: an integration test with a recording sender asserts the verification state before and after, that a repeated request delivers a second token and retires the first, and that a failing sender leaves the account usable.
- [x] 7.4 Implement the email change: the previous address stays in force until the new one is confirmed, an address another account already holds is refused, and confirming replaces the old one. Verify: an integration test asserts the original address still receives until the new one is confirmed, that a duplicate new address is refused, and that the confirmed change replaces the old address.
- [x] 7.5 Add the new wire messages for the resumption, the reset request, the reset completion, the email change, the verification, and the verification resend, keeping the protocol version at `1`. Verify: protocol tests assert each message's shape and that an existing client that sends none of them is unaffected.

## 8. Rate Limiting

- [x] 8.1 Implement the attempt windows: per client address and per device, in memory, with the limits coming from the configuration. Verify: unit tests assert that the attempts over the limit are refused with the dedicated error, that another address and another device are unaffected, and that the windows reset as they expire.
- [x] 8.2 Take the client address from the forwarding header only when the peer is covered by one of the configured trusted proxy networks, and from the peer's own address otherwise. Verify: a test asserts the header is used for a trusted proxy, ignored for another peer, and that the configured networks are parsed.
- [x] 8.3 Refuse an over-limit attempt before the password is hashed. Verify: a test counts the hashing work and asserts a refused attempt performs none.

## 9. Capability Advertisement

- [x] 9.1 Report the authentication features the running server supports in the readiness endpoint's body, using the fixed names `password_reset`, `email_verification`, `email_change`, `breached_password_refusal`, and `session_resumption`, and omit a feature that is turned off. Verify: a test asserts the body names the features, that every name is one of the fixed ones, and that a disabled feature does not appear.

## 10. iOS Client

- [x] 10.1 Store the session token in the keychain and add the "keep me signed in" choice, resuming the session with the stored token on the next connection and deleting it on sign-out or when the choice was declined. Verify: a unit test drives the storage round-trip, and the app comes back signed in after a relaunch.
- [x] 10.2 Replace the sign-in screen with the lichess-style flow: one identifier field, a primary sign-in action, a separate create-account path, guest play as a first-class choice, and inline errors that keep the form open and mark the field the error names. Verify: the app builds, and a manual pass covers signing in, registering, and guest play.
- [x] 10.3 Add the password-reset and email-verification screens, accepting the token by link or pasted code, asking for the new password twice, and showing that a reset does not sign the player in. Verify: the app builds, and a manual pass covers the reset with a pasted code.
- [x] 10.4 Register the app's URL scheme and handle the incoming link so a reset link opens the reset screen with the token filled in. Verify: the app builds, and a manual pass opens the app from a link.
- [x] 10.5 Remove the raw session token from the signed-in view and show the account's name and verification state instead. Verify: a manual pass finds no token and no control that reveals one.
- [x] 10.6 Read the server's advertised features with an HTTPS request to the readiness endpoint at the game connection's address, with a short timeout, and assume no optional feature when the request fails or times out. Verify: a manual pass against a server that does not advertise the features hides those paths and leaves sign-in and guest play working.
- [x] 10.7 Offer only the registration the server supports: no email field when the server does not advertise the email feature, and no field the server would ignore. Verify: a manual pass against a server without the email feature shows a registration form without the email field.

## 11. Android Client

- [x] 11.1 Add the encrypted shared preferences dependency and store the session token in it, with the "keep me signed in" choice, resuming the session with the stored token on the next connection and deleting it on sign-out. Verify: the module compiles, and a unit test drives the storage round-trip.
- [x] 11.2 Replace the sign-in screen with the lichess-style flow: one identifier field, a primary sign-in action, a separate create-account path, guest play as a first-class choice, and inline errors that keep the form open and mark the field the error names. Verify: the module compiles, and a manual pass covers signing in, registering, and guest play.
- [x] 11.3 Add the password-reset and email-verification screens, accepting the token by link or pasted code, asking for the new password twice. Verify: the module compiles, and a manual pass covers the reset with a pasted code.
- [x] 11.4 Add the deep-link intent filter and handle the incoming link so a reset link opens the reset screen with the token filled in. Verify: the module compiles, and a manual pass opens the app from a link.
- [x] 11.5 Remove the raw session token from the signed-in view and show the account's name and verification state instead. Verify: a manual pass finds no token and no control that reveals one.
- [x] 11.6 Read the server's advertised features with an HTTPS request to the readiness endpoint at the game connection's address, with a short timeout, and assume no optional feature when the request fails or times out. Verify: a manual pass against a server that does not advertise the features hides those paths and leaves sign-in and guest play working.
- [x] 11.7 Offer only the registration the server supports: no email field when the server does not advertise the email feature, and no field the server would ignore. Verify: a manual pass against a server without the email feature shows a registration form without the email field.

## 12. Deployment

- [x] 12.1 Add the new environment variables to `.env.example` and `docker-compose.prod.yml` — `EMAIL_TRANSPORT`, `EMAIL_API_KEY_FILE`, `EMAIL_FROM`, `EMAIL_FROM_NAME`, `PUBLIC_BASE_URL`, `DEPLOYMENT_MODE`, `HIBP_ENABLED`, `AUTH_RATE_LIMIT_LOGIN_PER_MIN`, `AUTH_RATE_LIMIT_REGISTER_PER_HOUR`, `AUTH_TRUSTED_PROXY_CIDRS` — mount the provider credential as a file, and ignore the secrets directory. Verify: `docker compose -f docker-compose.prod.yml config` renders the defaults, the overrides change the rendered values, and the credential never appears in the rendered environment.
- [x] 12.2 Document the transports, the development flow with the log transport, the production-mode refusal, the switch to a real provider, the trusted-proxy setting, and the secret file's handling in `docs/deploy-docker.md`. Verify: the documented commands run as written against the deployed box.

## 13. End-to-End Verification

- [x] 13.1 Run the whole flow against the deployed box: register, read the verification link from the log, confirm it, ask for a reset, read the reset link, set a new password, sign in, and confirm the session is resumed after an app restart. Verify: every step is observed end to end over the TLS endpoint, and the persisted rows are readable in the database.
- [x] 13.2 Keep the guest path working: a connection with no session, an expired session, or a revoked session still creates and joins a room and plays, and a refused sign-in leaves the guest player's seat and rating untouched. Verify: a regression test asserts each of the three cases, so the guest fallback cannot silently change.
- [x] 13.3 Have the flow reviewed as a whole by a second pass that did not write it, with the same evidence format as the earlier deployment pass. Verify: the review reports a pass or fail per criterion, with the command output behind each verdict.

## Workflow follow-up

- Archive the change after the project's review requirements are satisfied.
- When a mail provider and a sending domain exist: add the domain's DNS records, put the provider credential in the mounted secret file, switch the transport, and set the public base URL to the deployment's address. No code change is needed.
- When a public domain exists: add the universal-link handling (the iOS associated domain and the Android app-links asset) and the web landing page for the reset link.
- Curate the username reject list's contents.
- Two-factor authentication, OAuth sign-in, and the features that depend on a verified email are separate changes.
