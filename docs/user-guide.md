# User Guide: Cross-Platform Build System

Developer guide for building the chess app on iOS and Android with the OpenSpec-based build system.

## Quick Start

```bash
# 1. One-time environment setup (see docs/environment-setup.md for details)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
rustup target add aarch64-apple-ios aarch64-linux-android x86_64-linux-android
# Android only: an Android NDK must be available (cross-clang + Bionic sysroot)
ls ~/Library/Android/sdk/ndk   # or set ANDROID_NDK_HOME

# 2. Sanity-check your environment
./tests/validate-cross-platform.sh

# 3. Build what you need
./scripts/build-ios.sh              # iOS XCFramework + Swift bindings
./scripts/build-android.sh          # Android JNI libraries + Kotlin bindings
./scripts/build-coordination.sh     # Both, with a combined status report
```

## Everyday Commands

| What I want to do | Command |
| --- | --- |
| Build for iOS (device) | `./scripts/build-ios.sh` |
| Build for Android (device + emulator) | `./scripts/build-android.sh` |
| Build both, sequential (default) | `./scripts/build-coordination.sh` |
| Build both in parallel | `./scripts/build-coordination.sh --parallel` |
| Build only one platform | `./scripts/build-coordination.sh --platform android` |
| Debug build in a dev environment | `BUILD_ENVIRONMENT=development BUILD_TYPE=debug ./scripts/build-ios.sh` |
| Check my environment | `./tests/validate-ios-build.sh && ./tests/validate-android-build.sh && ./tests/validate-cross-platform.sh` |
| See the last build metrics | `cat target/build-metrics/ios-last.json target/build-metrics/android-last.json` |

## Where Artifacts Land

- **iOS:** `ios/Frameworks/ChessCore.xcframework/` (link this into the Xcode project)
- **Android:** `android/app/src/main/jniLibs/{aarch64,x86_64}/libchess_core.so`
- **Bindings:** `target/uniffi/ios/` (Swift), `target/uniffi/android/` (Kotlin)
- **Metrics:** `target/build-metrics/*.json`

## Environment Variables

| Variable | Values | Default | Meaning |
| --- | --- | --- | --- |
| `BUILD_ENVIRONMENT` | `development`, `staging`, `production` | `production` | Target environment |
| `PLATFORM_TARGET` | `ios`, `android` | per script | Platform being built |
| `BUILD_TYPE` | `debug`, `release` | `release` | Build profile |

## When a Build Fails

1. Read the `[ERROR]` line — it points at the missing prerequisite.
2. Run `./tests/validate-<platform>-build.sh` to see which check fails.
3. See [Troubleshooting](./support/troubleshooting.md) for the common cases.

## Where to Read More

- [Build specifications](./build-specifications.md) — what each spec file defines
- [Build processes](./build-processes.md) — step-by-step internals of each script
- [Environment setup](./environment-setup.md) — full prerequisite list
- [Admin guide](./admin-guide.md) — operations, CI, and maintenance
- [Troubleshooting](./support/troubleshooting.md) — failure modes and fixes
