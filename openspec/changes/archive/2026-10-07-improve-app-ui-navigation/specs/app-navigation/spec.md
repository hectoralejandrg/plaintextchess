# Spec Delta

## Purpose

Define una estructura de navegación Login → Home → Juego compartida por los clientes iOS y Android, separando identidad, selección de modo y partida sin cambiar reglas ni protocolo.

## ADDED Requirements

### Requirement: Login screen as app entry

The app SHALL present a Login screen as the initial destination with `username` and `password` fields, Register and Login actions, a "Play as guest" entry, and a visible auth error area.

#### Scenario: Guest entry preserved

- **WHEN** the user taps "Play as guest" without credentials
- **THEN** the app navigates to Home in guest mode using the existing `device_id` identity and no token is required

#### Scenario: Failed login shows generic message

- **WHEN** the server answers `invalid_credentials` or `username_taken` as applicable
- **THEN** the Login screen shows the generic auth error without revealing which field failed and stays on Login

### Requirement: Home screen with modes, profile and session state

The app SHALL present a Home screen after Login or guest entry showing the session state (guest vs authenticated user), mode selection (two players, CPU with difficulty, online create/join with time control), profile editing (`display_name`) and Logout.

#### Scenario: Authenticated home shows session actions

- **WHEN** the user arrives at Home with a valid session token
- **THEN** Home shows the username/display name, offers Edit profile and Logout, and mode selection remains available

#### Scenario: Guest home hides token actions

- **WHEN** the user arrives at Home as guest
- **THEN** Home shows guest state, offers Login/Register navigation, and does NOT offer Logout or profile update

### Requirement: Dedicated Game section preserves current behavior

The app SHALL present a dedicated Game section containing the existing board, status row, clocks, move list, controls (Undo/Resign/Flip), New game flow and game-end dialog with behavior unchanged from the current clients.

#### Scenario: Game rules unchanged from Home launch

- **WHEN** the user starts any mode from Home
- **THEN** the Game section enforces the existing availability rules (New game disabled mid-game, single active session, CPU/online semantics) exactly as today

### Requirement: Explicit Login-Home-Game navigation

The app SHALL implement explicit navigation between Login, Home and Game with predictable back behavior: back from Game returns to Home without destroying an in-progress game state unexpectedly, and Logout returns to Login clearing the token while leaving guest play available.

#### Scenario: Back from game preserves context

- **WHEN** the user navigates back from an in-progress Game to Home
- **THEN** the game screen state is preserved or explicitly abandoned by user action, and Home does not start a second concurrent session

#### Scenario: Logout clears session and returns to Login

- **WHEN** the user confirms Logout from Home
- **THEN** the app revokes/clears the token, shows Login again, and guest entry remains available
