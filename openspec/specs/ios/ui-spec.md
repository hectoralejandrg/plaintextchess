# iOS UI Specifications

## Overview
This document defines the user interface specifications for the iOS chess application, built with SwiftUI.

## Key Components

### 1. Chess Board View (`BoardView`)
**Responsibility:** Renderiza el tablero de ajedrez 8x8 e maneja la interacción del usuario

**Features:**
- Renderizado de cuadrícula 8x8 con celdas oscuras/claras
- Posicionamiento de piezas con imágenes vectoriales escalables
- Highlight de movimiento legal en hover/tap
- Animaciones suaves para movimientos y efectos de piezas

**APIs Principal:**
```swift
struct BoardView: View {
    @Binding var boardState: String
    @Binding var selectedSquare: String?
    @Binding var validMoves: [String]
    var body: some View
}
```

### 2. Chess Piece View (`PieceView`)
**Responsibility:** Renderiza piezas de ajedrez individuales con animaciones

**Features:**
- Imágenes SVG de piezas para cada color
- Animaciones de entrada/salida para movimientos
- Rotación y transformación según el color de la pieza
- Soporte para piezas arrastradas

### 3. Move Selection View (`MoveSelectionView`)
**Responsibility:** Muestra movimientos legales y maneja la selección de usuario

**Features:**
- Grid de movimiento legal con casillas resaltadas
- Mostrar flechas para movimientos en L (caballo)
- Confirmación de movimiento con casilla destino
- Indicador de accesibilidad con pronunciación de pantalla

### 4. Game Control View (`GameControlView`)
**Responsibility:** Maneja el flujo del juego, estado y controles globales

**Features:**
- Timer para cada jugador (opcional)
- Botones de inicio/nuevo juego/reinicio
- Display del rating del jugador actual
- Selector de oponente (humano vs CPU, rating del jugador guardado, etc.)

### 5. Game Result View (`GameResultView`)
**Responsibility:** Muestra resultados de partidas y actualiza ratings

**Features:**
- Diálogo modal para Jaque Mate, Tablas, o Victoria por timeout
- Display del rating actualizado del jugador
- Botones para guardar partida, compartir resultado
- Animación de particle effects para finalización de partida

## Gesture Handling

### Tap Gestures
- **Tap en casilla vacía**: Selecciona pieza
- **Tap en pieza**: Selecciona pieza
- **Tap en movimiento legal**: Ejecuta movimiento
- **Tap en casilla con pieza enemiga**: Captura (después de selección)

### Long Press
- **Long press en pieza**: Muestra menú de contexto (ver historial, etc.)

### Swipe
- **Swipe en pieza seleccionada**: Mueve pieza al destino

## UI State Management

### Observed Objects
```swift
@ObservableClass
class ChessGameState {
    var boardState: String = ""
    var selectedSquare: String? = nil
    var validMoves: [String] = []
    var currentPlayer: Player = .white
    var gameStatus: GameStatus = .playing
    var playerRating: Double = 1500.0
    var opponentRating: Double = 1500.0
    var isDarkMode: Bool = false
}
```

### Navigation Flow
1. **Pantalla de Inicio** → `GameView`
2. **Configuración de Juego** → `GameView` con parámetros
3. **Juego en Curso** → `GameView` con tablero en vivo
4. **Fin de Partida** → `GameResultView`

## Design Specifications

### Colors
```swift
struct ChessColors {
    static let lightSquare = Color(red: 0.9804, green: 0.8431, blue: 0.7059)
    static let darkSquare = Color(red: 0.5216, green: 0.3686, blue: 0.2353)
    static let selectedSquare = Color(red: 0.2824, green: 0.7843, blue: 0.7059).opacity(0.5)
    static let validMoveSquare = Color(red: 0.2392, green: 0.4745, blue: 0.8118).opacity(0.3)
}
```

### Typography
- **Título:** SF Pro Display, peso 600, tamaño 28pt
- **Cuerpo:** SF Pro Text, peso 400, tamaño 17pt
- **Pequeño:** SF Pro Text, peso 300, tamaño 14pt

### Iconografía
- **Sistema:** SF Symbols (ajedrez personalizado)
- **Offline:** Assets SVG embebidos para alta resolución

## Accessibility

### VoiceOver
- Cada casilla debe serAnnouncements
- Texto descriptivo para cada pieza
- Anuncios para estados de Jaque/Jaque Mate

### Dynamic Type
- Soporta tamaños de fuente grande hasta 150%
- Mantiene la cuadricula legible a todos los tamaños

## Performance Targets

### Render Performance
- **60 FPS** en dispositivos iPhone 12 y posteriores
- **Target await 16ms** por frame
- **Memory usage**: < 100MB para la app completa

### Animation Performance
- **Spring animations** con velocidad de recuperación de 0.5s
- **Path animations** para movimientos de piezas
- **Opacity transitions** para mostrar/ocultar elementos UI

## Localization

### Supported Languages
- Inglés (principal)
- Español
- Francés
- Alemán
- Chino (simplificado)

### Text Direction
- **LTR** para idiomas occidentales
- **RTL** para idiomas que se leen de derecha a izquierda

## Testing Specifications

### UI Tests
- Snapshot tests para cada componente
- Interacciones de gestures automáticas
- Pruebas de accessibility con axe-core
- Pruebas de performance con XCUITest

### Integration Tests
- Flujo de juego completo con move validation
- Sincronización de estado con backend (si aplica)
- Compatibilidad con dispositivos iOS 14-18

## Deployment Requirements

### App Store Compliance
- **64-bit binary** (arm64)
- **App Sandbox** con documentos/documentos de contabilidad
- **HealthKit** (opcional) para guardar progreso de partidas
- **GameplayKit** para IA del oponente CPU

### Minimum Requirements
- **iOS**: 15.0+
- **Devices**: iPhone 8+, iPad (2019) y posteriores, iPod touch (7th generation) y posteriores
- **Architecture**: arm64

## Theme System

### Light/Dark Mode
- **Automatic**: Sigue la configuración del sistema
- **Manual**: Toggle desde Configuración
- **Persisted**: Guarda preferencia en UserDefaults

### Custom Themes
- **Classic**: Fondo de madera, piezas tradicionales
- **Modern**: Minimalista, diseño limpio
- **High Contrast**: Colores de alto contraste para accesibilidad