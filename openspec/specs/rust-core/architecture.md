# Rust Core Architecture Specs

## Overview
The Rust core provides the foundation for the chess application with high-performance move validation, game state management, and player rating calculations.

## Key Components

### 1. Board Management (`domain/board.rs`)
**Responsibility:** Validate moves, manage game state, detect check/checkmate/draw

**APIs Principal:**
- `get_fen()` → devuelve el estado actual del tablero en FEN
- `get_valid_moves(square)` → devuelve movimientos legales desde una casilla
- `play_move(move)` → ejecuta un movimiento si es válido
- `is_check()`, `is_checkmate()`, `is_draw()` → estados del juego
- `get_piece_at(square)` → devuelve la pieza en una casilla específica

**Requirements:**
- Debe usar la biblioteca `shakmaty` para lógica de ajedrez pura
- Debe validar movimientos legales según las reglas estándar del ajedrez
- Debe manejar tanto Jaque como Jaque Mate
- Debe detectar tablas (stalemate, ronda repetida, falta de material)

### 2. Rating System (`domain/rating.rs`)
**Responsibility:** Calcula y actualiza los ratings de jugadores usando Glicko-2

**APIs Principal:**
- `update_player_rating(opponent_rating, result)` → actualiza el rating con un oponente
- `get_current_rating()` → devuelve el rating actual del jugador
- `serialize()` / `deserialize()` → persistencia del rating
- `state()` / `set_state(snapshot)` → expone y restaura el snapshot completo Glicko-2 (rating, desviación, volatilidad) para persistencia del servidor (no parte de la superficie FFI)

**Requirements:**
- Debe implementar Glicko-2 con `glicko2` crate
- Debe manejar rating inicial, desviación e incertidumbre
- Debe actualizar ratings después de cada partida
- Debe mantener el rating persistente entre sesiones

### 3. Game Session (`src/lib.rs`)
**Responsibility:** Coordina el tablero y el sistema de rating en una única sesión de juego

**APIs Principal:**
- `serialize()` → exporta el estado completo del juego
- `deserialize()` → importa el estado del juego
- `get_rating_state()` → expone el snapshot Glicko-2 completo del jugador de esta sesión (no parte de la FFI, para el consumidor en proceso: la persistencia del servidor)
- `new_game_session_from_rating_state(snapshot: RatingSnapshot) -> Arc<GameSession>` → constructor de sesión con el rating restaurado desde un snapshot persistido

### 4. FFI Bindings
**Responsibility:** Expone Rust al Kotlin (Android) y Swift (iOS)

**APIs Principal:**
- `get_board_state()` → devuelve el FEN como string
- `get_valid_moves(square)` → devuelve vector de strings
- `play_move(move)` → ejecuta movimiento
- `update_player_rating(opponent_rating, result)` → actualiza rating
- `get_current_rating()` → devuelve rating actual

### 5. Error Handling
**Responsibility:** Maneja errores de forma consistente en toda la aplicación

**Requirements:**
- Retorna `Result<T, E>` en lugar de usar panics
- Proporciona mensajes de error claros y específicos
- Maneja casos de borde (inputs inválidos, movimiento ilegal, etc.)

## Cross-Language Contracts

The Rust core FFI contracts are:

```rust
// Rust side
pub fn get_board_state() -> String
pub fn get_valid_moves(square: &str) -> Vec<String>
pub fn play_move(move: &str) -> Result<(), &str>
pub fn update_player_rating(opponent_rating: f64, result: f64)
pub fn get_current_rating() -> f64
```

**Kotlin Expectation:**
```kotlin
fun getBoardState(): String
fun getValidMoves(square: String): List<String>
fun playMove(move: String): Result<*, *>
fun updatePlayerRating(opponentRating: Double, result: Double)
fun getCurrentRating(): Double
```

**Swift Expectation:**
```swift
func getBoardState() -> String
func getValidMoves(square: String) -> [String]
func playMove(move: String) -> Result<*, *>
func updatePlayerRating(opponentRating: Double, result: Double)
func getCurrentRating() -> Double
```

## Performance Requirements

- **Move validation**: < 10ms por movimiento legal
- **Rating updates**: < 5ms por cálculo de rating
- **Memory usage**: < 50MB para el estado del juego
- **Thread safety**: Múltiples hilos pueden acceder simultáneamente

## Testing Specifications

### Unit Tests (Rust)
- Validación de movimiento legal para todas las piezas
- Calculo correcto del Jaque/Jaque Mate
- Actualizaciones precisas del sistema Glicko-2
- Manejo correcto de errores

### Integration Tests (Multi-language)
- Contratos FFI entre Rust y Kotlin/Swift
- Transmisión de datos a través del FFI
- Coordinación de builds multi-lenguaje

## Deployment Targets

### iOS
- Soporta iOS 14+
- Arquitectura: arm64 (native)
- Framework: XCFramework `ChessCore.xcframework`

### Android
- Soporta API 24+
- Arquitectura: arm64-v8a, x86_64
- Build: .so libraries (`libchess_core.so`)

## Security Considerations

- No datos sensibles en memoria
- Manejo seguro de errores (sin información de stack traces expuestos)
- Controles de acceso a la API FFI adecuados