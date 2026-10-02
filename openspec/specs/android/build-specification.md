# Android Build Specification

## Overview

This document defines the build configuration, deployment procedures, and performance requirements for the Android chess application. It is the source of truth for everything that `scripts/build-android.sh` must produce and validate. The delta requirements for this capability were introduced by the `setup-cross-platform-build` change.

## Build Configuration

### Build Targets
- **Rust targets:** `aarch64-linux-android` (device) and `x86_64-linux-android` (emulator), release profile by default
- **ABIs (jniLibs folders):** `arm64-v8a` and `x86_64` (AGP NDK ABI tags; the ARM device target is the Rust triple `aarch64-linux-android`)
- **Minimum SDK:** API 24
- **Profile:** Release builds for distribution; Debug builds allowed for local development

### Configuration Source
All build settings are defined in `config/android-build.yaml`:
- `build_targets`: Rust target triple(s) for Android
- `abis`: ABIs that must be produced and installed into `jniLibs`
- `deployment.min_sdk`: minimum Android API level
- `deployment.jni_libs_path`: output location of native libraries
- `binding`: UniFFi Kotlin binding generation parameters
- `validation`: required files and performance rules the build must satisfy

Environment overrides (see `openspec/specs/shared/cross-platform-requirements.md`):
- `BUILD_ENVIRONMENT` (development | staging | production, default: production)
- `BUILD_TYPE` (debug | release, default: release)
- `PLATFORM_TARGET` (android)

## Tools and Dependencies

| Tool | Requirement | Used for |
| --- | --- | --- |
| Rust toolchain (`cargo`, `rustup`) | stable channel | Compile `app-core` |
| `rustup target aarch64-linux-android` | installed | Cross-compilation for ARM devices |
| `rustup target x86_64-linux-android` | installed | Cross-compilation for x86_64 emulator |
| In-project CLI `core/src/bin/cargo-uniffi-bindgen.rs` | built by the script | Generate Kotlin bindings (uniffi 0.28 library mode) |
| Android NDK (cross-clang + Bionic sysroot) | discovered by the script | Link the cdylib `.so` files |
| Android SDK + Gradle | Android Studio or command-line tools | Build the app package (APK/AAB) |

The Rust core crate is defined in `core/Cargo.toml` (crate `app-core`, `crate-type = ["rlib", "cdylib"]`, UniFFi 0.28).

## Build Process

`scripts/build-android.sh` performs the following steps in order:

1. **Specification validation** — verifies that `openspec/specs/android/build-specification.md`, `config/android-build.yaml`, and `core/Cargo.toml` exist and that every configured target/ABI is present in the configuration.
2. **NDK discovery** — locates an Android NDK (`ANDROID_NDK_HOME`, `NDK_HOME`, or the default SDK locations) and selects its cross-clang; the NDK provides the Bionic sysroot required to link the cdylib.
3. **Environment configuration** — reads `BUILD_ENVIRONMENT`, `BUILD_TYPE`, and `PLATFORM_TARGET` with defaults, and checks that `cargo` and `rustup` are available.
4. **UniFFI CLI build** — `cargo build --bin cargo-uniffi-bindgen` (in-project CLI target of `app-core`).
5. **Rust compilation** — for each configured target: `rustup target add <triple>`, then `cargo build --release --target <triple>` with the NDK clang as the target linker (`CARGO_TARGET_<TRIPLE>_LINKER`).
6. **Native library installation** — copies each `libapp_core.so` into `android/app/src/main/jniLibs/<abi>/`.
7. **UniFFi binding generation** — library mode from the aarch64 cdylib: `./target/debug/cargo-uniffi-bindgen generate --library --crate app_core --language kotlin --out-dir target/uniffi/android/ target/aarch64-linux-android/release/libapp_core.so`.
8. **Artifact generation documentation** — prints the list of generated artifacts and their locations, and writes build metrics to `target/build-metrics/android-last.json`.

### Artifact Layout
```
android/app/src/main/jniLibs/
├── arm64-v8a/
│   └── libapp_core.so
└── x86_64/
    └── libapp_core.so
target/uniffi/android/             # generated Kotlin bindings
target/build-metrics/android-last.json
```

## Deployment Requirements

### Google Play Distribution
- App Bundle (AAB) is the required upload format for Play Console
- Native libraries packaged via `jniLibs` (arm64-v8a + x86_64)
- Release signing with the Play signing key must be configured in Gradle before upload

### Enterprise / Internal Distribution
- Signed APK built with `gradle assembleRelease` and a debug/enterprise keystore
- Distribution via internal app track or direct install

## Performance Targets

| Metric | Target |
| --- | --- |
| Clean build time | < 8 minutes |
| Build memory usage | < 4 GB |
| Final APK/AAB artifact size | < 200 MB |

The build script reports elapsed time so that regressions against these targets can be detected (see `target/build-metrics/android-last.json`).

## Validation Criteria

A build is considered valid when:
1. All required specification and configuration files exist (checked at the start of the script).
2. Rust compilation and UniFFi binding generation complete without errors for every configured target.
3. `android/app/src/main/jniLibs/arm64-v8a/libapp_core.so` and `android/app/src/main/jniLibs/x86_64/libapp_core.so` exist after the build.
4. The reported elapsed time does not exceed the clean build target by more than 50% (warning only).
