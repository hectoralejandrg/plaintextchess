# Tasks

## 1. iOS framework: two-slice XCFramework

- [x] 1.1 Extend `scripts/build-ios.sh` to also build `aarch64-apple-ios-sim` and assemble the XCFramework with `xcodebuild -create-xcframework` using standard slice names (`ios-arm64` + `ios-arm64-simulator`), keeping the artifact path `ios/Frameworks/AppCore.xcframework`, the Swift binding step, and the metrics output unchanged
  - Verify: run `./scripts/build-ios.sh`, then `ls ios/Frameworks/AppCore.xcframework/` shows both `ios-arm64` and `ios-arm64-simulator` directories, each containing `libapp_core.a` and `Info.plist`, and `target/build-metrics/ios-last.json` is updated
- [x] 1.2 Update `tests/validate-ios-build.sh` artifact checks to the new standard slice directory names (previously `ios-aarch64`)
  - Verify: after a fresh `./scripts/build-ios.sh`, `./tests/validate-ios-build.sh` reports ALL CHECKS PASSED

## 2. iOS app project

- [x] 2.1 Author `ios/app/MyApp.xcodeproj` (hand-written, single SwiftUI app target "MyApp", deployment target 15.0 — see D8, the Xcode 27 toolchain rejects targets below 15.0 — bundle `com.example.ios`) that links `ios/Frameworks/AppCore.xcframework` (link-only, no embed), compiles the generated `app_core.swift`, exposes `app_coreFFI` as a header-only clang module (generated modulemap + header search path to `target/uniffi/ios/`), and includes a pre-build script phase that runs `scripts/build-ios.sh` when the XCFramework or the Swift bindings are missing
  - Verify: `xcodebuild -project ios/app/MyApp.xcodeproj -scheme MyApp -destination 'generic/platform=iOS Simulator' build` exits 0
- [x] 2.2 Implement the placeholder SwiftUI screen that creates a game session through the Swift bindings and displays the initial board state (FEN) and the starting rating, showing the error in the UI instead of crashing when session creation fails
  - Verify: the app target builds (`xcodebuild ... build` exits 0) and the view code calls the bindings' session constructor, board-state, and rating accessors
- [x] 2.3 Launch the app on the iPhone 17 Pro simulator (`xcrun simctl install` + `xcrun simctl launch`)
  - Verify: the app process is still alive 10+ seconds after launch (check via `xcrun simctl spawn <sim> ps` / `pgrep`) and the simulator shows the placeholder screen with the standard start-position FEN and the starting rating

## 3. Android app project

- [x] 3.1 Bootstrap the committed Gradle wrapper pinned to Gradle 8.7 in `android/` (one-time `brew install gradle` on this machine if Gradle is absent, then `gradle wrapper --gradle-version 8.7`; fallback: reuse an existing `gradle-wrapper.jar` found on the machine)
  - Verify: `./gradlew --version` in `android/` prints Gradle 8.7 and the wrapper files (`gradlew`, `gradlew.bat`, `gradle/wrapper/`) are in place
- [x] 3.2 Create the root Gradle files: `settings.gradle.kts` (pluginManagement + dependencyResolutionManagement + `include(":app")`), root `build.gradle.kts` (AGP 8.5.2, Kotlin 1.9.24, Compose plugin), and `gradle.properties` (AndroidX enabled, jvmargs)
  - Verify: `./gradlew help` exits 0
- [x] 3.3 Create the `app` module: `build.gradle.kts` (com.android.application, applicationId `com.example.android`, compileSdk 35, minSdk 24, targetSdk 35, Java 17, `buildFeatures.compose = true` + Compose compiler 1.5.14 via `composeOptions` + Compose BOM 2024.08.00 + activity-compose 1.9.0 + JNA 5.14.0 AAR for the UniFFI bindings, `sourceSets["main"].kotlin.srcDir("../../target/uniffi/android")` so the generated `uniffi.app_core` binding compiles in, and a task that runs `scripts/build-android.sh` when jniLibs or the binding are missing), `AndroidManifest.xml` with a launcher `MainActivity`, and the Compose placeholder screen that creates a session and displays the initial FEN + rating (error shown in the UI instead of a crash)
  - Verify: `./gradlew :app:assembleDebug` exits 0
- [x] 3.4 Verify the debug APK contains the native libraries for both ABIs
  - Verify: `unzip -l android/app/build/outputs/apk/debug/app-debug.apk` lists `lib/arm64-v8a/libapp_core.so` and `lib/x86_64/libapp_core.so` (AGP repackages jniLibs under `lib/<abi>/`; the ARM tag is `arm64-v8a`)
- [x] 3.5 (Optional) Emulator smoke test: create/boot an AVD from a locally available system image, install the debug APK, start `MainActivity`
  - Verify: the app process is still running 10+ seconds after `adb shell am start` and logcat shows no UnsatisfiedLinkError or FFI crash

## 4. Documentation and cross-validation

- [x] 4.1 Write `docs/app-projects.md`: prerequisites per platform (Xcode + simulator; Java 17 + Android SDK, no global Gradle needed), how to open/build/run each app, where the generated artifacts come from, and the arm64-simulator-only limitation
  - Verify: every command in the doc matches the actual scripts/projects and was exercised at least once
- [x] 4.2 Run the final cross-validation suite
  - Verify: `openspec validate --all` passes (includes the change), `bash -n` succeeds on all modified scripts, and `./tests/validate-ios-build.sh`, `./tests/validate-android-build.sh`, `./tests/validate-cross-platform.sh` all report ALL CHECKS PASSED
