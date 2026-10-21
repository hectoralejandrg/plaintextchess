# player-auth Specification

## Purpose

Optional player accounts on the online server: a player registers a username,
an email address, and a password once, signs in with either the username or the
email to receive a bearer session, and that session follows the account across
devices and app restarts, while unauthenticated guest play by device identifier
keeps working exactly as before.

## Requirements

### Requirement: Player Account Registration
The server MUST allow a connecting player to create an account by presenting a
username, an email address, and a password, and MUST answer a successful
registration with a newly issued session for that account together with the
account's profile. The account MUST carry a server-generated identifier rather
than anything the client chose.

#### Scenario: Registering creates an account and returns a session
- **WHEN** a player registers with a well-formed, unused username, a well-formed email address, and a valid password
- **THEN** the server creates the account with its own server-generated identifier, links the connecting device to it, and answers with a session token and the account's profile

#### Scenario: A malformed field is rejected without side effects
- **WHEN** a player registers with a username, email address, or password outside the allowed format
- **THEN** the server answers with a validation error identifying the offending field, and no account, session, or device link is created

### Requirement: Account Credentials Are Stored as One-Way Hashes
The submitted password MUST be stored only as a slow one-way hash. The server
MUST NOT persist the plaintext password in the database, in any log line, in an
error message, or in any response to the client. A device that registers MUST
become linked to the new account, so later play from that device is attributed
to the account.

#### Scenario: The password is never echoed back
- **WHEN** a registration succeeds or fails
- **THEN** no response and no server log line contains the submitted password in plaintext

#### Scenario: A registering device is linked to the new account
- **WHEN** a player registers from a device
- **THEN** later play from that device is attributed to the newly created account

### Requirement: Password Policy
A password MUST be between 8 and 128 characters, and its content MUST NOT be
inspected: spaces and punctuation are the point of a passphrase. The length
MUST be checked before the password is hashed. A password MUST NOT be the
account's username.

#### Scenario: The password length boundaries are enforced
- **WHEN** a player submits a password shorter than 8 characters or longer than 128
- **THEN** the server answers with a validation error naming the password field, and hashes nothing

#### Scenario: The password's content is never inspected
- **WHEN** a player submits a passphrase with spaces and punctuation
- **THEN** the server accepts it as long as its length is within the bounds

#### Scenario: A password equal to the username is rejected
- **WHEN** a player registers with a password identical to the submitted username
- **THEN** the server answers with a validation error naming the password field

### Requirement: Username Policy
The submitted username MAY be in any letter case. The stored username MUST be
lowercase, and MUST be 2 to 20 characters drawn from letters, digits,
underscore, and hyphen. It MUST start with a letter, MUST end with a letter or
digit, and MUST NOT contain a repeated separator or start or end with one.
These rules apply to accounts created after this change.

#### Scenario: A well-formed username is accepted and normalized to lowercase
- **WHEN** a player registers with a username that satisfies the format in a different letter case
- **THEN** the server accepts it, stores it in lowercase, and the account reports that lowercase name

#### Scenario: A username outside the format is rejected naming the field
- **WHEN** a player registers with a username that is too short, too long, starts with a digit or a separator, ends with a separator, contains a repeated separator, or contains a character outside the allowed set
- **THEN** the server answers with a validation error naming the username field, and no account is created

#### Scenario: An existing account keeps its username
- **WHEN** an account that was created before these rules existed signs in
- **THEN** the server accepts its username exactly as it was registered, and the stricter rules apply only to newly created accounts

### Requirement: Username Uniqueness and Rejection
A username MUST be unique across all accounts, compared case-insensitively, so
a new account cannot take a legacy account's name in any case. A username MUST
NOT equal the submitted password, compared case-insensitively. A username on the
server's reject list MUST be refused.

#### Scenario: A taken username is rejected regardless of case
- **WHEN** a player registers with a username that differs only in letter case from an existing account's username
- **THEN** the server rejects the registration with a `username_taken` error, creates no account, and issues no session

#### Scenario: A username equal to the password is rejected
- **WHEN** a player registers with a username identical to the submitted password, in any letter case
- **THEN** the server rejects the registration with a validation error, and no account is created

#### Scenario: A username on the reject list is refused
- **WHEN** a player registers with a username that the server's reject list holds
- **THEN** the server rejects the registration with a validation error naming the username field

### Requirement: Email Identity
A registration MUST carry an email address, and the email MUST be the account's
private identity: unique across all accounts, compared case-insensitively, and
never published to another player, returned in a profile report, or written to
a log line. A duplicate email MUST be rejected with its own error. The address
MUST be well-formed. The server SHOULD check that the domain can receive mail,
and MUST continue when the check cannot be performed.

