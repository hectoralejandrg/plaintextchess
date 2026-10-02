# Cross-Platform Requirements

## Overview

This document defines the requirements and standards shared by both the iOS and Android build pipelines. It guarantees that the two platforms stay consistent in configuration, dependency management, validation, and performance targets. The delta requirements for this capability were introduced by the `setup-cross-platform-build` change.

## Unified Build Configuration

### Configuration Files
- `config/shared-build.yaml` — settings common to both platforms (toolchain, environment variables, shared performance targets, required files).
- `config/ios-build.yaml` — iOS-specific settings.
- `config/android-build.yaml` — Android-specific settings.

Platform configurations MUST extend the shared configuration: any value defined in `config/shared-build.yaml` applies to both platforms unless a platform configuration explicitly overrides it.

### Environment Variables
All build scripts MUST honor the same environment variables with the same semantics:

| Variable | Values | Default | Meaning |
| --- | --- | --- | --- |
| `BUILD_ENVIRONMENT` | `development`, `staging`, `production` | `production` | Target environment for the build |
| `PLATFORM_TARGET` | `ios`, `android` | set by the script | Platform being built |
| `BUILD_TYPE` | `debug`, `release` | `release` | Rust/profile build type |

### Configuration Validation
Every build script MUST, before compiling anything:
1. Verify its specification file exists (`openspec/specs/<platform>/build-specification.md`).
2. Verify its configuration file exists (`config/<platform>-build.yaml`).
3. Verify `core/Cargo.toml` exists.
4. Verify that every Rust target listed in the platform configuration is also listed in `config/shared-build.yaml`.
5. Fail fast with a clear error message if any check fails.

## Cross-Platform Dependency Management

### Single Source of Truth
- The Rust core (`core/`) is the single source of truth for chess logic, shared by both platforms through UniFFI bindings.
- Platform code MUST NOT reimplement chess rules; it MUST consume the FFI surface defined in `openspec/specs/shared/ffi-contracts.md`.

### Shared Dependencies
- `uniffi` 0.28 (crate) provides the runtime; the bindings CLI is the in-project binary `core/src/bin/cargo-uniffi-bindgen.rs` (uniffi 0.28 does not publish a standalone CLI crate), built by both platform scripts.
- `shakmaty` and `glicko2` are Rust-only dependencies; neither platform may vendor equivalent logic natively.
- `chess-core` crate type is `["rlib", "cdylib", "staticlib"]`: the staticlib feeds the iOS XCFramework, the cdylib feeds the Android `jniLibs`.

### Platform-Specific Dependencies
- **iOS:** Xcode 14+, Swift, `aarch64-apple-ios` Rust target.
- **Android:** Android SDK/NDK, Gradle, Kotlin, `aarch64-linux-android` and `x86_64-linux-android` Rust targets.
- Platform-specific dependencies MUST be declared in the platform configuration file, never hardcoded in scripts.

## Consistency Standards

- Artifact naming MUST follow the patterns defined per platform (`libchess_core.a` in the XCFramework, `libchess_core.so` in `jniLibs`).
- Every build script MUST write a metrics file to `target/build-metrics/<platform>-last.json` with the same schema: `platform`, `environment`, `build_type`, `status`, `duration_seconds`, `finished_at`.
- Build output MUST be scriptable: progress lines prefixed with `[build-<platform>]`, errors with `[build-<platform>][ERROR]`.
- Both platforms MUST report failures with a non-zero exit code and a machine-readable status in the metrics file.

## Validation and Compatibility Requirements

- `tests/validate-ios-build.sh` and `tests/validate-android-build.sh` MUST validate the platform-specific configuration against the existing build scripts.
- `tests/validate-cross-platform.sh` MUST verify that both platforms agree on shared settings (environment variables, toolchain targets, performance schema).
- Compatibility baseline: iOS 15.0+ (arm64) and Android API 24+ (aarch64, x86_64). Any change to the baseline MUST be reflected in both platform specifications and `config/shared-build.yaml`.

## Shared Performance Targets

Values mirrored from `config/shared-build.yaml`; individual platform details live in the platform specifications.

| Metric | iOS | Android | Coordinated |
| --- | --- | --- | --- |
| Clean build time | < 5 min | < 8 min | < 15 min total |
| Build memory | < 2 GB | < 4 GB | — |
| Artifact size | < 100 MB (IPA) | < 200 MB (APK/AAB) | — |
