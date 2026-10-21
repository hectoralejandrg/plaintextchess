# auth-ui Specification

## Purpose

Permitir que los usuarios de los clientes `iOS` y `Android` se autentiquen opcionalmente (`register`/`login`), cierren sesión (`logout`) y actualicen su perfil (`set_profile`) sin interrumpir el juego de invitado (`device_id`).

## Requirements

### Requirement: Client supports optional authentication messages
The client SHALL send `register`, `login`, `logout`, and `set_profile` messages
over the existing WebSocket endpoint (`wss://<host>/ws` in the TLS deployment,
`ws://` against a local server), using protocol version `1` (`"v":1`). The
messages SHALL follow the wire format defined in the server protocol
(`server/src/interface/protocol.rs`).

#### Scenario: Guest plays without authentication
- **WHEN** a user opens the app and selects an online mode without sending any authentication message
- **THEN** the game plays normally as a guest (`device_id` remains the identity key), the connection stays open, and no `not_authenticated` error is returned

#### Scenario: A client that never authenticates is unaffected by the email messages
- **WHEN** a client built against the current protocol connects and plays a game without sending any authentication or email message
- **THEN** the game plays normally, the connection is not rejected, and the protocol version reported by the server is unchanged

### Requirement: Client can register an account
The client SHALL provide a `register` message that includes `username`,
`email`, and `password`, and SHALL receive a `Session` message containing a
`token` when the account is created.

#### Scenario: Registration succeeds
- **WHEN** the user submits a valid username, a valid email address, and a valid password (8-128 chars) through the client UI
- **THEN** the client sends `register`, the server answers `Session` with a `token`, and the client links the device to the new account for future sessions

#### Scenario: Registration refused (taken username)
- **WHEN** the user submits a username that already exists (case-insensitive)
- **THEN** the client receives `username_taken`, issues no session, and keeps the form open with the username field marked

#### Scenario: Registration refused (taken email)
- **WHEN** the user submits an email address that already exists (case-insensitive)
- **THEN** the client receives `email_taken`, issues no session, and keeps the form open with the email field marked

#### Scenario: Registration refused (malformed field)
- **WHEN** the user submits a username, email address, or password outside the allowed format
- **THEN** the client receives the validation error naming that field, shows it next to the field, and keeps the form open

### Requirement: Client can log in
The client SHALL provide a `login` message that includes the identifier the
player typed and a `password`, and SHALL receive `Session` with a `token` when
the credentials match. The identifier MAY be the account's username or its
email address, and the client SHALL offer a single field for it.

#### Scenario: Login succeeds
- **WHEN** the user submits existing credentials
- **THEN** the client sends `login`, the server answers `Session` with a `token`, and the device is linked to the account

#### Scenario: Login succeeds with an email address
- **WHEN** the user submits an existing account's email address and its password
- **THEN** the client sends `login`, the server answers `Session` with a `token`, and the device is linked to the account

#### Scenario: Login refused
- **WHEN** the user submits wrong or unknown credentials
- **THEN** the client receives `invalid_credentials`, no `token` is issued, the connection remains open for guest play, and the message does not say whether the account exists

### Requirement: Client can log out
The client SHALL provide a `logout` message that includes `token`, and SHALL
receive `SessionOk` when the session is revoked. If the token is unknown or
already revoked, the client SHALL receive `not_authenticated`. Signing out MUST
also discard the session the client stored, so the app comes back signed out.

#### Scenario: Logout succeeds
- **WHEN** the user selects logout while authenticated with a valid `token`
- **THEN** the server revokes the session (`record_session_revoked` writes `revoked_at`) and answers `SessionOk`, and the client discards its stored session

#### Scenario: The app comes back signed out
- **WHEN** the user signs out and then relaunches the app
- **THEN** the app starts as a guest, with no stored session presented

### Requirement: Client can set profile display name
The client SHALL provide a `set_profile` message that includes `token` and `display_name` (trimmed, 1-32 chars, no control characters), and SHALL receive `ProfileUpdated` when the profile is updated. If not authenticated (`token` unknown, empty, revoked, or expired), the client SHALL receive `not_authenticated`. If the name is invalid, the client SHALL receive `invalid_display_name`.

