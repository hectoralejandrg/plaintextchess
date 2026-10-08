# Proposal

## Why

La app abre directo en el tablero con la autenticación metida dentro del sheet "New game" (chips Register/Login/Logout/Set profile). No hay una estructura clara de Login → Home → Juego, lo que confunde al usuario y mezcla identidad con creación de partida.

## What Changes

- Nueva pantalla inicial de Login con campos `username`/`password`, acciones Register/Login, entrada "Jugar como invitado" y mensaje de error genérico.
- Nueva pantalla Home post-identidad (o invitado) con: jugar (2 jugadores, CPU, online crear/unirse), ver/editar perfil (`display_name`), logout y estado de sesión visible.
- Nueva sección Juego dedicada que conserva el tablero, relojes, lista de jugadas y controles actuales sin cambios de reglas.
- Navegación explícita Login ↔ Home ↔ Juego en iOS (`SwiftUI NavigationStack`) y Android (`Compose NavHost`), con back/forward predecible y sin romper partida en curso.
- Sin cambios de protocolo: se reutilizan mensajes WS existentes (`register`/`login`/`logout`/`set_profile`, versión 1) y el juego por `device_id` (invitado) sigue intacto.

## Capabilities

### New Capabilities

- `app-navigation`: estructura de navegación Login/Home/Juego compartida por ambos clientes: entry de identidad, home de modos y sección de juego, con reglas de guardia de sesión y preservación de partida.

### Modified Capabilities

- Ninguna. Los requisitos de `android` e `ios` sobre tablero, modos, relojes y controles no cambian; solo se reubica dónde se invoca la autenticación y la creación de partida.

## Impact

- Código afectado: `ios/app/PlainTextChess/ContentView.swift` (+ nueva `LoginView`, `HomeView`, `GameView` o refactor equivalente), `android/.../MainActivity.kt` (+ destinos `login`/`home`/`game`), `GameViewModel` en ambas plataformas (estado de sesión/identidad separado del estado de partida).
- Sin impacto en servidor, protocolo WS ni `ChessCore` (UniFFI).
- Supuestos (porque no se pudo confirmar con el usuario tras el reinicio): aplica a iOS y Android con paridad; login opcional con guest preservado; Home incluye jugar + perfil + logout (sin historial/ranking en este cambio).
