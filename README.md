# App Core Monorepo: Cross-Platform Mobile Template

A starting point for cross-platform mobile projects: a high-performance Rust core with UniFFI bindings and native iOS/Android apps. The included sample demo is a chess application (game logic, move validation, rating).

## Overview

This template demonstrates a cross-platform application built with:

- **Rust Core**: High-performance chess logic, move validation, and rating system
- **iOS App**: Native iOS application built with SwiftUI
- **Android App**: Native Android application built with Jetpack Compose
- **OpenSpec**: Specification-driven development framework for coordinated development

## Architecture

```
app-core-monorepo/
├── openspec/                           # Specification-driven development
│   ├── specs/                         # All project specifications
│   │   ├── rust-core/                # Rust core specifications
│   │   ├── ios/                      # iOS specifications
│   │   ├── android/                 # Android specifications
│   │   └── shared/                  # Cross-platform specifications
│   └── changes/                      # Implemented changes history
├── core/                              # Rust core (sample: chess engine)
│   ├── Cargo.toml                      # Rust dependencies
│   ├── src/lib.rs                     # Core library
│   └── src/domain/                   # Domain modules
├── ios/                               # iOS Xcode project
│   └── Frameworks/                   # XCFramework for Rust core
├── android/                          # Android project
│   └── app/                          # Android app structure
└── scripts/                           # Build scripts
    ├── build-ios.sh                  # iOS build script
    └── build-android.sh              # Android build script
```

## Key Features

### Rust Core
- **Move Validation**: Legal move detection using Shakmaty chess library
- **Game State Management**: Complete board tracking with FEN support
- **Rating System**: Glicko-2 player rating calculations
- **Cross-Platform FFI**: Native bindings for iOS and Android

### iOS Application
- **SwiftUI Interface**: Modern iOS UI with reactive programming
- **8x8 Chess Board**: Interactive board with piece rendering
- **Gesture Handling**: Tap, drag, and swipe gestures for intuitive play
- **Dark Mode**: System-themed UI with Material Design
- **Accessibility**: VoiceOver support with semantic UI

### Android Application
- **Jetpack Compose**: Modern Android UI with declarative programming
- **Material Design**: Material Design components and theming
- **Native Performance**: Built with Android NDK for optimal performance
- **Drag and Drop**: Native Android drag-and-drop for piece movement

## Technical Specifications

### Rust Core
- **Language**: Rust 2021
- **Dependencies**: Shakmaty (chess logic), Glicko-2 (rating system), UniFFI (FFI)
- **Target Platforms**: iOS (arm64), Android (arm64, x86_64)
- **Performance**: <10ms move validation, <100MB memory usage

### iOS
- **Language**: Swift 5.0+
- **UI Framework**: SwiftUI
- **Target**: iOS 14.0+
- **Architecture**: MVVM with Combine

### Android
- **Language**: Kotlin 1.8+
- **UI Framework**: Jetpack Compose
- **Target**: API 24+
- **Architecture**: Jetpack Compose + ViewModel

## Development Workflow

### 1. Planning (OpenSpec)
Use OpenSpec to plan features before coding:

```bash
# Initialize OpenSpec for the project
openspec init

# Explore and plan features
/opsx:explore "Add chess move validation"

# Propose new features
/opsx:propose add-move-validation
```

### 2. Implementation
Follow the specification-driven workflow:

```bash
# Build for iOS
cd app-core-monorepo
./scripts/build-ios.sh

# Build for Android
./scripts/build-android.sh

# Test the Rust core
cd core
cargo test --lib
```

### 3. Testing
Test all components:

```bash
# Unit tests for Rust
cargo test --lib

# Integration tests
cargo test --test integration

# UI tests (if configured)
# (TBD)
```

## Getting Started

### Prerequisites

- **Rust**: Rust toolchain with `cargo`
- **iOS**: Xcode 14+ with iOS SDK
- **Android**: Android Studio with Android SDK and NDK

### Quick Start

1. **Clone the repository**
   ```bash
   git clone https
   cd app-core-monorepo
   ```

2. **Build for iOS**
   ```bash
   cd app-core-monorepo
   ./scripts/build-ios.sh
   ```

3. **Build for Android**
   ```bash
   cd app-core-monorepo
   ./scripts/build-android.sh
   ```

4. **Test the Rust core**
   ```bash
   cd app-core-monorepo/core
   cargo test --lib
   ```

## Project Structure

### Core (`core/`)
- `Cargo.toml`: Rust package configuration
- `src/lib.rs`: Main library exports
- `src/domain/`: Domain modules (board, rating)

### iOS (`ios/`)
- `Frameworks/`: XCFramework containing the Rust core
- SwiftUI views and views models

### Android (`android/`)
- `app/`: Android application structure
- JNI libraries for Rust FFI

### OpenSpec (`openspec/`)
- `specs/`: All project specifications
- `changes/`: History of implemented changes

### Build Scripts (`scripts/`)
- `build-ios.sh`: Build script for iOS
- `build-android.sh`: Build script for Android

## Contributing

### Development Guidelines

1. **Follow the OpenSpec workflow**: Plan features before implementation
2. **Maintain code quality**: Write clean, idiomatic Rust and Kotlin/Swift
3. **Test thoroughly**: Write comprehensive unit and integration tests
4. **Document changes**: Update OpenSpec specifications as needed

### Pull Request Process

1. Create a feature branch
2. Implement the feature following the spec
3. Add tests for the new functionality
4. Update relevant OpenSpec specifications
5. Submit a pull request with a clear description

## License

This project is licensed under the MIT License. See `LICENSE` for more information.

## Acknowledgments

- **Rust Core**: Built with [Shakmaty](https://github.com/dpc/shakmaty) and [Glicko-2](https://github.com/dmathwin/glicko2)
- **OpenSpec**: Built with [Fission AI OpenSpec](https://github.com/Fission-AI/OpenSpec)
- **UI Frameworks**: SwiftUI and Jetpack Compose

## Contact

For questions, suggestions, or contributions, please reach out through the GitHub repository or OpenSpec channels.

---

*This project uses OpenSpec for specification-driven development, ensuring coordinated development across all platforms.*