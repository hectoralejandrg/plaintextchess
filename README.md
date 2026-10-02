# App Core Monorepo: Cross-Platform Mobile Template

A starting point for cross-platform mobile projects: a high-performance **Rust core** exposing a
**UniFFI** surface, plus native **iOS (SwiftUI)** and **Android (Jetpack Compose)** apps.
The included sample is a chess demo (board state, move validation, Glicko-2 rating); both apps are
placeholder screens that prove the full FFI chain end-to-end — initial FEN + starting rating on
screen, no crash. A playable game UI is a deliberate follow-up.

## What's in the box

- `core/` — Rust crate `app-core` with a UniFFI 0.28 FFI surface (`newGameSession`, `getBoardState`, `getCurrentRating`, …)
- `ios/app/` — `MyApp` SwiftUI project consuming `AppCore.xcframework`
- `android/app/` — `MyApp` Compose module consuming `libapp_core.so` (JNI)
- `scripts/` — the **single producers** of every generated artifact
- `tests/` — build validators · `config/` — build settings · `docs/` — setup & training · `openspec/` — specs + change history

## Repository layout

```
app-core-monorepo/
├── core/                        # Rust crate app-core (sample: chess demo)
│   ├── src/lib.rs               #   UniFFI FFI surface
│   ├── src/domain/              #   chess logic + Glicko-2 rating
│   └── src/bin/cargo-uniffi-bindgen.rs   # in-project bindgen CLI (no external install)
├── ios/
│   ├── app/MyApp.xcodeproj      # SwiftUI app (placeholder screen)
│   └── Frameworks/              # [generated] AppCore.xcframework (not committed)
├── android/
│   ├── app/                     # Compose app (placeholder screen)
│   └── gradlew + gradle/wrapper # committed Gradle 8.7 wrapper
├── scripts/                     # build-ios.sh / build-android.sh / build-coordination.sh
├── tests/                       # validate-{ios,android,cross-platform}-build.sh
├── config/                      # per-platform + shared build configuration (YAML)
├── docs/                        # environment-setup, app-projects, build docs, training
├── openspec/                    # specs/ + changes/archive/ (SDD history)
├── .opencode/                   # OpenSpec slash-commands (opsx-*)
└── .github/workflows/           # CI: validate + build both platforms + metrics report
```

## Prerequisites

| Tool | Requirement |
| --- | --- |
| Rust | `rustup` + `cargo` (stable). The build scripts **auto-install** the needed targets (`aarch64-apple-ios`, `aarch64-apple-ios-sim`, `aarch64-linux-android`, `x86_64-linux-android`). |
| Xcode | Recent stable (verified with Xcode 27) + an iOS Simulator runtime. macOS only. |
| JDK | 17 (`export JAVA_HOME=$(/usr/libexec/java_home -v 17)` on macOS). |
| Android SDK | Platform 35 + build-tools 35.0.0. |
| Android NDK | Any recent r2x (CI pins r27c). Found via `ANDROID_NDK_HOME`, `NDK_HOME`, or `$ANDROID_SDK_ROOT/ndk/*` / `~/Library/Android/sdk/ndk/*`. |

Pinned, self-sufficient versions (no global tooling needed): Gradle 8.7 (committed wrapper),
AGP 8.5.2, Kotlin 1.9.24, Compose compiler 1.5.14, Compose BOM 2024.08.00, minSdk 24 /
targetSdk 35, iOS baseline 15.0.

## Quick start

```bash
git clone https://github.com/<you>/app-core-monorepo.git
cd app-core-monorepo

# 1) Generate all FFI artifacts (Rust -> XCFramework + Swift bindings, jniLibs + Kotlin bindings)
./scripts/build-ios.sh       # macOS only (uses xcodebuild -create-xcframework)
./scripts/build-android.sh   # needs an Android NDK

# 2) Validate (specs, config, scripts, and the built artifacts when present)
./tests/validate-ios-build.sh
./tests/validate-android-build.sh
./tests/validate-cross-platform.sh

# 3) Test the Rust core (unit tests + ffi_smoke integration test)
(cd core && cargo test)
```

### Run the apps

- **iOS**: open `ios/app/MyApp.xcodeproj` in Xcode and hit Run (or `xcodebuild -project
  ios/app/MyApp.xcodeproj -scheme MyApp -destination 'generic/platform=iOS Simulator' build`).
  The pre-build phase re-runs `build-ios.sh` automatically if the XCFramework is missing.
- **Android**: `cd android && ./gradlew :app:assembleDebug`, then install
  `app/build/outputs/apk/debug/app-debug.apk` on a device/emulator. The `ensureCore` Gradle
  task re-runs `build-android.sh` automatically if the `.so` files are missing.

Per-platform details, troubleshooting, and artifact reference: `docs/app-projects.md`,
`docs/support/troubleshooting.md`.

## Generated artifacts (never committed)

| Artifact | Produced by | Consumed by |
| --- | --- | --- |
| `ios/Frameworks/AppCore.xcframework` (arm64 device + simulator slices) | `scripts/build-ios.sh` | `ios/app/MyApp.xcodeproj` |
| `target/uniffi/ios/app_core.swift` | `scripts/build-ios.sh` | iOS app (Swift) |
| `android/app/src/main/jniLibs/{arm64-v8a,x86_64}/libapp_core.so` | `scripts/build-android.sh` | `android/app` (packaged into the APK) |
| `target/uniffi/android/app_core.kt` | `scripts/build-android.sh` | `android/app` (Kotlin) |
| `target/build-metrics/{ios,android}-last.json` | both scripts | validators, CI metrics report |

Invariant: app projects **never** commit generated code. A fresh clone builds everything from the
Rust sources; the pre-build phases only regenerate when artifacts are missing.

## Development workflow (OpenSpec)

The repository ships with the full specification-driven setup — no `openspec init` needed:

- `openspec/specs/` — current specs (`rust-core`, `ios`, `android`, `shared`)
- `openspec/changes/archive/` — history of implemented changes
- `.opencode/` — `opsx-*` slash-commands (`/opsx:explore`, `/opsx:propose`, `/opsx:apply`,
  `/opsx:sync`, `/opsx:archive`, `/opsx:update`)

Propose a change, implement its tasks, sync the delta specs, then archive — see
`openspec/specs/README.md`.

## Continuous integration

`.github/workflows/build-validation.yml` runs on push/PR:

1. **validate** — runs the three validator scripts (specs, config, scripts, target consistency).
2. **build-ios** — builds the XCFramework on `macos-latest`.
3. **build-android** — builds the JNI libraries on `ubuntu-latest` (NDK r27c via
   `nttld/setup-ndk`).
4. **quality** — collects `target/build-metrics/*` from both builds and publishes a report artifact.

## License

MIT — see [`LICENSE`](LICENSE).
