# Design

## Context

See proposal.md for motivation. Current state that shapes this design:

- `core/` produces all game logic behind a UniFFI 0.28 FFI surface; `scripts/build-ios.sh` / `scripts/build-android.sh` produce the platform artifacts:
  - iOS: `ios/Frameworks/AppCore.xcframework` (single **device** slice `ios-aarch64`, hand-assembled Info.plist) + Swift bindings in `target/uniffi/ios/` (`app_core.swift`, `app_coreFFI.h`, `app_coreFFI.modulemap`).
  - Android: `android/app/src/main/jniLibs/{aarch64,x86_64}/libapp_core.so` + Kotlin binding at `target/uniffi/android/uniffi/app_core/app_core.kt`. (Implementation note: the ARM folder is `arm64-v8a`, not `aarch64` — AGP jniLibs folder names must be its NDK ABI tags; `scripts/build-android.sh`, the validator, the config, and the specs were updated accordingly.)
- `ios/` contains no Xcode project; `android/` contains no Gradle project.
- Build machine (verified): Xcode 27.0 with iOS 27 device + simulator SDKs and a booted iPhone 17 Pro simulator; Android SDK with platforms 35/36, build-tools 34–36, adb, and emulator system images (android-31, android-37.1); Java 17; **no global Gradle**, no `xcodegen`, no `xcodeproj` ruby gem. Network access to Maven Central / Google Maven is available.
- Repo conventions from the previous change: build scripts honor `BUILD_ENVIRONMENT`/`BUILD_TYPE`/`PLATFORM_TARGET`, write metrics to `target/build-metrics/`, and are bash 3.2-compatible (macOS system bash). `config/shared-build.yaml` sets `ios_min_version` (now "15.0" after D8) and `android_min_sdk: 24`.

## Goals / Non-Goals

**Goals:**
- Minimal, buildable app shells on both platforms that prove the FFI end-to-end: launch → placeholder screen → `newGameSession(1500)` (iOS) / `newGameSession(1500.0)` (Kotlin) → show initial FEN + rating.
- Self-contained developer experience: open the project (or run the wrapper) and build without hand-preparing generated files.
- Keep the change non-invasive: no `core/` or FFI surface changes; the Rust side stays frozen.

**Non-Goals:**
- Game UI (board interaction, move selection, two-player play), AI, networking, persistence, App Store/Play distribution, provisioning/signing, x86_64-Mac simulator support, and CI pipelines for the apps.

## Decisions

### D1 — Hand-craft the Xcode project instead of installing a generator
`ios/app/MyApp.xcodeproj` is a single-target app project written by hand (pbxproj with generated-but-stable UUIDs). No `xcodegen` or `xcodeproj` gem exists on the build machine, and adding one is a new machine-level dependency for a one-off scaffold.
- *Alternatives*: `brew install xcodegen` + `project.yml` (extra tooling, extra source of truth); SPM executable package (SPM cannot produce an iOS app bundle without Xcode anyway).
- Keep the project intentionally minimal: one app target, SwiftUI, no Storyboards, no test targets in this change.

### D2 — Consume the UniFFI Swift artifacts as a clang module + linked XCFramework
The Xcode app target compiles the generated `app_core.swift`, links `AppCore.xcframework` (static; link-only, no embed), and exposes `app_coreFFI` as a header-only Clang module using the generated `app_coreFFI.modulemap` (search path points at `target/uniffi/ios/`).
- Rationale: this is exactly the layout the uniffi generator emits; the generated Swift file guards its `import app_coreFFI` with `#if canImport(...)`, so both the module and a bridging-header setup work.
- *Fallback (recorded, not committed)*: if the module setup proves flaky under Xcode 27, the same two generated files can be supplied via a Swift/Objective-C bridging header instead of the clang module.

### D3 — Xcode pre-build phase guarantees artifacts
A "Shell Script" pre-build action runs `../../scripts/build-ios.sh` only when the XCFramework or the Swift bindings are missing (cheap test, idempotent script, cached rebuilds are seconds). This makes "open project → Run" work without a separate manual step, and keeps the source of truth for artifacts where it already is.

