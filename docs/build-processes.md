# Build Processes

How to build the Rust core and platform artifacts for iOS and Android.

## Scripts

| Script | Purpose |
| --- | --- |
| `./scripts/build-ios.sh` | Builds the Rust core for iOS and assembles `ios/Frameworks/ChessCore.xcframework` |
| `./scripts/build-android.sh` | Builds the Rust core for Android and installs `libchess_core.so` into `android/app/src/main/jniLibs/` |
| `./scripts/build-coordination.sh` | Orchestrates both platform builds with status reporting |

All scripts run from the repository root (they `cd` themselves), use `set -euo pipefail`, validate their spec and configuration files before compiling, and write metrics to `target/build-metrics/`.

## Building iOS

```bash
./scripts/build-ios.sh
```

Steps performed:
1. Validate `openspec/specs/ios/build-specification.md`, `config/ios-build.yaml`, and `core/Cargo.toml`.
2. Build the in-project UniFFI CLI (`cargo build --bin cargo-uniffi-bindgen`).
3. Compile `cargo build --release --target aarch64-apple-ios`.
4. Generate UniFFi Swift bindings (library mode, from the compiled staticlib) into `target/uniffi/ios/`.
5. Assemble `ios/Frameworks/ChessCore.xcframework/` (library + `Info.plist`).
6. Report artifacts and write `target/build-metrics/ios-last.json`.

## Building Android

```bash
./scripts/build-android.sh
```

Steps performed:
1. Validate `openspec/specs/android/build-specification.md`, `config/android-build.yaml`, and `core/Cargo.toml`.
2. Discover the Android NDK and its cross-clang (linker for the cdylib).
3. Build the in-project UniFFI CLI (`cargo build --bin cargo-uniffi-bindgen`).
4. For each target (`aarch64-linux-android`, `x86_64-linux-android`): install the rustup target if missing, compile the release build with the NDK clang as target linker, and copy `libchess_core.so` to `android/app/src/main/jniLibs/<abi>/`.
5. Generate UniFFi Kotlin bindings (library mode, from the aarch64 cdylib) into `target/uniffi/android/`.
6. Report artifacts and write `target/build-metrics/android-last.json`.

## Coordinated Build

```bash
# Both platforms, sequential (default): iOS then Android
./scripts/build-coordination.sh

# Single platform (CI matrix)
./scripts/build-coordination.sh --platform ios

# Both platforms in parallel (CI runners, >= 4 cores recommended)
./scripts/build-coordination.sh --parallel
```

The coordinator writes `target/build-metrics/coordination-last.json` with per-platform status, durations, and the overall result. Sequential mode stops at the first failure; parallel mode reports all failures. See `openspec/specs/shared/build-coordination.md` for error handling and recovery.

## Environment Variables

| Variable | Values | Default | Meaning |
| --- | --- | --- | --- |
| `BUILD_ENVIRONMENT` | `development`, `staging`, `production` | `production` | Target environment for the build |
| `PLATFORM_TARGET` | `ios`, `android` | set by the script | Platform being built |
| `BUILD_TYPE` | `debug`, `release` | `release` | Build profile |

Example:

```bash
BUILD_ENVIRONMENT=development BUILD_TYPE=debug ./scripts/build-ios.sh
```

## Artifacts and Metrics

- `ios/Frameworks/ChessCore.xcframework/` — iOS XCFramework
- `android/app/src/main/jniLibs/{aarch64,x86_64}/libchess_core.so` — Android native libraries
- `target/uniffi/{ios,android}/` — generated UniFFi bindings
- `target/build-metrics/{ios,android,coordination}-last.json` — last build metrics per platform

## Validation

- `./tests/validate-ios-build.sh` — validates the iOS build setup (spec, config, script, targets).
- `./tests/validate-android-build.sh` — validates the Android build setup.
- `./tests/validate-cross-platform.sh` — validates cross-platform consistency.
