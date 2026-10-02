# FFI Contracts & Cross-Language Specifications

## Overview

This document defines the contracts and specifications for Foreign Function Interface (FFI) between the Rust core (`chess-core`) and the platform-specific applications (Android Kotlin and iOS Swift).

## Core Principles

### 1. Stable ABI
All FFI interfaces must maintain backward compatibility. Breaking changes require semantic versioning and coordinated releases across all platforms.

### 2. Error Handling Consistency
All FFI methods return `Result<T, Error>` or equivalent error types in target languages.

### 3. Data Serialization
- Simple types (`bool`, `i32`, `f64`, `String`) use standard serialization
- Complex types use flat structs for minimum overhead
- No dynamic memory allocation on Rust side for FFI calls

## Rust Core FFI Interface

### Module Declaration
```rust
// src/ffi/mod.rs
pub mod core {
    /// Get current board state as FEN string
    pub fn get_board_state() -> String;
    
    /// Get valid moves for a square
    /// 
    /// # Arguments
    /// * `square` - Algebraic notation (e.g., "e2")
    /// 
    /// # Returns
    /// Vector of UCI move strings (e.g., ["e2e4", "e2e3"])
    pub fn get_valid_moves(square: &str) -> Vec<String>;
    
    /// Execute a move on the board
    ///
    /// # Arguments
    /// * `uci_move` - UCI move notation (e.g., "e2e4")
    ///
    /// # Returns
    /// `Ok(())` if move executed successfully, `Err` otherwise
    pub fn play_move(uci_move: &str) -> Result<(), &'static str>;
    
    /// Get current player rating
    pub fn get_current_rating() -> f64;
    
    /// Update player rating after a game
    ///
    /// # Arguments
    /// * `opponent_rating` - Rating of the opponent
    /// * `result` - Game result: 1.0 (win), 0.5 (draw), 0.0 (loss)
    pub fn update_player_rating(opponent_rating: f64, result: f64);
    
    /// Get piece at a specific square
    ///
    /// # Arguments
    /// * `square` - Algebraic notation (e.g., "e2")
    ///
    /// # Returns
    /// String representation of the piece ("K", "Q", "R", "B", "N", "P", or "")
    pub fn get_piece_at(square: &str) -> String;
    
    /// Check if the current player is in check
    pub fn is_check() -> bool;
    
    /// Check if the game is in checkmate
    pub fn is_checkmate() -> bool;
    
    /// Check if the game is a draw
    pub fn is_draw() -> bool;
}
```

## Kotlin (Android) FFI Bindings

### Interface Definition
```kotlin
// android/app/src/main/java/com/example/chess/ffi/ChessCore.kt
package com.hectoralejandrg.plaintextchess.ffi

interface ChessCore {
    // Board State
    fun getBoardState(): String
    fun getValidMoves(square: String): List<String>
    fun playMove(uciMove: String): Result<Unit, ChessError>
    
    // Game State
    fun isCheck(): Boolean
    fun isCheckmate(): Boolean
    fun isDraw(): Boolean
    
    // Player Management
    fun getCurrentRating(): Double
    fun updatePlayerRating(opponentRating: Double, result: Double)
    
    // Piece Information
    fun getPieceAt(square: String): String
}
```

### Implementation (JNI/JVM)
```kotlin
// android/app/src/main/java/com/example/chess/ffi/ChessCoreImpl.kt
package com.hectoralejandrg.plaintextchess.ffi

class ChessCoreImpl : ChessCore {
    override fun getBoardState(): String {
        return ChessCoreNative.getBoardState()
    }
    
    override fun getValidMoves(square: String): List<String> {
        return ChessCoreNative.getValidMoves(square)
    }
    
    override fun playMove(uciMove: String): Result<Unit, ChessError> {
        return try {
            ChessCoreNative.playMove(uciMove)
            Result.success(Unit)
        } catch (e: Exception) {
            Result.failure(ChessError("Move failed: ${e.message}"))
        }
    }
    
    // ... other implementations
}
```