#### Scenario: Profile update succeeds
- **WHEN** an authenticated user submits a valid display name
- **THEN** the server updates the `profiles` table and answers `ProfileUpdated`

#### Scenario: Guest cannot change display name
- **WHEN** a user sends `set_profile` without a valid `token`
- **THEN** the server answers `not_authenticated` and the profile remains unchanged

### Requirement: Existing clients remain unaffected
Clients that never send `register`/`login`/`logout`/`set_profile` SHALL play unchanged. The server SHALL not reject the connection (`PROTOCOL_CLOSE_CODE` 4000 is reserved for malformed JSON, unknown message types, or unsupported versions only), and the protocol version SHALL remain `1`.

#### Scenario: Existing client connects and plays
- **WHEN** an unmodified client (`iOS` or `Android`) connects without authentication messages and creates/joins a room
- **THEN** the game plays normally, the server reports `"v":1`, and the `seated` flag (`handle_socket`) applies correctly: `!seated` keeps the connection open on `None` events, `seated` requires graceful close handshake

### Requirement: Client sends the email-related messages
The client SHALL send the email-related messages (`forgot_password`,
`reset_password`, `change_email`, `verify_email`) in the same protocol
version, and SHALL only offer the flows that the server it is talking to
actually supports.

#### Scenario: The email flows use the same protocol version
- **WHEN** the client sends a password-reset request
- **THEN** the message carries the protocol version the game messages carry
### Requirement: A refused registration is shown on the form
If the username or the email is already taken, the client SHALL receive the
matching error code (`username_taken` or `email_taken`), and if a field is
malformed, the matching validation error, which names the offending field in a
machine-readable field. The client SHALL keep the registration form open after a
refusal, SHALL mark the field the error names, and SHALL keep what the player
typed.

#### Scenario: The form keeps what the player typed
- **WHEN** a registration is refused
- **THEN** the form stays open with the fields the player filled in, and the field the error names is marked
### Requirement: A refused sign-in says nothing about the account
If the identifier is unknown or the password is wrong, the client SHALL receive
`invalid_credentials` with the identical generic message
(`"That username and password do not match an account"`) and no session is
issued.

#### Scenario: The message does not say whether the account exists
- **WHEN** a sign-in is refused for an identifier that belongs to no account
- **THEN** the client shows the same message it would show for an account that exists with a wrong password
### Requirement: Client keeps the session across launches
The client SHALL store the session token in the platform's secure storage — the
system keychain on iOS and encrypted shared preferences on Android — and SHALL
resume the session with it when a connection is opened, so a player who signed
in stays signed in across app restarts. The client SHALL offer a choice to keep
the session. The stored token MUST NOT be readable from ordinary application
storage.

#### Scenario: The player stays signed in after a restart
- **WHEN** a player signed in with the session kept, and the app is closed and relaunched
- **THEN** the client resumes the session with the stored token on the next connection and reports the account it is signed in as

#### Scenario: Declining to keep the session starts as a guest
- **WHEN** a player signs in with the session not kept
- **THEN** the client deletes the stored token, and the next launch starts as a guest

#### Scenario: The stored session is not in ordinary storage
- **WHEN** the client stores a session token
- **THEN** the token is held in the platform's secure storage rather than in plain preferences or a plain file

#### Scenario: A stale stored session falls back to guest play
- **WHEN** the client presents a stored token the server no longer accepts
- **THEN** the client discards it and continues as a guest, and the player can sign in again

#### Scenario: The session is resumed only against a server that supports it
- **WHEN** the client restores a stored session against a server that does not report the resumption feature
- **THEN** the client does not ask for a resumption, the connection is not spent on a frame the server would refuse, and the player signs in from the sign-in screen
### Requirement: Client offers password reset
The client SHALL offer a "forgot password" path that asks for the email
address, sends a `forgot_password` message, and shows a confirmation that does
not say whether the address is registered. When the player receives a reset
token — through a link the app opens, or by pasting the code — the client SHALL
send `reset_password` with the token and the new password, and SHALL ask for
the new password twice. A successful reset SHALL NOT sign the player in.