### D4 — Two-slice XCFramework via `xcodebuild -create-xcframework`
`scripts/build-ios.sh` additionally builds `aarch64-apple-ios-sim` and assembles a two-slice XCFramework with `xcodebuild -create-xcframework` (device slice `ios-arm64`, simulator slice `ios-arm64-simulator`), replacing the hand-written single-slice Info.plist assembly. The binding generation continues to use the device staticlib (FFI metadata is slice-independent).
- *Alternatives*: keep two separate XCFrameworks and switch per destination (more Xcode wiring, two artifacts to manage); hand-extend the Info.plist (fragile, format-pinned).
- Apple Silicon simulator only (arm64-sim). x86_64 Macs would need an extra slice — out of scope (see Open Questions).

### D5 — Android project: Gradle Kotlin DSL, Compose placeholder, pinned toolchain
New files under `android/`: `settings.gradle.kts`, root `build.gradle.kts`, `gradle.properties`, committed wrapper, and the `app/` module (`build.gradle.kts`, `AndroidManifest.xml`, Kotlin sources) reusing the existing `app/src/main/jniLibs/` location.
- Pinned set (compatible with the verified machine: Java 17, SDK 35, build-tools 35.0.0): **Gradle 8.7, AGP 8.5.2, Kotlin 1.9.24, Compose compiler 1.5.14 (targets Kotlin 1.9.24, wired via `android.composeOptions` — 1.x releases are not Gradle plugins), Compose BOM 2024.08.00 (Compose UI 1.6.8 — the runtime pairing for that Kotlin/compiler pair; 2024.05.00/1.6.2 is too old and breaks Compose intrinsic inlining), activity-compose 1.9.0**; `compileSdk 35`, `targetSdk 35`, `minSdk 24` (matches `config/shared-build.yaml`), `jvmTarget`/toolchain 17.
- **JNA dependency**: the UniFFI 0.28 Kotlin bindings call Rust through JNA (`com.sun.jna.*`), so the app declares `implementation("net.java.dev.jna:jna:5.14.0@aar")`. The AAR ships `libjnidispatch.so` for arm64-v8a/x86_64, which JNA loads via `System.loadLibrary` on Android at startup (official UniFFI Kotlin/Gradle docs recommend `jna >= 5.12.0` as an AAR).
- **jniLibs ABI folder names**: AGP expects its NDK ABI tags (`arm64-v8a`, `x86_64`) — the previous `aarch64` folder name was not recognized by `mergeDebugNativeLibs` ("... is not an ABI"). `scripts/build-android.sh` now emits `arm64-v8a`; the validator, `config/android-build.yaml`, and the specs were updated to match.
- **`buildFeatures.compose = true` is mandatory**: in AGP 8.5.x, `composeOptions.kotlinCompilerExtensionVersion` alone does NOT activate the Compose compiler — AGP only adds `androidx.compose.compiler:compiler:<version>` to the Kotlin task classpath when the compose build feature is enabled (verified by decompiling `VariantTaskManager.configureKotlinPluginTasksIfNecessary` from the AGP 8.5.2 jar). Without it, `@Composable` compiles as an ordinary annotation and the backend fails at IR lowering with `Backend Internal error: Couldn't inline method call ... ComposablesKt.remember` (the runtime class file only carries the compiler-transformed `remember(Function0, Composer, int)` signature). The BOM was also moved to 2024.08.00 (Compose UI 1.6.8), the runtime pairing for Kotlin 1.9.24 + Compose compiler 1.5.14 (1.9.24 released May 7 2024, compiler 1.5.14 released May 14 2024, per the AndroidX release notes).
- Placeholder screen: single Compose `Column` showing the app name, the initial FEN from `getBoardState()`, and the rating from `getCurrentRating()`; session created in `MainActivity` via the binding's constructor. Errors surface as `Text` (no crash).
- *Alternatives*: Views-based placeholder (less idiomatic, more XML boilerplate for the same value); latest AGP/Gradle (less of a tested pairing on this machine; upgrade is a follow-up).

