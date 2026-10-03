# Android UI Specifications

## Overview
This document defines the user interface specifications for the Android chess application, built with Jetpack Compose.

## Implementation Status

Implemented (current milestone):

- 8x8 board rendered from the core's board state, with in-board file/rank
  coordinates and the bundled cburnett piece set
- Two-tap selection: tap a piece to highlight its core-driven legal
  destinations, tap a highlighted square to play the move
- **Piece drag and drop**: a board-level gesture (`pointerInput` +
  `detectDragGestures`) that coexists with the per-square `clickable` tap
  path; lift/drop drive the same `select` intent as taps
- **Pawn promotion**: when a pawn's destination is on the last rank, an inline
  picker card (queen/rook/bishop/knight, tap targets ≥ 48 dp) is shown over
  the destination square; tapping outside the card cancels the move
- **Move animation**: a 0.2 s slide of the moved piece from origin to
  destination; skipped when `ANIMATOR_DURATION_SCALE` is 0 (the Android
  equivalent of Reduce Motion)
- Status row (to move / check / checkmate / draw), UCI move list, and the
  **New game** action

Still aspirational (described above, not implemented): timers, ratings,
CPU opponent, game result dialog, long-press context menu, themes/dark mode,
localization, and TalkBack announcements.

## Key Components

### 1. Chess Board Component (`ChessBoardComposable`)
**Responsibility:** Renderiza el tablero de ajedrez 8x8 y maneja la interacción del usuario

**Features:**
- Renderizado de cuadrícula 8x8 con Material Design
- Animaciones fluidas con Compose
- Highlight de movimiento legal en tap/hover
- Soporte para efecto material elevation en piezas

**APIs Principal:**
```kotlin
@Composable
fun ChessBoardComposable(
    boardState: State<String>,
    selectedSquare: MutableState<String?> = mutableStateOf(null),
    validMoves: MutableState<List<String>> = mutableStateOf(emptyList()),
    onMoveSelected: (String, String) -> Unit
)
```

### 2. Chess Piece Component (`ChessPieceComposable`)
**Responsibility:** Renderiza piezas de ajedrez individuales con animaciones

**Features:**
- Dibujo vectorial con PainterImage
- Animaciones smooth con transition
- Efectos de material elevation
- Soporte para drag and drop

### 3. Legal Moves Overlay (`LegalMovesOverlay`)
**Responsibility:** Muestra movimientos legales disponibles con indicadores visuales

**Features:**
- Overlay circular para movimiento en L del caballo
- Flechas para movimientos de piezas
- Indicadores de ruta para movimientos
- Apariencia consistente con Material Design

### 4. Game Controls (`GameControls`)
**Responsibility:** Maneja el flujo del juego, estado y controles globales

**Features:**
- Timer para ambos jugadores (opcional)
- Botones Material (floating action button, buttons)
- Display del rating del jugador actual
- Diálogo de configuración de oponente

### 5. Game Result Dialog (`GameResultDialog`)
**Responsibility:** Muestra resultados de partidas y actualiza ratings

**Features:**
- AlertDialog con Material Design
- Animaciones de entrada/salida con Compose
- Mostrar rating actualizado
- Botones de acción para compartir, guardar, jugar de nuevo

## Gesture Handling

### Tap Gestures
- **Tap en casilla vacía**: Selecciona pieza
- **Tap en pieza**: Selecciona pieza
- **Tap en movimiento legal**: Ejecuta movimiento
- **Tap en casilla con pieza enemiga**: Captura (después de selección)

### Long Press
- **Long press en pieza**: Muestra menú contextual

### Drag and Drop
- **Drag de pieza**: Inicia operación de drag and drop
- **Drop en casilla legal**: Ejecuta movimiento

## Compose State Management

### State Hoists
```kotlin
@Composable
fun ChessGameScreen() {
    val gameState = rememberSaveableStateHolder { ChessGameState() }
    
    ChessBoardComposable(
        boardState = gameState.boardState,
        selectedSquare = gameState.selectedSquare,
        validMoves = gameState.validMoves,
        onMoveSelected = { from, to -> gameState.makeMove(from, to) }
    )
}
```

### Effect Handlers
```kotlin
@Composable
fun GameEffects(gameState: ChessGameState) {
    SideEffect {
        // Update board when game state changes
        if (gameState.gameStatus.isGameOver()) {
            showGameResultDialog(gameState)
        }
    }
}
```

## Material Design Specifications