### Native Interface (Kotlin/Native)
```kotlin
// android/app/src/main/cpp/chess_core.h
#ifndef CHESS_CORE_H
#define CHESS_CORE_H

#include <stdio.h>
#include <stdbool.h>
#include <stdlib.h>

#ifdef __cplusplus
extern "C" {
#endif

// Board State
const char* get_board_state();

// Valid Moves
void get_valid_moves(const char* square, char** moves, int* count);

// Move Execution
int play_move(const char* uci_move);

// Player Rating

```

## Swift (iOS) FFI Bindings

### Protocol Definition
```swift
// ios/chess-core/ChessCore.swift
import Foundation

protocol ChessCore: AnyObject {
    // Board State
    func getBoardState() -> String
    func getValidMoves(forSquare square: String) -> [String]
    func playMove(_ uciMove: String) -> Result<Void, ChessError>
    
    // Game State
    func isCheck() -> Bool
    func isCheckmate() -> Bool
    func isDraw() -> Bool
    
    // Player Management
    func getCurrentRating() -> Double
    func updatePlayerRating(opponentRating: Double, result: Double)
    
    // Piece Information
    func getPieceAt(square: String) -> String
}
```

### Implementation (Swift Package)
```swift
// ios/chess-core/Sources/ChessCoreImpl.swift
import Foundation

class ChessCoreImpl: ChessCore {
    private let native: ChessCoreNative
    
    init() {
        self.native = ChessCoreNative()
    }
    
    func getBoardState() -> String {
        return native.getBoardState()
    }
    
    func getValidMoves(forSquare square: String) -> [String] {
        return native.getValidMoves(forSquare: square)
    }
    
    func playMove(_ uciMove: String) -> Result<Void, ChessError> {
        return native.playMove(uciMove)
    }
    
    // ... other implementations
}
```

### C Interface (Swift C Interop)
```swift
// ios/chess-core/Sources/ChessCoreCInterop.swift
import Foundation

struct ChessCoreC {
    let native: OpaquePointer?
    
    init?() {
        // Load the dynamic library
        let libraryPath = "../Frameworks/ChessCore.xcframework/ios-aarch64/ChessCore.framework/ChessCore"
        let library = dlopen(libraryPath, RTLD_NOW)
        guard let library = library else {
            return nil
        }
        self.native = library
    }
    
    func getBoardState() -> String {
        guard let funcPtr = dlsym(native, "get_board_state") else {
            fatalError("Function not found")
        }
        let func = unsafeBitCast(funcPtr, to: @escaping () -> UnsafePointer<CChar>.self)
        return String(cString: func())
    }
    
    // ... other C function wrappers
}
```

## Data Types and Serialization

### Rust Side
```rust
// Define simple, C-compatible types
#[repr(C)]
struct MoveDto {
    from: [c_char; 3],
    to: [c_char; 3],
    promotion: c_char,
}

#[repr(C)]
struct BoardStateDto {
    fen: *const c_char,
    board_width: c_int,
    board_height: c_int,
    pieces: *const c_char,
}
```

### Kotlin Side
```kotlin
// Data classes for Kotlin
@Parcelable
data class MoveDto(
    val from: String,
    val to: String,
    val promotion: Char?
) : Parcelable

data class BoardStateDto(
    val fen: String,
    val boardWidth: Int,
    val boardHeight: Int,
    val pieces: String
)
```

### Swift Side
```swift
// Structs for Swift
struct MoveDto: Codable {
    let from: String
    let to: String
    let promotion: Character?
}

struct BoardStateDto: Codable {
    let fen: String
    let boardWidth: Int
    let boardHeight: Int
    let pieces: String
}
```

## Error Handling

### Error Types
```rust
// Rust error types
pub enum ChessError {
    InvalidMove,
    BoardError,
    RatingError,
    FFIError(String),
}

impl std::fmt::Display for ChessError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            ChessError::InvalidMove => write!(f, "Invalid move"),
            ChessError::BoardError => write!(f, "Board error"),
            ChessError::RatingError => write!(f, "Rating error"),
            ChessError::FFIError(msg) => write!(f, "FFI error: {}", msg),
        }
    }
}
```

