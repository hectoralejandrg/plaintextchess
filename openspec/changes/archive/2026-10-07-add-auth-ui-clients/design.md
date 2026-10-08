# Design

## Context

El cambio `add-player-login-sessions` (archivado) implementó la autenticación opcional en el servidor (`ws.rs`, `Config`, `accounts`/`profiles`/`sessions`) y el protocolo WS (`VERSION = 1`, mensajes `register`/`login`/`logout`/`set_profile`). Los clientes (`iOS`: `OnlineProtocol.swift`, `GameViewModel.swift`; `Android`: `OnlineProtocol.kt`, `GameViewModel.kt`) no fueron modificados en ese cambio (`spec.md` del cambio archivado confirma: `No client (iOS/Android) code or spec changes`). El `seated` flag (`D15`) ya está documentado en el diseño del cambio anterior. La verificación con `mobilecli` confirma que el simulador `iPhone 17 Pro` (`D610A679...`) funciona con `tap` y `screenshot`, pero la inspección del árbol de accesibilidad requiere el componente `agent` de `mobile-mcp` (`Agent is not installed on the device`), documentado como deuda técnica (`tasks.md` del cambio anterior).

Ver `proposal.md` para la motivación (por qué se necesita la UI en los clientes ahora que el servidor la soporta).

## Goals / Non-Goals

**Goals:**
- Agregar pantallas/navegación de autenticación (`register`, `login`, `logout`, `set_profile`) en los clientes `iOS` y `Android` sin romper el juego de invitado (`device_id`).
- Reutilizar los mensajes WS del protocolo existente (no crear nuevos códigos de protocolo).
- Mantener `Board Layout Stability`: la UI debe funcionar con los layouts existentes del tablero (`BoardView`, `FEN`).

**Non-Goals:**
- No modificar el servidor (`ws.rs`, `Config`, `auth`): el protocolo y la lógica de autenticación del servidor ya están archivados.
- No agregar nuevas pantallas de juego (solo autenticación: registro/login/logout/perfil).
- No migrar datos de `device_id` a `account_id` (el cambio anterior dejó esto como `Non-Goals` y `D14` en su diseño).
- No implementar `rate limiting` de login (mencionado en el contexto del cambio anterior como riesgo aceptado).

## Decisions

### D1: Usar el protocolo WS existente (`VERSION = 1`)
**Rationale**: El cambio `add-player-login-sessions` definió los mensajes de autenticación (`register`, `login`, `logout`, `set_profile`) sobre el protocolo existente (`VERSION = 1`). No es necesario crear un nuevo protocolo (`VERSION = 2`) ni modificar los códigos de error (`ErrorCode`).
**Alternatives**: Crear un protocolo separado (`VERSION = 2`) para auth — rechazada porque aumenta la complejidad del cliente sin beneficio observable para el usuario.

### D2: No modificar `device_id` como clave de identidad
**Rationale**: El cambio anterior (`add-player-login-sessions`) mantuvo `device_id` como la clave de identidad para los invitados y agregó `account_id` para los autenticados (`Identity` con `Guest`/`Account`). El cliente debe seguir usando `device_id` para los invitados y enviar `token` para los autenticados, sin cambiar la clave de calificación/partida.
**Alternatives**: Reemplazar `device_id` por `account_id` para todo — rechazada porque rompe la compatibilidad con clientes existentes (`Board Layout Stability` y `Existing clients are unaffected`).

### D3: `seated` flag (`D15`) aplicable a los clientes
**Rationale**: El `seated` flag (`handle_socket` en `ws.rs`) distingue entre conexiones que nunca vieron `room_ready` (`!seated`, conexión abierta sin handshake) y las que sí (`seated`, requiere handshake). Los clientes que autentican a mitad de una conexión (`login` después de `join_room`) deben respetar este comportamiento: si la conexión estaba `!seated` y luego se autentica, el `seated` pasa a `true` tras `room_ready`/`state`, y el cierre debe ser gracioso.
**Alternatives**: Ignorar `seated` en los clientes — rechazada porque el cambio anterior (`add-player-login-sessions`) documentó explícitamente que `seated` restaura la conexión abierta (`"The requirement the seated flag restores"`).

### D4: `mobile-mcp` (`mobile-next/mobile-mcp`) para verificación de UI
**Rationale**: El `mobilecli` (`npm install -g @mobilenext/mobilecli@latest`) funciona (`tap`, `launch`, `screenshot`), pero `mobile-mcp` requiere el componente `agent` (`Agent is not installed on the device`) para la inspección del árbol de accesibilidad (`mobile_list_elements_on_screen`). Esto debe resolverse antes de que los tests automatizados de UI (`online` con `register`/`login`) puedan verificarse completamente.
**Alternatives**: Usar `simctl` con `AppleScript` o `idb` — rechazada porque `AppleScript` falló (`process "Simulator"` no encontrado) y `idb` no está instalado. La solución recomendada es instalar/configurar el `agent` del `mobile-mcp` (`.claude-plugin` o `mcp.json`).

### D5: No agregar cambios en los specs del cambio anterior
**Rationale**: El cambio `add-player-login-sessions` (archivado) ya cubre los requisitos del protocolo (`VERSION = 1`, `register`/`login`/`logout`/`set_profile`). Este cambio (`add-auth-ui-clients`) es una extensión de la capacidad `auth-ui` (o `player-auth`), no una modificación de los requisitos del servidor.
**Alternatives**: Modificar `specs/player-auth/spec.md` del cambio anterior — rechazada porque los requisitos del servidor no cambian; solo se agrega la capa de UI en los clientes.

## Risks / Trade-offs

- [Falta del `agent` de `mobile-mcp`] → Mitigación: documentar en `tasks.md` (ya hecho) y resolver con `.claude-plugin` / `mcp.json` del repositorio `mobile-next/mobile-mcp`.
- [Clientes sin UI de auth hoy] → Mitigación: agregar pantallas/navegación sin romper `New game` (`online`) ni el juego de invitado (`device_id`).
- [No hay cambios en los clientes hoy] → Mitigación: este cambio cubre exactamente esa brecha.
- [No hay `skip_specs`] → Este cambio declara una nueva capacidad (`auth-ui`) y requiere `spec.md` (`add-auth-ui-clients/specs/auth-ui/spec.md`).

## Migration Plan

- Fase 1: Implementar `register`/`login` (`iOS`/`Android`) — agregar mensajes en `OnlineProtocol` (`OnlineConnectionManager`).
- Fase 2: Implementar `logout`/`set_profile`.
- Fase 3: Verificar que los clientes existentes (`New game` → `Online`) siguen funcionando sin cambios (`device_id` intacto).
- Fase 4: Resolver `agent` de `mobile-mcp` para verificación automatizada de UI.

## Open Questions

- ¿Se requiere un nuevo diseño para la navegación entre `New game` y la pantalla de autenticación? (No cambia los specs, puede resolverse en la implementación).
- ¿El `agent` de `mobile-mcp` debe ser parte del repositorio (`.claude-plugin`) o configurado externamente? (No cambia los specs del cambio; documentado en `tasks.md` del cambio anterior como deuda técnica).