### Colors
```kotlin
object ChessColors {
    val lightSquare = Color(0xFFEBEE)  // Light red 50
    val darkSquare = Color(0x3E2723)   // Brown 900
    val selectedSquare = Color(0x4DD0E1)  // Cyan 300
    val validMoveSquare = Color(0x42A5F5) // Blue 400
    val capturedPiece = Color(0xEF5350)  // Red 400
}
```

### Typography (Material Typography)
- **Título (Headline):** MaterialTypography.h4
- **Cuerpo (Body):** MaterialTypography.body1
- **Pequeño (Caption):** MaterialTypography.caption

### Iconography
- **Iconos:** Material Design Icons
- **Icons personalizadas:** Vector drawables para piezas

## Accessibility (Android)

### TalkBack
- **Anuncios:** Cada casilla debe ser announced
- **Texto descriptivo:** Para cada pieza
- **Anuncios de estado:** Para Jaque, Jaque Mate

### View Structure
- **Semantic navigation:** Estructura de vistas apropiada
- **Content descriptions:** Para todas las imágenes
- **Focus handling:** Manejo apropiado de navegación

## Compose Performance

### Performance Optimizations
- **Lazy columns** para la cuadrícula
- **Key transformations** para animaciones
- **State hoisting** para minimzar recompilaciones

### Performance Targets
- **Target 60 FPS** en dispositivos de 60Hz
- **Frame time:** < 16ms por frame
- **Memory usage:** < 200MB para la app completa

## Animation Specifications

### Transition Types
- **Fade transition:** for showing/hiding components
- **Scale transition:** for dragging pieces
- **Slide transition:** for dialogs

### Spring Animations
```kotlin
val smoothEasing = spring(
    dampingRatio = Spring.DampingRatioMediumBouncy,
    stiffness = Spring.StiffnessMediumLow
)
animate(
    targetValue = offset,
    transition = smoothEasing
)
```

## Localization

### Supported Languages
- **Inglés:** (predeterminado)
- **Español:** (es)
- **Francés:** (fr)
- **Alemán:** (de)
- **Chino simplificado:** (zh-Hans)

### Layout Direction
- **LTR:** Para idiomas occidentales
- **RTL:** Para idiomas que se leen de derecha a izquierda

## Testing Specifications

### Jetpack Compose Test
- **Composable assertions:** probar cada componente
- **Gesture testing:** simular taps, drags, gestures
- **Animation testing:** verificar timings de animación
- **Accessibility testing:** verificar compatibilidad con TalkBack

### Integration Testing
- **UI Integration:** flujo de juego completo
- **Data Integration:** con Rust backend a través de FFI
- **Device Compatibility:** una variedad de dispositivos Android

## Material Design Components

### Theme
```kotlin
val ChessTheme = MaterialTheme(
    colors = ChessColors,
    typography = ChessTypography,
    shapes = ChessShapes
) {
    // Composables
}
```

### Components
- **Buttons:** MaterialButton, FloatingActionButton
- **Cards:** GameCard, RatingCard
- **Dialogs:** AlertDialog, GameResultDialog
- **Sheets:** GameSettingsSheet

## Dark Theme Support

### Compose Dark Theme
```kotlin
val darkColors = darkColors(
    background = Color(0x121212),
    surface = Color(0x1E1E1E),
    onBackground = Color(0xE0E0E0)
)
```

### Dynamic Color
- **Seguir la configuración del sistema:** usar `isSystemInDarkTheme()`
- **Toggle manual:** Guardar preferencia en SharedPreferences

## Performance Monitoring

### Compose Debugging
- **Composition local:** Depuración del árbol de composición
- **Visual debugging:** Mostrar límites de componentes
- **Animation timeline:** Ver cronograma de animaciones

## Security & Privacy

### Permissions
- **STORAGE:** Opcional para guardar imágenes de partidas
- **NETWORK:** Para guardar rating en la nube (si aplica)
- **BACKGROUND_EXECUTION:** Para movimiento automático del oponente CPU

## File Structure

```
android/
├── app/
│   ├── src/main/
│   │   ├── java/com/example/chess/
│   │   ├── res/
│   │   │   ├── drawable/    # Imágenes de piezas
│   │   │   ├── layout/      # XML layouts (si aplica)
│   │   ├── mipmap/         # Iconos de app
│   │   └── kotlin/
│   │       ├── ui/         # Composable screens
│   │       ├── viewmodel/  # Game viewmodels
│   │       └── model/      # Data classes
│   └── AndroidManifest.xml
```