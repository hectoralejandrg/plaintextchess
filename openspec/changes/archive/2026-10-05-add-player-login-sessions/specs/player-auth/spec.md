# Spec Delta: player-auth

## Purpose
Optional player accounts on the online server: a player registers a username and password once, logs in to receive a bearer session, and that session follows the account across devices, while unauthenticated guest play by device identifier keeps working exactly as before.

## ADDED Requirements

### Requirement: Player Account Registration
The server MUST allow a connecting player to create an account by presenting a username and a password, and MUST answer a successful registration with a newly issued session for the new account together with that account's profile. Usernames MUST be unique across all accounts, compared case-insensitively, so `Ana` and `ana` are the same username and a second registration with either spelling MUST be rejected. A username MUST consist of 3 to 24 characters drawn from letters, digits, underscore, and hyphen, and MUST NOT be only whitespace. A password MUST be between 8 and 128 characters. A registration that violates the username or password format MUST be rejected with a validation error naming the offending field, and the server MUST create no account and no session for it.

The server MUST assign the account its own server-generated identifier rather than reusing anything the client chose. The submitted password MUST be stored only as a slow one-way hash: the server MUST NOT persist the plaintext password in the database, in any log line, in an error message, or in any response to the client. A device that registers becomes linked to the new account, so later play from that device is attributed to the account.

#### Scenario: Registering creates an account and returns a session
- **WHEN** a player registers with a well-formed, unused username and a valid password
- **THEN** the server creates the account with its own server-generated identifier, links the connecting device to it, and answers with a session token and the account's profile

#### Scenario: A taken username is rejected regardless of case
- **WHEN** a player registers with a username that differs only in letter case from an existing account's username
- **THEN** the server rejects the registration with a `username_taken` error, creates no account, and issues no session

#### Scenario: Malformed credentials are rejected without side effects
- **WHEN** a player registers with a username or password outside the allowed format
- **THEN** the server answers with a validation error identifying the offending field, and no account, session, or device link is created

#### Scenario: The password is never echoed back
- **WHEN** a registration succeeds or fails
- **THEN** no response and no server log line contains the submitted password in plaintext

### Requirement: Login and Session Issuance
The server MUST allow a connecting player to present a username and password and, when both match an existing account, MUST issue a session token for that account. The session token MUST be a long random opaque value chosen by the server, and the server MUST persist only a one-way hash of it: the database MUST NOT contain any value from which the token can be recovered. When the username does not exist or the password does not match — including when the password is wrong for a real username — the server MUST reject the attempt with the same `invalid_credentials` error carrying the same generic message, so a caller cannot learn whether a username is taken. A successful login MUST link the connecting device to the account, and MUST NOT change the account's display name, ratings, or game history.

#### Scenario: Correct credentials issue a session
- **WHEN** a player presents the username and password of an existing account
- **THEN** the server issues a session token bound to that account, links the connecting device to the account, and answers with the token and the account's profile

#### Scenario: A wrong password is rejected
- **WHEN** a player presents an existing username with an incorrect password
- **THEN** the server answers `invalid_credentials`, issues no session, and leaves the account's data unchanged

#### Scenario: An unknown username is indistinguishable from a wrong password
- **WHEN** a player presents a username that no account holds
- **THEN** the server answers with the same `invalid_credentials` error and the same message it would give for an existing username with a wrong password, and no session is issued

#### Scenario: The issued token is not recoverable from the database
- **WHEN** a login succeeds
- **THEN** the stored session record contains only a hash of the issued token, and no stored field equals the token the client received

### Requirement: Session Lifetime, Reuse, and Revocation
A session MUST remain usable until it expires or is revoked. A session MUST expire once its lifetime has elapsed, with the lifetime coming from server configuration and having a built-in default. An expired or revoked session MUST be rejected as unauthenticated and MUST NOT be silently renewed: using an expired session MUST NOT create a new one. An account MUST be allowed to hold more than one active session at a time, so the same player can be signed in on a phone and a tablet, and MUST be allowed to revoke exactly one of them. A `logout` MUST revoke only the session it presents and MUST leave the account's other sessions untouched. Every session attempt — accepted, expired, or revoked — MUST be recorded in the server log without the token value.