#### Scenario: A duplicate email is rejected
- **WHEN** a player registers with an email address that an existing account already holds, in any letter case
- **THEN** the server rejects the registration with an `email_taken` error, creates no account, and issues no session

#### Scenario: The email is never disclosed to another player
- **WHEN** a player reports the profile of an account that has an email address
- **THEN** the reported profile carries the account's username, display name, and verification state, and never its email address

#### Scenario: A malformed email is rejected naming the field
- **WHEN** a player registers with an address that is not well-formed
- **THEN** the server answers with a validation error naming the email field, and no account is created

#### Scenario: A mail-domain check that cannot be performed does not block the registration
- **WHEN** the server cannot determine whether the address's domain can receive mail
- **THEN** the registration continues, and the server logs that the check was skipped

### Requirement: Breached Password Refusal
The server MUST refuse a password that appears in a known breach corpus, using
a k-anonymity check that reveals nothing about the submitted password beyond a
short prefix of its hash. The check MUST be configurable and MUST be skippable,
and when the corpus is unreachable the server MUST NOT block the registration or
the password change.

#### Scenario: A breached password is refused
- **WHEN** a player registers, or sets a new password, with a password that the corpus reports as breached
- **THEN** the server rejects it with a validation error naming the password field, and creates no account or changes no password

#### Scenario: The check reveals nothing about the password
- **WHEN** the server consults the breach corpus
- **THEN** the request carries only a short prefix of the password's hash and never the password itself

#### Scenario: An unreachable corpus does not block the registration
- **WHEN** the corpus cannot be reached
- **THEN** the server accepts the password and records that the check was skipped

### Requirement: Login and Session Issuance
The server MUST allow a connecting player to present an identifier — the
account's username or its email address — together with a password, and MUST
issue a session token when both match. The token MUST be a long random opaque
value chosen by the server, and only a one-way hash of it MUST be persisted.
When the identifier is unknown or the password is wrong, the server MUST answer
the same `invalid_credentials` error with the same generic message.

#### Scenario: Correct credentials issue a session
- **WHEN** a player presents the username and password of an existing account
- **THEN** the server issues a session token bound to that account, links the connecting device to the account, and answers with the token and the account's profile

#### Scenario: A wrong password is rejected
- **WHEN** a player presents an existing identifier with an incorrect password
- **THEN** the server answers `invalid_credentials`, issues no session, and leaves the account's data unchanged

#### Scenario: An unknown identifier is indistinguishable from a wrong password
- **WHEN** a player presents an identifier that no account holds
- **THEN** the server answers with the same `invalid_credentials` error and the same message it would give for an existing identifier with a wrong password, and no session is issued

#### Scenario: An email address works as the identifier
- **WHEN** a player presents an existing account's email address and its password
- **THEN** the server issues a session for that account exactly as it would for the username

#### Scenario: The issued token is not recoverable from the database
- **WHEN** a login succeeds
- **THEN** the stored session record contains only a hash of the issued token, and no stored field equals the token the client received

### Requirement: Session Resumption
A client MUST be able to present a session token it stored earlier on a fresh
connection, and the server MUST answer with the session and the profile when
the token is still usable. A token that is unknown, expired, or revoked MUST be
answered with the corresponding authentication error, and the connection MUST
stay open so the player can continue as a guest. Resumption MUST NOT issue a
new token and MUST NOT extend the session's lifetime.

#### Scenario: A stored token resumes the session
- **WHEN** a client presents a token it stored from an earlier session, and that session is still usable
- **THEN** the server answers with the session and the profile, and issues no new token

#### Scenario: A stale token is refused and the connection stays open
- **WHEN** a client presents a token that is unknown, expired, or revoked
- **THEN** the server answers with the corresponding authentication error, issues no token, and leaves the connection open for guest play

#### Scenario: Resumption does not extend the session
- **WHEN** a client resumes a session close to its expiry
- **THEN** the session's expiry is unchanged, and the server issues no replacement token

### Requirement: Credential Attempts Do Not Reveal Account Existence
An attempt whose identifier names no account MUST perform the same work as an
attempt whose password is wrong, so the time an attempt takes does not tell a
caller whether the account exists. The server MUST answer both with the same
error and the same message.

#### Scenario: An unknown identifier costs the same as a wrong password
- **WHEN** a caller signs in with an identifier no account holds, and then with an existing identifier and a wrong password
- **THEN** both attempts perform the same verification work and answer with the same error and message