#### Scenario: The reset request is accepted without revealing anything
- **WHEN** the user asks for a reset link with an address that belongs to no account
- **THEN** the client shows the same confirmation it shows for a registered address, and no error about the address appears

#### Scenario: The player sets a new password
- **WHEN** the user submits a valid reset token and a new password twice
- **THEN** the client sends `reset_password`, reports that the password was changed, and does not sign the player in

#### Scenario: An invalid token is reported
- **WHEN** the user submits a token that is expired or already used
- **THEN** the client shows the error the server sent and offers to ask for a new reset link

#### Scenario: A reset link opens the app
- **WHEN** the player follows a reset link that the operating system routes to the app
- **THEN** the app opens on the reset screen with the token already filled in
### Requirement: Client reports the email verification state
The client SHALL show whether the signed-in account's email address is
verified, and SHALL offer to send another verification message while it is not.
The client SHALL NOT let an unverified account block play: every game feature
remains available.

#### Scenario: An unverified account is told so
- **WHEN** the client signs in to an account whose email address is not verified
- **THEN** the client shows that the address is unverified and offers to send another verification message

#### Scenario: An unverified account can still play
- **WHEN** a player whose account is unverified creates a room and plays
- **THEN** the game plays normally, and no feature is withheld because of the verification state

#### Scenario: The verification state refreshes
- **WHEN** the player confirms the address and signs in again
- **THEN** the client reports the account as verified

#### Scenario: A confirmation link opens the app
- **WHEN** the player follows a confirmation link that the operating system routes to the app
- **THEN** the app sends the token it carries, and the account is reported as confirmed
### Requirement: Client never displays the session token
The client SHALL NOT show the session token, in whole or in part, in any
screen, and SHALL NOT offer a way to copy it. The signed-in state SHALL be
presented as the account's name and its verification state.

#### Scenario: The signed-in screen shows no token
- **WHEN** the player opens the signed-in view
- **THEN** the view shows the account's name and verification state, and shows no token and no control that reveals one
### Requirement: Client degrades when the server lacks a feature
Before offering a feature that depends on a newer server, the client SHALL read
the server's readiness information over HTTPS at the same address as the game
connection, with a short timeout, and learn which features the server supports
from the feature list it reports. When the request fails or times out, the
client SHALL assume the server supports no optional feature. A missing feature
SHALL hide that path, and the flows the server does support SHALL keep
working.

#### Scenario: The reset path is hidden on an older server
- **WHEN** the client connects to a server that does not advertise the reset feature
- **THEN** the "forgot password" path is not offered, and signing in and guest play still work

#### Scenario: The features the server does support keep working
- **WHEN** the client talks to a server that supports a subset of the features
- **THEN** every supported flow behaves normally, and only the unsupported ones are unavailable
### Requirement: Client presents authentication errors inline
The client SHALL present an authentication failure next to the form that caused
it, naming the field when the server named one, and SHALL keep the form open
with the player's input intact so it can be corrected and resubmitted. The
client SHALL NOT present an authentication failure as a fatal error, and SHALL
leave the connection usable for guest play.

#### Scenario: A refused sign-in keeps what was typed
- **WHEN** the user submits credentials that the server refuses
- **THEN** the error appears next to the form, the identifier stays filled in, the password field is cleared, and the connection stays usable

#### Scenario: A rate-limited attempt says when to retry
- **WHEN** the server refuses an attempt because of a rate limit
- **THEN** the client shows that the attempt was rate limited and, when the server said so, when the player may try again
### Requirement: Client offers guest play without an account
The client SHALL offer to play as a guest from the sign-in screen, without
requiring an account, and MUST NOT make an account a precondition for playing.

#### Scenario: A player reaches the game without an account
- **WHEN** the user chooses guest play on the sign-in screen
- **THEN** the app proceeds to the game as a guest, with no account created and no credential asked for
### Requirement: Client offers only what the server supports
The client SHALL NOT offer a registration that asks for an email address unless
the server reports that it supports the email feature, and SHALL NOT send a
field the server would ignore. A server that does not report the feature MUST
be offered only the registration it does support.

#### Scenario: A server without the email feature is offered no email registration
- **WHEN** the client connects to a server that does not report the email feature
- **THEN** the registration form does not ask for an email address, and the client sends no field the server would ignore