### D6 — Self-sufficient Gradle wrapper
The wrapper (`gradlew`, `gradlew.bat`, `gradle/wrapper/gradle-wrapper.{jar,properties}` pinned to Gradle 8.7) is committed; only Java 17 + Android SDK are prerequisites.
- Bootstrap on this machine (no global Gradle): one-time `brew install gradle`, then `gradle wrapper --gradle-version 8.7` in `android/`. *Fallback*: reuse an existing `gradle-wrapper.jar` found on the machine and commit it with the pinned `gradle-wrapper.properties` (the properties file, not the jar, selects the distribution).
- The first `./gradlew` downloads the distribution + dependencies (~200–300 MB); acceptable one-time cost, documented.

### D7 — App-side handling of generated files
- Android: `app/build.gradle.kts` adds the generated directory to the main source set (`sourceSets["main"].kotlin.srcDir("../../target/uniffi/android")` so the `uniffi.app_core` package resolves from its generated path) and a `preBuild`-dependent task runs `../../scripts/build-android.sh` when `jniLibs` or the binding are missing.
- iOS: see D3; file references point at `target/uniffi/ios/` and `ios/Frameworks/AppCore.xcframework` via repository-relative paths.
- Both keep the build scripts as the single producer of artifacts; the app projects never commit generated code.

### D8 — App identity and baselines
- Names: "MyApp" on both platforms. Bundle ID `com.example.ios`, applicationId `com.example.android`.
- Android minSdk 24 comes from `config/shared-build.yaml` (baseline compatibility).
- iOS deployment target 15.0: the previous baseline (14.0) is not buildable with the available toolchain — Xcode 27 rejects deployment targets below 15.0 ("range of supported deployment target versions is 15.0 to 27.0.x"). The baseline is therefore raised to 15.0 everywhere (app project, `config/shared-build.yaml`, `config/ios-build.yaml`, and the platform specs that mirror it).

## Risks / Trade-offs

- [Hand-written pbxproj is fragile] → Keep it to one target and minimal sections; the first task sequence builds with `xcodebuild` immediately after authoring, so a malformed project fails fast and is fixed in place.
- [Clang module wiring for the header-only FFI can be finicky] → The generated Swift supports both the clang module and a bridging header (D2 fallback); whichever works gets documented in `docs/app-projects.md`.
- [First Gradle run is heavy (distribution + dependency downloads)] → Documented; pinned versions make later runs deterministic. If Maven/Google hosts are unreachable, the build is impossible — network is a stated prerequisite, same as for the Rust toolchain.
- [Two-slice XCFramework changes the artifact layout] → Downstream expectations are only `ios/Frameworks/AppCore.xcframework` (path unchanged); the validators check that path, not the slice count. `build-ios.sh` stays the single producer.
- [Simulator slice is arm64-only] → Apple Silicon Macs work; Intel Macs cannot run the app in this version. Mitigation: documented; adding `x86_64-apple-ios-sim` is a small follow-up (Open Question).
- [Emulator/device verification of the Android app is slow and flaky in this environment] → The required acceptance is the APK build + ABI content check; the emulator launch is an explicit optional task, not a gate.
- [Traded: no test targets in either app] → The FFI logic is already covered by `core/tests/ffi_smoke.rs`; app-side tests belong with the game UI change.

## Migration Plan

Purely additive: new directories/files plus one script extension. Rollback = delete `ios/app/`, the new `android/` Gradle files (keep `jniLibs`), revert `scripts/build-ios.sh`, and delete `docs/app-projects.md`. No data, no migration.

## Open Questions

- Should the XCFramework also ship an `x86_64-apple-ios-sim` slice for Intel Macs? (Deferrable; does not change the specs — the simulator requirement is satisfied by the arm64 slice on this machine.)
- Should app builds (xcodebuild/gradlew) be added to the GitHub Actions workflow? (Follow-up change; current CI only validates the Rust artifacts.)