### Requirement: Session Lifetime, Reuse, and Revocation
A session MUST remain usable until it expires or is revoked, with the lifetime
coming from server configuration and having a built-in default. An expired or
revoked session MUST be rejected as unauthenticated and MUST NOT be silently
renewed. An account MUST be allowed to hold more than one active session, and
MUST be allowed to revoke exactly one of them. Every session attempt MUST be
recorded in the server log without the token value.

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
Each device MUST be linked to at most one account at a time. Registering or
logging in from a device MUST link that device to the account, and MUST
re-link a device that was previously linked to a different account, so a device
that signs into an account adopts that account's profile. Logging out MUST end
the session but MUST retain the device-to-account link, so the account is still
recognized on that device and no authenticated action is possible until the
player signs in again.

#### Scenario: Logging in re-links a device that belonged to another account
- **WHEN** a device that was linked to one account signs in with the credentials of a different account
- **THEN** the device is linked to the newly authenticated account and its profile reports that account, and no reference to the previous account remains for that device

#### Scenario: A device belongs to one account at a time
- **WHEN** a device is already linked to an account
- **THEN** it is linked to that account and no other, and one device cannot be linked to two accounts simultaneously

#### Scenario: Logging out keeps the link but ends the session
- **WHEN** a player logs out on a linked device
- **THEN** the device remains linked to the account and still reports its profile, but the connection is unauthenticated and any authenticated-only request is refused until the player signs in again

### Requirement: Persistent Player Profile
The server MUST maintain a persistent profile for each device identifier, stored
in the persistence database and loaded at startup, so a reconnecting player is
recognized by its profile rather than treated as an unknown device. A profile
MUST carry the device identifier, the linked account identifier, and a creation
timestamp, and its durable write MUST NOT block the player's connection or the
room the player occupies.

#### Scenario: A device's profile is created on first connection
- **WHEN** a device identifier is seen for the first time
- **THEN** the server creates its profile with a creation timestamp, commits it, and continues with the connection

#### Scenario: The profile reloads across restarts
- **WHEN** the server restarts against the same database
- **THEN** the profiles are loaded at startup and a reconnecting player receives its existing profile rather than a new one

#### Scenario: A failed profile write does not block the connection
- **WHEN** the profile cannot be committed
- **THEN** the server logs the failure and the player still connects and plays

### Requirement: Profile Display Name
An account MUST be able to carry a display name that differs from its username,
and that display name MUST belong to the account rather than to the device that
set it. A display name MUST be between 1 and 32 characters after surrounding
whitespace is trimmed and MUST NOT contain control characters; a request
violating these limits MUST be rejected and MUST leave the stored display name
unchanged. An account with no display name set MUST report its username as the
display name.

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

### Requirement: Authentication Wire Messages
The server MUST accept registration, login, session resumption, and logout
requests, a display-name change, and the email-related requests (reset request,
password reset, email change, verification resend, and verification
confirmation) as client message types on the existing WebSocket protocol, and
MUST answer them with server message types carrying the issued session and the
profile. The protocol version MUST NOT change, and every existing message and
field MUST keep its current shape.

#### Scenario: A client can sign in before joining a room
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
- **THEN** the server reports back the account it resolved, the session's expiry, the profile, and the account's email verification state

#### Scenario: A guest connection cannot change a display name
- **WHEN** a connection with no valid session requests a display-name change
- **THEN** the server answers `not_authenticated`, stores nothing, and leaves the connection open for the player to sign in and retry

### Requirement: Password Reset by Email
A player MUST be able to ask for a password reset by email address, and the
server MUST answer with a single-use token that expires, is stored only as a
one-way hash, and is delivered to the account's email address. The answer MUST
be identical whether or not the address belongs to an account, so the requester
cannot learn which addresses are registered. Setting a new password with a
valid token MUST change the password and MUST leave the account's other
sessions untouched.

#### Scenario: A reset request delivers a token
- **WHEN** a player asks for a reset link for an address that belongs to an account
- **THEN** the server delivers a message to that address carrying a single-use token, and stores only a hash of it

#### Scenario: The answer never reveals whether the address exists
- **WHEN** a player asks for a reset link for an address that belongs to no account
- **THEN** the server answers with the same confirmation it gives for a registered address, and delivers nothing

#### Scenario: A valid token sets a new password
- **WHEN** a player presents a valid reset token and a new password
- **THEN** the server stores the new password's hash, invalidates the token, issues no session, and leaves the account's other sessions working

#### Scenario: An expired or already-used token is refused
- **WHEN** a player presents a reset token that has expired or has already been used
- **THEN** the server refuses it with a dedicated error and changes no password

