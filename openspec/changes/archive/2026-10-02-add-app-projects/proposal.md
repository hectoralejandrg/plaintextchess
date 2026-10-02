# Proposal

## Why

The cross-platform build system now produces working Rust FFI artifacts (`AppCore.xcframework` for iOS, `libapp_core.so` + Kotlin bindings for Android), but there is no app to consume them: `ios/` has no Xcode project and `android/` has no Gradle project. The user's goal — "levantar el proyecto en Android e iOS" — requires minimal runnable app shells that prove the FFI surface works end-to-end inside a real mobile app.

## What Changes

- New iOS app project (`ios/app/`) — an Xcode project with a placeholder SwiftUI screen that instantiates a `GameSession` through the UniFFI Swift bindings and displays the board state (FEN) and current rating. Links `AppCore.xcframework`.
- New Android app project (`android/`) — a Gradle project (Kotlin + Jetpack Compose) whose placeholder screen instantiates `GameSession` through the UniFFi Kotlin bindings and displays the board state and rating. Bundles `jniLibs` (aarch64 + x86_64).
- Committed Gradle wrapper for Android so the app builds without a global Gradle installation (Java 17 + Android SDK are the only prerequisites).
- `scripts/build-ios.sh` extended to also build the Apple-simulator slice (`aarch64-apple-ios-sim`) so the XCFramework supports both device and simulator, and the Swift bindings are available in one consistent location for the Xcode project.
- Documentation: how to open/build/run each app (`docs/app-projects.md`).

Non-goals: playable game UI (board interaction, move selection), AI opponent, networking, persistence, signing/provisioning, App Store / Play distribution. Those belong to follow-up changes.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `ios`: adds requirements for the iOS app project (placeholder screen that instantiates a game session via the Swift bindings) and for simulator support of `AppCore.xcframework` (device + simulator slices).
- `android`: adds requirements for the Android app project (placeholder screen that instantiates a game session via the Kotlin bindings, native libraries bundled) and for a self-sufficient Gradle wrapper build.

## Impact

- **New code**: `ios/app/` (Xcode project + Swift sources), `android/` (Gradle build files, wrapper, Kotlin sources, manifest).
- **Modified**: `scripts/build-ios.sh` (extra simulator slice + binding location); `docs/` (new app-projects guide).
- **Consumed artifacts**: `ios/Frameworks/AppCore.xcframework`, `target/uniffi/ios/*` (Swift), `android/app/src/main/jniLibs/*/libapp_core.so`, `target/uniffi/android/uniffi/app_core/app_core.kt` — all produced by the existing build scripts.
- **No changes** to `core/` (Rust crate, FFI surface unchanged), to `config/` or to the `android`/`shared` build scripts.
- **Build tooling assumed available** (verified on the build machine): Xcode 27 with iOS 27 simulator SDKs and iPhone simulators; Android SDK with platform 35/36, build-tools 34–36, and system images; Java 17. Gradle arrives via the committed wrapper (or a one-time `brew install gradle` to bootstrap it).