#### Scenario: An expired session stops authenticating
- **WHEN** a client presents a session token whose lifetime has elapsed since it was issued
- **THEN** the server treats the connection as unauthenticated, answers an authentication request with `session_expired`, and issues no replacement token

#### Scenario: Logging out revokes only the presented session
- **WHEN** a client logs out with one of an account's active sessions
- **THEN** that session stops authenticating while the account's other active sessions keep working

#### Scenario: One account may hold sessions on several devices
- **WHEN** the same player logs in from two different devices
- **THEN** both sessions are active independently and revoking one leaves the other working

#### Scenario: A revoked session no longer authenticates
- **WHEN** a client presents a session token that has been revoked
- **THEN** the server treats the connection as unauthenticated and answers with an authentication error

### Requirement: Device-to-Account Profile Linkage
Each device MUST be linked to at most one account at a time. Registering or logging in from a device MUST link that device to the account, and MUST re-link a device that was previously linked to a different account, so a device that signs into an account adopts that account's profile. A linked device's profile MUST carry the account's identifier, username, and display name, and the display name MUST default to the username when the account has none. A profile MUST be durable: it MUST be stored in the persistence database and reloaded at server startup, so a device that was linked before a restart is still linked afterwards. Logging out MUST end the session but MUST retain the device-to-account link, so the account is still recognized on that device and no authenticated action is possible until the player logs in again.

#### Scenario: Logging in re-links a device that belonged to another account
- **WHEN** a device that was linked to one account logs in with the credentials of a different account
- **THEN** the device is linked to the newly authenticated account and its profile reports that account, and no reference to the previous account remains for that device

#### Scenario: A device belongs to one account at a time
- **WHEN** a device is already linked to an account
- **THEN** it is linked to that account and no other, and one device cannot be linked to two accounts simultaneously

#### Scenario: The profile defaults its display name to the username
- **WHEN** an account has no display name set
- **THEN** the profile the server reports for it carries the username as the display name

#### Scenario: A device stays linked across a server restart
- **WHEN** a server that has linked a device to an account stops and restarts against the same database
- **THEN** the link is loaded at startup and the same device is still linked to that account

#### Scenario: Logging out keeps the link but ends the session
- **WHEN** a player logs out on a linked device
- **THEN** the device remains linked to the account and still reports its profile, but the connection is unauthenticated and any authenticated-only request is refused until the player logs in again

### Requirement: Profile Display Name
An account MUST be able to carry a display name that differs from its username, and that display name MUST belong to the account rather than to the device that set it, so every device signed in to the account reports the same display name. After a display name is set it MUST survive a server restart and MUST be reported by the profile until the account's username changes or the display name is replaced. A display name MUST be between 1 and 32 characters after surrounding whitespace is trimmed, and MUST NOT contain control characters; a request violating these limits MUST be rejected with a validation error and MUST leave the previously stored display name unchanged. An account with no display name set MUST report its username as the display name.

#### Scenario: Setting a display name updates the account's profile
- **WHEN** an authenticated player sets a valid display name on their account
- **THEN** the server stores it as the account's display name and reports it back in the account's profile

#### Scenario: A display name follows the account across devices
- **WHEN** a player sets a display name on one device and later reports the profile from a second device signed in to the same account
- **THEN** the second device reports the same display name

#### Scenario: An invalid display name is rejected and the old one is kept
- **WHEN** a player submits a display name that is empty after trimming, longer than 32 characters, or contains control characters
- **THEN** the server answers with a validation error and the account's previously stored display name is unchanged

#### Scenario: The display name survives a restart
- **WHEN** a player set a display name and the server restarts against the same database
- **THEN** the reported profile still carries that display name

#### Scenario: A rejected display name keeps reporting the username
- **WHEN** an account has never had a display name set
- **THEN** every profile report for that account carries the username as the display name

