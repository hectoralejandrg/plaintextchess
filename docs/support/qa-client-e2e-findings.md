# QA — E2E de clientes reales (tarea 5.1) — hallazgos

Fecha: 2026-10-09
Alcance: verificación de la tarea **5.1** del cambio `split-realtime-architecture`
(E2E de la arquitectura tiered con clientes iOS/Android reales). El cambio está
archivado; estas notas son para **correcciones posteriores** (harness de QA).

## Entorno usado
- Tiers split locales: Redis propio (16379) + `TIER=logic` (healthz 18792) +
  `TIER=transport` (8765), `RECONNECT_GRACE_SECS=45`.
- iOS: Xcode 27, simuladores `iPhone 17 Pro` y `iPhone Air Test` (booted).
- Android: SDK + AVD `Pixel_6` (`emulator-5554`), JDK 17.
- Hooks E2E de las apps:
  - iOS: `-PLAINTCHESS_ONLINE_URL <url>`, `-PLAINTCHESS_SCRIPT "onlinecreate|onlinejoin:<code> <uci> ..."`.
  - Android: `--es online_url ws://10.0.2.2:8765/ws` (sin hook headless de create/join).

## Resultado (núcleo): PASS
- Cliente iOS #1 (creador) conectó por el **transporte split** y creó la sala
  (`ptc.room_owner.<code>` + `ptc.conn.*` en Redis).
- Cliente iOS #2 (joiner) conectó y **se unió** a la misma sala.
- **Ambas apps mostraron la partida online sincronizada** (oponente 1.500,
  relojes, tablero). **Sin warnings/errores en los tiers** y **sin crash**.
- Android: `./gradlew :app:assembleDebug` OK, instala y arranca sin crash.

Esto confirma lo esencial de 5.1: **la arquitectura split sirve clientes móviles
reales** (create + join + partida sincronizada), sin regresión de wire protocol.

## Hallazgos a corregir

### E1 — Las jugadas de `-PLAINTCHESS_SCRIPT` no se aplicaron
- **Observado:** ambas apps quedaron en la posición inicial ("No moves yet") tras
  `onlinecreate e2e4` (creador) y `onlinejoin:<code> e7e5` (joiner).
- **Causa:** el token de jugada se dispara ~1 s tras la creación/unión; en el
  creador, **antes de que el rival entrara** (la partida no está "playing"), y en
  el joiner, en **turno equivocado**. Es **secuencia del harness de QA**, no un
  defecto de producto (los tests `split_e2e` a nivel WS ya prueban el relé de
  jugadas).
- **Corrección sugerida:** encolar las jugadas **después** de confirmar que ambos
  jugadores están sentados (token de espera / polling de estado), o ejecutar las
  jugadas en un segundo relanzamiento con la partida ya en curso.

### E2 — Reconexión por relanzamiento no automatizable headless
- **Observado:** al relanzar el creador, la app cae en la **pantalla de auth**
  (Username/Password/Registrar/Login/Play as guest); no recupera la sala sola.
- **Causa:** la recuperación (`UserDefaults "plaintextchess.online_room_code"`)
  está **detrás de entrar como guest** (un tap); y sin `-PLAINTCHESS_ONLINE_URL`
  no hay servidor al que reconectar (la URL es debug-injected, no persistida).
- **Corrección sugerida:** hook de debug que entre en guest y recupere la sala
  automáticamente, o cubrir con **XCUITest** (UI automation).

### E3 — Android sin hook headless de create/join
- **Observado:** Android solo acepta `online_url`; no hay extra para crear/unir.
- **Corrección sugerida:** añadir `--es online_create true` / `--es online_join <code>`
  (debug-only) o cubrir con **Espresso**.

### E4 — "Browse history" no ejercitado
- **Observación:** el guion de 5.1 menciona "browse history"; no se probó.
- **Corrección sugerida:** incluirlo en el guion de release.

## Reproducción (para retomar)
1. Levantar Redis propio + `TIER=logic` + `TIER=transport` (8765).
2. iOS: `xcrun simctl launch <sim> com.hectoralejandrg.plaintextchess \
   -PLAINTCHESS_ONLINE_URL ws://127.0.0.1:8765/ws -PLAINTCHESS_SCRIPT "onlinecreate"`.
3. Leer el código de sala de
   `xcrun simctl get_app_container <sim> com.hectoralejandrg.plaintextchess data`
   → `Library/Preferences/com.hectoralejandrg.plaintextchess.plist`
   (clave `plaintextchess.online_room_code`).
4. Segundo simulador: `-PLAINTCHESS_SCRIPT "onlinejoin:<code>"`.
5. Verificar en Redis `ptc.room_owner.<code>` y `ptc.conn.*`; capturar pantallas.

## Conclusión
El **núcleo de 5.1 pasa**; la **automatización headless completa no está** (E1–E4).
5.1 permanece como **gate manual de release**, con estas correcciones pendientes
(todas de **harness de QA**, ninguna de producto).