#### Scenario: The reset does not sign the player in
- **WHEN** a player completes a password reset
- **THEN** no session is issued, and the player must sign in with the new password

### Requirement: Email Verification
A newly registered account MUST be sent a verification token, and the account
MUST report whether its email address is verified. The player MUST be able to
ask again for a verification message; that request MUST deliver a new token and
MUST invalidate the previous one, and MUST leave the account's state unchanged.
The token MUST be single-use and expiring, and its delivery MUST NOT be able to
lose the account that was already created.

#### Scenario: A new account is unverified until it is confirmed
- **WHEN** a player registers
- **THEN** the account reports that its email address is not yet verified, and the server delivers a verification token to that address

#### Scenario: Confirming marks the account verified
- **WHEN** a player presents a valid verification token
- **THEN** the account reports that its email address is verified, and the token cannot be used again

#### Scenario: Asking again delivers a new token and drops the old one
- **WHEN** a player asks again for a verification message
- **THEN** the server delivers a new token, the previous token can no longer be used, and the account keeps its existing state

#### Scenario: The account survives a failed delivery
- **WHEN** the verification message cannot be delivered
- **THEN** the account still exists and can be verified by asking again

### Requirement: Email Change
A signed-in player MUST be able to ask for a different email address. The
account's current address MUST stay in force until the new address is
confirmed, so a player who starts the change and never confirms keeps
receiving mail at the old one. A new address another account already holds
MUST be refused. Confirming the new address MUST replace the old one, and the
old address MUST stop receiving the account's mail.

#### Scenario: The old address stays in force until the new one is confirmed
- **WHEN** a player asks for a different email address and does not confirm it
- **THEN** the account still reports its original address, and mail for that account is still delivered to it

#### Scenario: A new address already in use is refused
- **WHEN** a player asks for an address that another account already holds
- **THEN** the server refuses the request with the email-taken error, and the account keeps its original address

#### Scenario: Confirming replaces the old address
- **WHEN** a player confirms the new address
- **THEN** the account reports the new address, and the old address no longer receives the account's mail

### Requirement: Authentication Rate Limiting
The server MUST limit how often a client address and a device may attempt to
register or to sign in, and MUST answer an attempt that exceeds the limit with
a dedicated error that says when the player may try again. The limits MUST be
kept per action, so signing in does not fill the registration window, and per
client address and per device. The limits MUST come from configuration.

#### Scenario: Repeated failures are rate limited
- **WHEN** a client exceeds the number of sign-in or registration attempts the limit allows
- **THEN** the server refuses the attempt with a rate-limit error, and the attempt performs no password hashing

#### Scenario: A limit is kept per client address
- **WHEN** one client address exceeds the limit
- **THEN** the attempts from that address are refused, while another address is unaffected

#### Scenario: A limit is kept per device
- **WHEN** one device exceeds the limit
- **THEN** the attempts from that device are refused, while another device is unaffected

#### Scenario: A forged forwarding header is ignored
- **WHEN** a peer that is not a configured trusted proxy sends a forwarding header
- **THEN** the server keys that peer by its own address and ignores the header's value

#### Scenario: The limits come from the environment
- **WHEN** the server starts with no rate-limit configuration
- **THEN** it starts successfully using its built-in defaults

### Requirement: The Client Address Comes From the Proxy
The client address MUST come from the forwarding header the reverse proxy sets,
used only when the peer is a configured trusted proxy; otherwise the peer's own
address MUST be used. The address MUST be the one the proxy saw — the last
entry of the header's chain — and never one the client chose. The trusted
proxies MUST come from configuration.

#### Scenario: A header the client chose does not move the bucket
- **WHEN** a client sends a forwarding header of its own, and the proxy appends the address it saw
- **THEN** the attempt is counted against the address the proxy saw, and rotating the client's own header does not give it a new bucket

#### Scenario: A peer that is not the proxy is keyed by its own address
- **WHEN** a peer that is not a configured trusted proxy sends a forwarding header
- **THEN** the server keys the attempt by the peer's own address and ignores the header

### Requirement: Guest Play Fallback
A connection that presents no session, an expired session, a revoked session, or
an unknown session token MUST still be able to create and join rooms and play
games, identified by its device identifier. The server MUST NOT refuse gameplay
because authentication is missing or failed. Every player interaction available
to an unauthenticated player MUST remain available to an authenticated one. A
rejected authentication MUST NOT alter the guest identity, device link, rating,
or room membership.

