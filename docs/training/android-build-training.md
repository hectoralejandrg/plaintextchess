# Android Build Training

Training material for the Android cross-platform build setup of
`app-core-monorepo`.
Target audience: developers who need to build, validate, and troubleshoot the
Android Rust artifacts (`libapp_core.so` per ABI + UniFFi Kotlin bindings).

## 1. Concepts

| Concept | What it means here |
| --- | --- |
| `app-core` | The Rust crate in `core/` containing all game logic. |
| UniFFI (library mode) | FFI framework; metadata is extracted from the compiled cdylib. |
| cdylib | `libapp_core.so` shared object, built per ABI target. |
| ABIs | `arm64-v8a` (devices, AGP NDK ABI tag for the `aarch64-linux-android` Rust target) and `x86_64` (emulator). Mapped in `config/android-build.yaml`. |
| jniLibs | `android/app/src/main/jniLibs/<abi>/libapp_core.so` — where Gradle picks the libraries up. |
| Kotlin bindings | Generated into `target/uniffi/android/` (`uniffi/app_core/app_core.kt`). |
| NDK | Provides the cross-clang + Bionic sysroot (`libc`, `liblog`, `libunwind`, …) required to **link** the `.so`. Without it the link step fails with missing `-lc -llog …`. |
| In-project CLI | `core/src/bin/cargo-uniffi-bindgen.rs` — the UniFFI 0.28 CLI. |

## 2. Prerequisites (checklist)

```bash
source "$HOME/.cargo/env"
cargo --version
rustup target list | grep linux-android
# NDK discovery order used by the script:
echo "${ANDROID_NDK_HOME:-}" "${NDK_HOME:-}"
ls ~/Library/Android/sdk/ndk        # macOS default location
```

The Android SDK must contain at least one NDK. The script picks the first one
found in the discovery list (`ANDROID_NDK_HOME` → `NDK_HOME` → `ANDROID_SDK_ROOT`/
`ANDROID_HOME`/`~/Library/Android/sdk`/`/usr/local/lib/android/sdk`).

## 3. Building from scratch

```bash
./scripts/build-android.sh
```

Expected steps (in order):

1. Validate `openspec/specs/android/build-specification.md`, `config/android-build.yaml`, `core/Cargo.toml`.
2. Discover the NDK and its `toolchains/llvm/prebuilt/<host>/bin` directory.
3. `cargo build --bin cargo-uniffi-bindgen` (host, debug).
4. For each target triple:
   - `rustup target add <triple>` (idempotent).
   - Export `CARGO_TARGET_<TRIPLE>_LINKER=<ndk>/…/<triple>21-clang`.
   - `cargo build --release --target <triple>`.
   - Copy `libapp_core.so` to `android/app/src/main/jniLibs/<abi>/`.
5. `./target/debug/cargo-uniffi-bindgen generate --library --crate app_core --language kotlin ...`
   (from the aarch64 cdylib).
6. Write `target/build-metrics/android-last.json`.

First run on a clean machine: ≈ 5–10 min (two cross target trees + CLI deps).
Cached runs finish in seconds.

## 4. Exercise: manual build and inspection

```bash
cd core
NDK=~/Library/Android/sdk/ndk/27.1.12297006   # or the NDK your machine has
CLANG=$NDK/toolchains/llvm/prebuilt/darwin-x86_64/bin
CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER=$CLANG/aarch64-linux-android21-clang \
    cargo build --release --target aarch64-linux-android
file target/aarch64-linux-android/release/libapp_core.so
# ELF 64-bit LSB shared object, ARM aarch64 …
```

Then verify the full pipeline:

```bash
./scripts/build-android.sh
find android/app/src/main/jniLibs -name '*.so' | sort
python3 -m json.tool target/build-metrics/android-last.json
./tests/validate-android-build.sh
```

## 5. Exercise: development build

```bash
BUILD_ENVIRONMENT=development BUILD_TYPE=debug ./scripts/build-android.sh
```

## 6. Common mistakes

| Symptom | Cause | Fix |
| --- | --- | --- |
| `linking with cc failed … --version-script` | Default Apple `cc`/ld64 used as linker for an ELF target. | Point `CARGO_TARGET_<TRIPLE>_LINKER` at the NDK clang (the script does this automatically). |
| `unable to find library -lc / -llog / -lunwind` | Linking with a bare `ld.lld` without the Bionic sysroot. | Use the NDK cross-clang, not a standalone lld, as the target linker. |
| `no Android NDK found` | NDK not installed or not discoverable. | Install an NDK or set `ANDROID_NDK_HOME` (see `docs/environment-setup.md`). |
| `aarch64: unbound variable` (on macOS) | Used bash 4+ `declare -A` with the system bash 3.2. | The script maps target→ABI with a `case` function (bash 3.2 compatible). |
| `no such command: uniffi-bindgen` | Invoked the CLI through cargo subcommand resolution. | Call the binary directly: `./target/debug/cargo-uniffi-bindgen generate …`. |
| `Crate app-core not found` | Hyphen in `--crate`. | Use `--crate app_core`. |
| Wrong `CARGO_TARGET_…` variable name | Missing `_` between triple and `LINKER` (e.g. `…ANDROIDLINKER`). | The variable is `CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER` — always `CARGO_TARGET_<TRIPLE>_LINKER`. |

## 7. Self-check

After completing this training you should be able to:

1. Run `./scripts/build-android.sh` and explain each log line.
2. Verify both `.so` files exist with the correct ELF arch (`file`).
3. Locate the generated Kotlin bindings and open `app_core.kt`.
4. Read `target/build-metrics/android-last.json` and explain each field.
5. Run `./tests/validate-android-build.sh` and interpret a failing check.
6. Explain why an NDK is required even though only Rust is being compiled.
