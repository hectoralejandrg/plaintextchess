# PlainTextChess: Cross-Platform Chess App

A cross-platform chess application: a high-performance **Rust core** (game logic, legal-move
validation, Glicko-2 rating) exposing a **UniFFI** surface, plus native
**iOS (SwiftUI)** and **Android (Jetpack Compose)** apps. Grown from the
[app-core-monorepo](https://github.com/hectoralejandrg/app-core-monorepo) cross-platform template.

Both apps currently show a placeholder screen that proves the full FFI chain end-to-end
(initial FEN + starting rating, errors rendered in the UI instead of crashing). The playable
game UI is the active next milestone (see `openspec/` for the in-flight change).

## What's in the box

- `core/` — Rust crate `chess-core` with a UniFFI 0.28 FFI surface (`newGameSession`, `getBoardState`, `getCurrentRating`, …)
- `ios/app/` — `PlainTextChess` SwiftUI project consuming `ChessCore.xcframework`
- `android/app/` — `PlainTextChess` Compose module consuming `libchess_core.so` (JNI)
- `scripts/` — the **single producers** of every generated artifact
- `tests/` — build validators · `config/` — build settings · `docs/` — setup & training · `openspec/` — specs + change history

## Repository layout

```
plaintextchess/
├── core/                        # Rust crate chess-core (chess logic + Glicko-2 rating)
│   ├── src/lib.rs               #   UniFFI FFI surface
│   ├── src/domain/              #   chess logic + Glicko-2 rating
│   └── src/bin/cargo-uniffi-bindgen.rs   # in-project bindgen CLI (no external install)
├── ios/
│   ├── app/PlainTextChess.xcodeproj      # SwiftUI app (placeholder screen)
│   └── Frameworks/              # [generated] ChessCore.xcframework (not committed)
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
git clone https://github.com/<you>/plaintextchess.git
cd plaintextchess

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

- **iOS**: open `ios/app/PlainTextChess.xcodeproj` in Xcode and hit Run (or `xcodebuild -project
  ios/app/PlainTextChess.xcodeproj -scheme PlainTextChess -destination 'generic/platform=iOS Simulator' build`).
  The pre-build phase re-runs `build-ios.sh` automatically if the XCFramework is missing.
- **Android**: `cd android && ./gradlew :app:assembleDebug`, then install
  `app/build/outputs/apk/debug/app-debug.apk` on a device/emulator. The `ensureCore` Gradle
  task re-runs `build-android.sh` automatically if the `.so` files are missing.

Per-platform details, troubleshooting, and artifact reference: `docs/app-projects.md`,
`docs/support/troubleshooting.md`.

## Generated artifacts (never committed)

| Artifact | Produced by | Consumed by |
| --- | --- | --- |
| `ios/Frameworks/ChessCore.xcframework` (arm64 device + simulator slices) | `scripts/build-ios.sh` | `ios/app/PlainTextChess.xcodeproj` |
| `target/uniffi/ios/chess_core.swift` | `scripts/build-ios.sh` | iOS app (Swift) |
| `android/app/src/main/jniLibs/{arm64-v8a,x86_64}/libchess_core.so` | `scripts/build-android.sh` | `android/app` (packaged into the APK) |
| `target/uniffi/android/chess_core.kt` | `scripts/build-android.sh` | `android/app` (Kotlin) |
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