### Kotlin Error Types
```kotlin
// Kotlin sealed classes
sealed class ChessError {
    data class InvalidMove(val message: String) : ChessError()
    data class BoardError(val message: String) : ChessError()
    data class RatingError(val message: String) : ChessError()
    data class FFIError(val message: String) : ChessError()
}
```

### Swift Error Types
```swift
// Swift enum
enum ChessError: Error, LocalizedError {
    case invalidMove(String)
    case boardError(String)
    case ratingError(String)
    case ffiError(String)
    
    var errorDescription: String? {
        switch self {
        case .invalidMove(let message):
            return NSLocalizedString("Invalid move: \(message)", comment: "")
        case .boardError(let message):
            return NSLocalizedString("Board error: \(message)", comment: "")
        case .ratingError(let message):
            return NSLocalizedString("Rating error: \(message)", comment: "")
        case .ffiError(let message):
            return NSLocalizedString("FFI error: \(message)", comment: "")
        }
    }
}
```

## Performance Considerations

### Memory Management
- **Rust**: Use `CString` and `Vec` for temporary allocations
- **Kotlin**: Use `String` and `ArrayList` with proper lifecycle
- **Swift**: Use `String` and `Array` with ARC

### Thread Safety
- **Rust**: Use `Mutex` for shared mutable state
- **Kotlin**: Use `synchronized` blocks or `Mutex` from kotlinx.coroutines
- **Swift**: Use `DispatchQueue` for thread-safe operations

## Build and Deployment

### iOS
```swift
// XCFramework
// Frameworks/ChessCore.xcframework/
//   ├── ios-aarch64/
//   │   ├── ChessCore.framework/
//   │   │   ├── Headers/ChessCore.h
//   │   │   └── Libraries/libChessCore.a
//   └── ... other platforms
```

### Android
```kotlin
// gradle dependencies
// app/build.gradle.kts

dependencies {
    implementation("androidx.core:core-ktx:1.12.0")
    implementation("androidx.lifecycle:lifecycle-viewmodel-ktx:2.6.2")
    implementation("com.google.android.material:material:1.10.0")
    
    // Native library (jniLibs folder names use AGP NDK ABI tags)
    implementation(files("src/main/jniLibs/arm64-v8a/libchess_core.so"))
    implementation(files("src/main/jniLibs/x86_64/libchess_core.so"))
}
```

## Testing the FFI

### Integration Tests
```rust
// rust/tests/ffi_tests.rs
#[cfg(test)]
mod tests {
    use super::super::core;
    
    #[test]
    fn test_get_board_state() {
        let state = core::get_board_state();
        assert!(!state.is_empty());
    }
    
    #[test]
    fn test_get_valid_moves() {
        let moves = core::get_valid_moves("e2");
        assert!(!moves.is_empty());
    }
    
    #[test]
    fn test_play_move() {
        let result = core::play_move("e2e4");
        assert!(result.is_ok());
    }
}
```

### Platform Integration Tests
- **Android**: Espresso tests para interacción de UI + JUnit tests para FFI
- **iOS**: XCUITest para UI + Swift tests para FFI

## Maintenance and Evolution

### Versioning Strategy
- **Major version**: Cambios incompatibles en la API FFI
- **Minor version**: Nuevas características, API backward compatible
- **Patch version**: Correciones de errores

### Breaking Changes
1. Deprecar API vieja con advertencia
2. Implementar nueva API
3. Coordenar releases de todas las plataformas
4. Actualizar documentación

### Migration Guide
```swift
// Swift migration example
func migrateOldAPI() {
    // Old API: getBoardState()
    // New API: getBoardStateAsync()
    
    let oldState = session.getBoardState()
    let newState = try await session.getBoardStateAsync()
}
```

## Documentation

### FFI Documentation
- **Rust**: `src/ffi/README.md`
- **Kotlin**: `android/app/src/main/java/com/example/chess/ffi/README.md`
- **Swift**: `ios/chess-core/README.md`

### Contract Specifications
- **OpenAPI**: Generar swagger de API FFI
- **IDL**: Interfaz de lenguaje de definición (para clientes)
- **Schema**: Esquema JSON para validación

This document serves as the single source of truth for all FFI contracts between the Rust core and platform-specific applications. All platforms must implement these interfaces exactly to ensure interoperability.