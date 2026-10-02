# iOS Build Specification

## Overview

This document defines the build configuration, deployment procedures, and performance requirements for the iOS chess application. It is the source of truth for everything that `scripts/build-ios.sh` must produce and validate. The delta requirements for this capability were introduced by the `setup-cross-platform-build` change.

## Build Configuration

### Build Targets
- **Rust target:** `aarch64-apple-ios` (release profile by default)
- **Architecture:** arm64 (64-bit only)
- **Minimum iOS version:** 15.0
- **Profile:** Release builds for distribution; Debug builds allowed for local development

### Configuration Source
All build settings are defined in `config/ios-build.yaml`:
- `build_targets`: Rust target triple(s) for iOS
- `deployment.min_ios_version`: minimum deployment version
- `deployment.xcframework_path`: output location of the XCFramework
- `binding`: UniFFi Swift binding generation parameters
- `validation`: required files and performance rules the build must satisfy

Environment overrides (see `openspec/specs/shared/cross-platform-requirements.md`):
- `BUILD_ENVIRONMENT` (development | staging | production, default: production)
- `BUILD_TYPE` (debug | release, default: release)
- `PLATFORM_TARGET` (ios)

## Tools and Dependencies

| Tool | Requirement | Used for |
| --- | --- | --- |
| Rust toolchain (`cargo`, `rustup`) | stable channel | Compile `chess-core` |
| `rustup target aarch64-apple-ios` | installed | Cross-compilation for iOS |
| In-project CLI `core/src/bin/cargo-uniffi-bindgen.rs` | built by the script | Generate Swift bindings (uniffi 0.28 library mode) |
| Xcode 14+ | with iOS SDK | Build/distribute the app and framework |

The Rust core crate is defined in `core/Cargo.toml` (crate `chess-core`, `crate-type = ["rlib", "cdylib"]`, UniFFi 0.28).

## Build Process

`scripts/build-ios.sh` performs the following steps in order:

1. **Specification validation** — verifies that `openspec/specs/ios/build-specification.md`, `config/ios-build.yaml`, and `core/Cargo.toml` exist and that the configured target is present in the configuration.
2. **Environment configuration** — reads `BUILD_ENVIRONMENT`, `BUILD_TYPE`, and `PLATFORM_TARGET` with defaults, and checks that `cargo` is available.
3. **UniFFI CLI build** — `cargo build --bin cargo-uniffi-bindgen` (in-project CLI target of `chess-core`; uniffi 0.28 does not publish a standalone CLI crate).
4. **Rust compilation** — `cargo build --release --target aarch64-apple-ios` and `cargo build --release --target aarch64-apple-ios-sim` (produces the cdylib and staticlib for device and Apple simulator; `staticlib` is declared in `core/Cargo.toml`).
5. **UniFFi binding generation** — library mode from the compiled device staticlib (FFI metadata is slice-independent): `./target/debug/cargo-uniffi-bindgen generate --library --crate chess_core --language swift --out-dir target/uniffi/ios/ target/aarch64-apple-ios/release/libchess_core.a`.
6. **XCFramework assembly** — `xcodebuild -create-xcframework` with both staticlibs, producing a two-slice XCFramework (`ios-arm64` device slice + `ios-arm64-simulator` simulator slice) with per-slice and top-level `Info.plist` files.
7. **Artifact generation documentation** — prints the list of generated artifacts and their locations, and writes build metrics to `target/build-metrics/ios-last.json`.

### Artifact Layout
```
ios/Frameworks/ChessCore.xcframework/
├── Info.plist
├── ios-arm64/
│   └── libchess_core.a
└── ios-arm64-simulator/
    └── libchess_core.a
target/uniffi/ios/            # generated Swift bindings
target/build-metrics/ios-last.json
```

## Deployment Requirements

### App Store Deployment
- 64-bit (arm64) binary, App Sandbox enabled
- Distribution via App Store Connect (IPA built from the Xcode project linking the XCFramework)
- Version and build number must be set before archiving (`xcodebuild archive`)

### Enterprise Distribution
- Enterprise signing profile must be selected in Xcode before `xcodebuild -exportArchive`
- Distribution via in-house provisioning profile

## Performance Targets

| Metric | Target |
| --- | --- |
| Clean build time | < 5 minutes |
| Build memory usage | < 2 GB |
| Final IPA artifact size | < 100 MB |

The build script reports elapsed time so that regressions against these targets can be detected (see `target/build-metrics/ios-last.json`).

## Validation Criteria

A build is considered valid when:
1. All required specification and configuration files exist (checked at the start of the script).
2. Rust compilation and UniFFi binding generation complete without errors.
3. `ios/Frameworks/ChessCore.xcframework/Info.plist`, `ios-arm64/libchess_core.a`, and `ios-arm64-simulator/libchess_core.a` exist after the build.
4. The reported elapsed time does not exceed the clean build target by more than 50% (warning only).