#### Scenario: An unauthenticated player can still play
- **WHEN** a client that has never registered or signed in creates and joins a room and plays a game
- **THEN** the game is accepted and rated exactly as it is today, with the player identified by its device identifier

#### Scenario: An invalid session does not block play
- **WHEN** a client presents an unknown, expired, or revoked session token and then creates or joins a room
- **THEN** the room action is accepted and the game is played with the device identifier, as if no session had been presented

#### Scenario: Authenticating adds capabilities but removes none
- **WHEN** a guest player signs in and then plays a game
- **THEN** every action the guest could perform before signing in is still permitted, and the room, ratings, and clock behave identically

#### Scenario: A rejected sign-in leaves the guest player untouched
- **WHEN** a sign-in attempt fails for a player that is already playing as a guest
- **THEN** the player's device link, rating, seat, and ongoing game are unchanged

### Requirement: Credential and Session Data Protection
Credentials, session tokens, and email addresses MUST be treated as secrets end
to end. The server MUST NOT write a submitted password or an issued token to a
log line, and MUST NOT write an account's email address to a log line or to
another player's profile report. The only exception is the development-only log
transport, which writes the message it was given. The hashing cost and the
session lifetime MUST come from environment configuration.

#### Scenario: Secrets are absent from the logs
- **WHEN** an authentication attempt succeeds, fails, expires, or is revoked
- **THEN** the server log records the outcome and the account or device involved, and contains neither the submitted password nor the issued or presented token

#### Scenario: An email address is absent from the logs
- **WHEN** a registration, a sign-in, or a reset request is recorded
- **THEN** the log line names the account or device and the outcome, and does not contain the email address

#### Scenario: The development log transport is the only place a token may appear
- **WHEN** the server runs with the development log transport selected
- **THEN** the message it was given is written to the log, and every other log line still holds no secret

### Requirement: Authentication Error Codes
The server MUST answer every authentication refusal with a stable
machine-readable code, so a client can react to the kind of refusal without
parsing the message text. A validation refusal MUST name the offending field
in a machine-readable field of the error, not only in its message.

#### Scenario: Every refusal carries a stable code
- **WHEN** the server refuses a registration, a sign-in, a resumption, a reset, a verification, an email change, a display-name change, or a rate-limited attempt
- **THEN** the answer carries a code from a fixed set — a taken username, a taken email, refused credentials, no usable session, an expired session, an invalid field, an invalid display name, a rate-limited attempt, a refused reset token, an unverified email address, or a failed mail delivery — and the same refusal always carries the same code

#### Scenario: A validation refusal names the field machine-readably
- **WHEN** the server refuses a field that failed its format check
- **THEN** the error carries the name of that field in a machine-readable field, and a client can mark it without reading the message text

#### Scenario: An oversized password does not consume unbounded work
- **WHEN** a player submits a password far longer than the allowed maximum
- **THEN** the server rejects it without performing the full hashing cost an accepted password would require

#### Scenario: A below-minimum session lifetime is refused at startup
- **WHEN** the server starts with a configured session lifetime shorter than the supported minimum
- **THEN** startup fails with an explicit configuration error rather than running with the weaker value

#### Scenario: Authentication settings come from the environment
- **WHEN** the server starts with no authentication-related environment variables set
- **THEN** it starts successfully using its built-in defaults for the hashing cost and the session lifetime

### Requirement: Pluggable Email Delivery
The server MUST deliver transactional email through a transport selected by
configuration, and MUST be able to deliver it to a log or to an external
provider without changing the account, verification, or reset logic. When no
provider is configured, the transport MUST still deliver the message to the
server log. The log transport MUST write the whole message, including its
link, and MUST be refused at startup in production mode. A failed delivery MUST
NOT undo an account already created.

#### Scenario: The log transport delivers the message
- **WHEN** the log transport is selected and a message is delivered
- **THEN** the message's recipient, subject, and body are written to the server log, and the flow continues as if the delivery had succeeded

#### Scenario: The log transport is refused in production mode
- **WHEN** the server is configured to run in production mode with the log transport selected
- **THEN** the server refuses to start and names the misconfiguration

#### Scenario: A provider transport sends the message
- **WHEN** a provider transport is selected and configured, and a message is delivered
- **THEN** the provider receives the message and accepts it, and the flow continues

#### Scenario: A failed delivery does not undo the registration
- **WHEN** the delivery of a verification message fails after an account was created
- **THEN** the account still exists and can be verified by asking again

#### Scenario: A delivery failure is reported without the token
- **WHEN** a delivery fails
- **THEN** the server logs the failure and the account it concerns, and the log line contains neither the reset nor the verification token
