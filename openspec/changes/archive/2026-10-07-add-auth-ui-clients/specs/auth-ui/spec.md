# Spec Delta

## Purpose

Permitir que los usuarios de los clientes `iOS` y `Android` se autentiquen opcionalmente (`register`/`login`), cierren sesión (`logout`) y actualicen su perfil (`set_profile`) sin interrumpir el juego de invitado (`device_id`).

## ADDED Requirements

### Requirement: Client supports optional authentication messages
The client SHALL send `register`, `login`, `logout`, and `set_profile` messages over the existing WebSocket (`ws://.../ws`) using protocol version `1` (`"v":1`). The messages SHALL follow the wire format defined in the server protocol (`server/src/interface/protocol.rs`).

#### Scenario: Guest plays without authentication
- **WHEN** a user opens the app and selects an online mode without sending any authentication message
- **THEN** the game plays normally as a guest (`device_id` remains the identity key), the connection stays open, and no `not_authenticated` error is returned

### Requirement: Client can register an account
The client SHALL provide a `register` message that includes `username` and `password`, and SHALL receive a `Session` message containing a `token` when the username is available. If the username is taken, the client SHALL receive `username_taken` with the same error code stability as the server (`ErrorCode::UsernameTaken`).

#### Scenario: Registration succeeds
- **WHEN** user submits a unique username and a valid password (8-128 chars) through the client UI
- **THEN** the client sends `register`, the server answers `Session` with a `token`, and the client links the device to the new account for future sessions

#### Scenario: Registration refused (taken username)
- **WHEN** user submits a username that already exists (`username_taken` in the `accounts` table, case-insensitive `username_key`)
- **THEN** the client receives `username_taken` and does not issue a session; the connection stays open for guest play

### Requirement: Client can log in
The client SHALL provide a `login` message that includes `username` and `password`, and SHALL receive `Session` with a `token` when the credentials match. If the username is unknown or the password is wrong, the client SHALL receive `invalid_credentials` with the identical generic message (`"That username and password do not match an account"`) and no session is issued.

#### Scenario: Login succeeds
- **WHEN** user submits existing credentials
- **THEN** the client sends `login`, the server answers `Session` with a `token`, and the device is linked to the account

#### Scenario: Login refused
- **WHEN** user submits wrong or unknown credentials
- **THEN** the client receives `invalid_credentials`, no `token` is issued, and the connection remains open for guest play

### Requirement: Client can log out
The client SHALL provide a `logout` message that includes `token`, and SHALL receive `SessionOk` when the session is revoked. If the token is unknown or already revoked, the client SHALL receive `not_authenticated`.

#### Scenario: Logout succeeds
- **WHEN** user selects logout while authenticated with a valid `token`
- **THEN** the server revokes the session (`record_session_revoked` writes `revoked_at`) and answers `SessionOk`

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