### Requirement: Authentication Wire Messages
The server MUST accept registration, login, and logout requests, and a request to change the account's display name, as new client message types on the existing WebSocket protocol, and MUST answer them with new server message types carrying the issued session and the resolved profile. The protocol version MUST NOT change, and every existing message and field MUST keep its current shape, so a client that sends no authentication message at all MUST continue to play online games exactly as before. An authentication request MUST be answerable at any point on a connection, including before the player has created or joined a room. Changing the display name MUST require an authenticated session and MUST be refused on a guest connection.

#### Scenario: A client can log in before joining a room
- **WHEN** a client sends a login request on a freshly opened connection that has not created or joined any room
- **THEN** the server answers with the session and profile, and the connection stays open and usable for creating or joining a room afterwards

#### Scenario: Existing clients are unaffected
- **WHEN** a client built against the current protocol connects and plays a game without sending any authentication message
- **THEN** the game plays normally, the connection is not rejected, and the protocol version reported by the server is unchanged

#### Scenario: An authentication failure keeps the connection open
- **WHEN** a login request is rejected
- **THEN** the server answers with the authentication error, and the connection remains open so the player can retry or play as a guest

#### Scenario: The player learns its identity after authenticating
- **WHEN** a client authenticates successfully
- **THEN** the server reports back the account it resolved, the session's expiry, and the profile, so the client knows which account it is signed in as

#### Scenario: A guest connection cannot change a display name
- **WHEN** a connection with no valid session requests a display-name change
- **THEN** the server answers `not_authenticated`, stores nothing, and leaves the connection open for the player to log in and retry

### Requirement: Guest Play Fallback
A connection that presents no session, an expired session, a revoked session, or an unknown session token MUST still be able to create and join rooms and play games, identified by its device identifier exactly as an unauthenticated player is today. The server MUST NOT refuse gameplay because authentication is missing or failed. Every player interaction available to an unauthenticated player MUST remain available to an authenticated one, so authenticating never removes a capability. A failed or rejected authentication MUST NOT alter the guest identity, device link, rating, or room membership the player already had.

#### Scenario: An unauthenticated player can still play
- **WHEN** a client that has never registered or logged in creates and joins a room and plays a game
- **THEN** the game is accepted and rated exactly as it is today, with the player identified by its device identifier

#### Scenario: An invalid session does not block play
- **WHEN** a client presents an unknown, expired, or revoked session token and then creates or joins a room
- **THEN** the room action is accepted and the game is played with the device identifier, as if no session had been presented

#### Scenario: Authenticating adds capabilities but removes none
- **WHEN** a guest player logs in and then plays a game
- **THEN** every action the guest could perform before logging in is still permitted, and the room, ratings, and clock behave identically

#### Scenario: A rejected login leaves the guest player untouched
- **WHEN** a login attempt fails for a player that is already playing as a guest
- **THEN** the player's device link, rating, seat, and ongoing game are unchanged

### Requirement: Credential and Session Data Protection
Credentials and session tokens MUST be treated as secrets end to end. The server MUST NOT write a submitted password or an issued token to any log line, and any log line about an authentication attempt MUST identify the outcome without revealing the secret. A submitted password MUST be length-bounded before it is hashed, so an oversized input cannot make the server spend unbounded CPU hashing it. Configuration of the password hashing cost and the session lifetime MUST come from environment configuration with safe built-in defaults, and a configured value below the server's supported minimum MUST be rejected at startup rather than silently weakened.

#### Scenario: Secrets are absent from the logs
- **WHEN** an authentication attempt succeeds, fails, expires, or is revoked
- **THEN** the server log records the outcome and the account or device involved, and contains neither the submitted password nor the issued or presented token

#### Scenario: An oversized password does not consume unbounded work
- **WHEN** a player submits a password far longer than the allowed maximum
- **THEN** the server rejects it without performing the full hashing cost an accepted password would require

#### Scenario: A below-minimum session lifetime is refused at startup
- **WHEN** the server starts with a configured session lifetime shorter than the supported minimum
- **THEN** startup fails with an explicit configuration error rather than running with the weaker value

#### Scenario: Authentication settings come from the environment
- **WHEN** the server starts with no authentication-related environment variables set
- **THEN** it starts successfully using its built-in defaults for the hashing cost and the session lifetime
