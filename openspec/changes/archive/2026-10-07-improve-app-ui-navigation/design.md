# Design

## Context

Hoy `GameScreen` (Android) y `ContentView` (iOS) son la raíz: tablero + `NewGameSetupSheet`/`NewGameSetupView` con chips Register/Login/Logout/Set profile y `GameViewModel` mezclando sesión (`authToken`/`authMode`/`authError`) con partida. Ver `specs/app-navigation/spec.md`. Sin cambios de protocolo (WS v1) ni `ChessCore`.

## Goals / Non-Goals

**Goals:**
- Separar identidad (Login), selección (Home) y partida (Game) con navegación explícita y paridad iOS/Android.
- Reutilizar protocolo y ViewModels existentes, solo reubicando UI y hoisteando estado de sesión.
- Preservar guest por `device_id`, `seated`, relojes y reglas de New game.

**Non-Goals:**
- Sin historial/ranking, sin diseño visual nuevo (solo reestructura con Material3/SwiftUI existente), sin cambios de servidor.

## Decisions

- **D1 — iOS: `NavigationStack` con rutas tipadas (`login`/`home`/`game`) sobre `ContentView` descompuesto en `LoginView`/`HomeView`/`GameView`.** Alternativa: sheets encadenados; se descarta porque oculta el back y mezcla identidad con partida (problema actual).
- **D2 — Android: `NavHost` (`login` start destination, `home`, `game`) con `GameViewModel` de alcance actividad para sesión y estado de juego por destino.** Alternativa: todo en un `GameScreen` con flags; se descarta porque ya colapsó (`isAuth`/`isCpu`/`isOnline`) y no escala.
- **D3 — Estado de sesión hoisteado (`authToken`/`userName`/`authError`) fuera del sheet, guard de rutas (sin token → Login salvo guest explícito).** Reutiliza `doRegister`/`doLogin`/`doLogout`/`doSetProfile` y `onAuthResult` existentes.
- **D4 — Game como destino que recibe modo ya elegido; el sheet "New game" se reduce a crear/unirse dentro de Game o se elimina si Home ya elige modo.** Preserva `canStartNewGame`, single-session y re-attach.
- **D5 — iOS compila sin firma en CI (`xcodebuild -destination simulator` falla hoy solo por signing); Android verifica con `compileDebugKotlin` + `installDebug`.** Sin mobile-mcp como gate (deuda conocida `Agent is not installed`).

## Risks / Trade-offs

- [Back desde Game con partida viva crea doble sesión] → Game scoped al modo elegido; Home no auto-recrea; confirmación explícita de abandono.
- [Logout con re-attach online pendiente] → Logout solo desde Home, no mid-game; la conexión se cierra limpio tras `SessionOk`.
- [Paridad visual limitada] → Misma estructura, componentes nativos por plataforma; sin pixel-perfect cross-platform en este cambio.

## Migration Plan

1. Extraer Login/Home/Game sin borrar el entry actual (feature detrás de navegación, guest default intacto).
2. Mover chips de auth del sheet a Login/Home; verificar guest y `New game` sin regresión.
3. Rollback: revertir destinos y restaurar raíz actual; sin migración de datos (token en memoria).

## Open Questions

- Ninguna que cambie specs o tareas; estilo visual final se define en implementación con componentes existentes.
