# Proposal

## Why

El cambio `add-player-login-sessions` (archivado) implementó la autenticación opcional (`register`/`login`/`logout`/`set_profile`) en el servidor (`ws.rs`) y en la base de datos (`accounts`/`profiles`/`sessions`), con el protocolo de mensajes WS correspondiente (`VERSION = 1`). Sin embargo, los clientes actuales (`iOS`/`Android`) no conocen estos mensajes ni ofrecen una UI para autenticarse. Esto deja la funcionalidad inaccesible para los usuarios que desean crear cuentas. Además, la prueba con `mobile-mcp` (`iPhone 17 Pro`) mostró que el botón `New game` existe pero no hay navegación hacia autenticación, confirmando la brecha.

## What Changes

- Agregar la UI de autenticación (`register`, `login`, `logout`, `set_profile`) en los clientes `iOS` y `Android`.
- Integrar con el protocolo WS existente (`VERSION = 1`, mensajes `register`/`login`/`logout`/`set_profile`).
- No modificar el comportamiento de invitado (`device_id`): los clientes que nunca se autentiquen deben funcionar sin cambios.
- Documentar el flujo de autenticación para los desarrolladores de clientes.

**BREAKING**: No es un cambio rompedero en el sentido del protocolo (los mensajes son nuevos, no reemplazan existentes), pero introduce nuevas pantallas/navegación en los clientes.

## Capabilities

### New Capabilities
- `auth-ui`: Interfaz de usuario para autenticación opcional en clientes `iOS`/`Android`. Cubre los mensajes `register`, `login`, `logout`, `set_profile` sobre el protocolo WS existente.

### Modified Capabilities
- `player-auth`: No se modifica el requisito del servidor (ya implementado y archivado), pero este cambio extiende su alcance a los clientes, haciendo que la capacidad sea usable end-to-end.

## Impact

- Clientes `iOS` (`ios/app/PlainTextChess/`) y `Android` (`android/app/src/main/java/com/hectoralejandrg/plaintextchess/`).
- Protocolo `OnlineProtocol.swift` / `OnlineProtocol.kt`: agregar los nuevos mensajes de autenticación (`register`, `login`, `logout`, `set_profile`).
- `docs/admin-guide.md` (opcional): actualizar la sección de autenticación con instrucciones para clientes.
- Pruebas con `mobile-mcp`: permitirá verificar la interacción completa (desde `New game` → `Online` → `Login`/`Register`).
