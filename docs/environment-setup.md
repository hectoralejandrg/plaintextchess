# Development Environment Setup

Prerequisites and setup steps for building this project on macOS. The same steps apply on Linux for the Android side (Xcode/iOS steps are macOS-only).

## 1. Rust Toolchain

```bash
# Install rustup + stable toolchain
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"

# Verify
cargo --version
rustup --version

# Install the cross-compilation targets (mirrors config/shared-build.yaml)
rustup target add aarch64-apple-ios
rustup target add aarch64-linux-android
rustup target add x86_64-linux-android
```

The UniFFI CLI is an in-project binary target (`core/src/bin/cargo-uniffi-bindgen.rs`) that the build scripts compile before generating bindings; no separate installation is required.

## 2. iOS (macOS only)

```bash
# Xcode 14 or newer with the iOS SDK
xcodebuild -version
xcodebuild -showsdks | grep iOS
```

- Accept the Xcode license if prompted: `sudo xcodebuild -license accept`
- The iOS build (`./scripts/build-ios.sh`) only produces the XCFramework and bindings; building the final app requires the Xcode project linking `ios/Frameworks/AppCore.xcframework`.

## 3. Android

```bash
# Verify Android SDK
echo "$ANDROID_HOME"        # or $ANDROID_SDK_ROOT

# Verify an NDK is available (needed to link the Rust cdylib .so files)
ls "$ANDROID_HOME/ndk"      # or set ANDROID_NDK_HOME to a specific NDK
```

- Android SDK with platform API 24+ and an NDK (the script discovers it via `ANDROID_NDK_HOME`, `NDK_HOME`, or the default SDK location; on macOS the default is `~/Library/Android/sdk/ndk/<version>`).
- The NDK's cross-clang is used as the Rust target linker because it knows the Bionic sysroot (`libc`, `liblog`, `libunwind`, ...).
- The Android build script (`./scripts/build-android.sh`) only produces the Rust JNI libraries and UniFFi Kotlin bindings; the app package (APK/AAB) is built with Gradle from `android/app`.

## 4. Utilities Used by the Build System

| Tool | Used for | Check |
| --- | --- | --- |
| `bash` ≥ 3.2 | All build scripts | `bash --version` |
| `jq` or `sed` | Metrics reporting in `build-coordination.sh` | `jq --version` |
| `ruby` (with stdlib `yaml`) or any YAML linter | CI YAML validation | `ruby -ryaml -e 'puts 1'` |
| `openspec` CLI | Spec validation (`openspec validate`) | `openspec --version` |

## 5. Verify the Environment

Run the validation test scripts; they check specs, configuration, scripts, and toolchain presence:

```bash
./tests/validate-ios-build.sh
./tests/validate-android-build.sh
./tests/validate-cross-platform.sh
```

All three should exit with code 0 on a correctly configured machine.

## 6. OpenSpec Workflow (optional)

```bash
openspec list --json                      # active changes
openspec validate --all                   # validate specs and changes
```
