# App Projects (iOS + Android)

The `ios/app` and `android/app` projects are **placeholder scaffolds**: each one
instantiates a `GameSession` through the platform UniFFI bindings and displays
the initial board state (FEN) plus the player's starting rating. Errors surface
in the UI instead of crashing. A playable game UI is a follow-up.

Both apps consume artifacts produced by the shared build scripts — **never
commit generated code** (`target/uniffi/**`, `ios/Frameworks/**`, and
`android/app/src/main/jniLibs/**` are build outputs). The app pre-build phases
only regenerate artifacts when they are missing, so day-to-day builds stay fast.

| App | Location | Bindings consumed from | Core artifact |
| --- | --- | --- | --- |
| iOS (SwiftUI) | `ios/app/PlainTextChess.xcodeproj` | `target/uniffi/ios/` (`chess_core.swift`) | `ios/Frameworks/ChessCore.xcframework` |
| Android (Compose) | `android/app/` | `target/uniffi/android/` (`uniffi.chess_core`) | `android/app/src/main/jniLibs/{arm64-v8a,x86_64}/libchess_core.so` |

## 1. Prerequisites

### iOS

- Xcode (verified with Xcode 27 on Apple Silicon) with the iOS Simulator runtime.
- Rust with the `aarch64-apple-ios` and `aarch64-apple-ios-sim` targets
  (`rustup target add aarch64-apple-ios aarch64-apple-ios-sim`).
- `~/.cargo/bin` on `PATH` (the pre-build script phase adds it automatically).

### Android

- Java 17 (set per command: `export JAVA_HOME=$(/usr/libexec/java_home -v 17)`).
- Android SDK with platform 35, build-tools 35.0.0, and an NDK (discovered by
  `scripts/build-android.sh`).
- **No global Gradle is needed**: the Gradle 8.7 wrapper is committed under
  `android/` (`gradlew`, `gradle/wrapper/`).
- Rust with the `aarch64-linux-android` and `x86_64-linux-android` targets.
- `adb` from the SDK (`platform-tools`) for the emulator smoke test.

## 2. iOS app

```bash
# 1) Build the XCFramework + Swift bindings (only needed when missing;
#    the pre-build phase of the Xcode project runs this automatically).
./scripts/build-ios.sh

# 2) Build the app for the iOS simulator.
xcodebuild -project ios/app/PlainTextChess.xcodeproj \
    -scheme PlainTextChess \
    -destination 'generic/platform=iOS Simulator' build

# 3) Install + launch on a booted simulator.
xcrun simctl install <SIMUDID> \
    ~/Library/Developer/Xcode/DerivedData/PlainTextChess-*/Build/Products/Debug-iphonesimulator/PlainTextChess.app
xcrun simctl launch <SIMUDID> com.hectoralejandrg.plaintextchess

# 4) Optional: screenshot to verify the placeholder screen.
xcrun simctl io <SIMUDID> screenshot /tmp/ios-screen.png
```

Notes:

- The project links `ios/Frameworks/ChessCore.xcframework` (link-only, no
  embed) and exposes `chess_coreFFI` as a header-only clang module via the
  generated modulemap in `target/uniffi/ios/`.
- A pre-build script phase runs `scripts/build-ios.sh` only when the
  XCFramework or `chess_core.swift` are missing (`alwaysOutOfDate = 1` keeps
  the check cheap; suppresses the no-outputs warning intentionally).
- **arm64-simulator-only limitation**: both XCFramework slices are arm64
  (`ios-arm64` device + `ios-arm64-simulator`), matching Apple Silicon
  simulators. Intel Macs (x86_64 simulators) are not supported yet.
- Deployment target is 15.0 — Xcode 27 rejects `IPHONEOS_DEPLOYMENT_TARGET`
  below 15.0.

## 3. Android app

```bash
export JAVA_HOME=$(/usr/libexec/java_home -v 17)
cd android

# 1) Build the debug APK (the ensureCore preBuild hook runs
#    scripts/build-android.sh only when jniLibs or the Kotlin binding are
#    missing).
./gradlew :app:assembleDebug

# 2) Verify both ABIs are packaged (AGP repackages jniLibs under lib/<abi>/).
unzip -l app/build/outputs/apk/debug/app-debug.apk | grep libchess_core
# lib/arm64-v8a/libchess_core.so  and  lib/x86_64/libchess_core.so

# 3) Emulator smoke test (needs a booted AVD; adb from the SDK).
adb install -r app/build/outputs/apk/debug/app-debug.apk
adb shell am start -n com.hectoralejandrg.plaintextchess/.MainActivity
adb shell pidof com.hectoralejandrg.plaintextchess      # process still alive after 10+ s
adb logcat -d | grep -iE 'UnsatisfiedLink|FATAL'   # expect no matches
adb exec-out screencap -p > /tmp/android-screen.png
```

Notes:

- Pinned toolchain (see `design.md` D5): Gradle 8.7 (committed wrapper),
  AGP 8.5.2, Kotlin 1.9.24, Compose compiler 1.5.14 via
  `android.composeOptions` plus **`buildFeatures { compose = true }`**
  (AGP 8.5.x does not activate the Compose compiler from `composeOptions`
  alone), Compose BOM 2024.08.00, activity-compose 1.9.0,
  `compileSdk`/`targetSdk` 35, `minSdk` 24, Java 17.
- The UniFFI 0.28 Kotlin bindings call Rust through JNA, so the app declares
  `implementation("net.java.dev.jna:jna:5.14.0@aar")`; the AAR ships
  `libjnidispatch.so` for the Android ABIs, which JNA loads via
  `System.loadLibrary` at startup.
- jniLibs folder names use AGP NDK ABI tags: `arm64-v8a` (the
  `aarch64-linux-android` Rust target) and `x86_64`.
- The generated binding compiles in via
  `sourceSets["main"].kotlin.srcDir("../../target/uniffi/android")`, so the
  `uniffi.chess_core` package resolves from its generated path.

## 4. Where the generated artifacts come from

| Artifact | Producer | Consumed by |
| --- | --- | --- |
| `ios/Frameworks/ChessCore.xcframework` (2 arm64 slices) | `scripts/build-ios.sh` | Xcode project (link) |
| `target/uniffi/ios/chess_core.swift` (+ headers, modulemap) | `scripts/build-ios.sh` | Xcode project (source) |
| `android/app/src/main/jniLibs/{arm64-v8a,x86_64}/libchess_core.so` | `scripts/build-android.sh` | Gradle (jniLibs merge) |
| `target/uniffi/android/uniffi/chess_core/chess_core.kt` | `scripts/build-android.sh` | Gradle (kotlin source dir) |

Regenerating everything from scratch:

```bash
./scripts/build-ios.sh && ./scripts/build-android.sh
```

Validation (no app builds required):

```bash
./tests/validate-ios-build.sh
./tests/validate-android-build.sh
./tests/validate-cross-platform.sh
```
