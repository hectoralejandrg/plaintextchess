#!/bin/bash
#
# build-android.sh — Builds the Rust core into Android JNI libraries.
#
# Spec:   openspec/specs/android/build-specification.md
# Config: config/android-build.yaml (shared settings: config/shared-build.yaml)
#
# Environment overrides (see openspec/specs/shared/cross-platform-requirements.md):
#   BUILD_ENVIRONMENT  development|staging|production (default: production)
#   BUILD_TYPE         debug|release                  (default: release)
#   PLATFORM_TARGET    android                        (default: android)

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

# ---------------------------------------------------------------------------
# Environment configuration
# ---------------------------------------------------------------------------
BUILD_ENVIRONMENT="${BUILD_ENVIRONMENT:-production}"
PLATFORM_TARGET="${PLATFORM_TARGET:-android}"
BUILD_TYPE="${BUILD_TYPE:-release}"

# Rust target triple -> jniLibs ABI directory. AGP expects its NDK ABI tags
# ("arm64-v8a", "x86_64"), not the Rust/NDK triple short names.
target_abi() {
    case "$1" in
        aarch64-linux-android) echo "arm64-v8a" ;;
        x86_64-linux-android) echo "x86_64" ;;
        *) echo "unknown" ;;
    esac
}
ANDROID_TARGETS=("aarch64-linux-android" "x86_64-linux-android")
JNI_LIBS_DIR="android/app/src/main/jniLibs"
METRICS_FILE="target/build-metrics/android-last.json"

log()  { printf '[build-android] %s\n' "$*"; }
fail() { printf '[build-android][ERROR] %s\n' "$*" >&2; exit 1; }

# Locate an Android NDK (provides the Bionic sysroot used to link the cdylib).
find_ndk() {
    if [ -n "${ANDROID_NDK_HOME:-}" ] && [ -d "$ANDROID_NDK_HOME" ]; then
        echo "$ANDROID_NDK_HOME"; return
    fi
    if [ -n "${NDK_HOME:-}" ] && [ -d "$NDK_HOME" ]; then
        echo "$NDK_HOME"; return
    fi
    local sdk ndk
    for sdk in "${ANDROID_SDK_ROOT:-}" "${ANDROID_HOME:-}" "$HOME/Library/Android/sdk" "/usr/local/lib/android/sdk"; do
        [ -n "$sdk" ] && [ -d "$sdk/ndk" ] || continue
        for ndk in "$sdk"/ndk/*; do
            if [ -d "$ndk" ]; then echo "$ndk"; return; fi
        done
    done
    return 1
}

# ---------------------------------------------------------------------------
# Specification validation
# ---------------------------------------------------------------------------
SPEC_FILE="openspec/specs/android/build-specification.md"
CONFIG_FILE="config/android-build.yaml"

[ -f "$SPEC_FILE" ]      || fail "spec not found: $SPEC_FILE"
[ -f "$CONFIG_FILE" ]    || fail "config not found: $CONFIG_FILE"
[ -f "core/Cargo.toml" ] || fail "core/Cargo.toml not found"
for target in "${ANDROID_TARGETS[@]}"; do
    grep -q "$target" "$CONFIG_FILE" || fail "target '$target' is not declared in $CONFIG_FILE"
done

command -v cargo  >/dev/null 2>&1 || fail "cargo not found. Install the Rust toolchain (docs/environment-setup.md)."
command -v rustup >/dev/null 2>&1 || fail "rustup not found. Install the Rust toolchain (docs/environment-setup.md)."

log "env: BUILD_ENVIRONMENT=$BUILD_ENVIRONMENT BUILD_TYPE=$BUILD_TYPE PLATFORM_TARGET=$PLATFORM_TARGET"

# ---------------------------------------------------------------------------
# Build
# ---------------------------------------------------------------------------
START_TS="$(date +%s)"
log "starting Android build (targets: ${ANDROID_TARGETS[*]}, type: $BUILD_TYPE)"

# Build Rust library for Android
mkdir -p target/uniffi/android
cd core

# Cross-linking the cdylib requires an Android NDK (Bionic sysroot).
NDK="$(find_ndk)" || fail "no Android NDK found. Set ANDROID_NDK_HOME (or install the SDK; see docs/environment-setup.md)."
case "$(uname -s)" in
    Darwin) NDK_HOST="darwin-x86_64" ;;
    Linux)  NDK_HOST="linux-x86_64" ;;
    *)      NDK_HOST="windows" ;;
esac
NDK_CLANG_BIN="$NDK/toolchains/llvm/prebuilt/$NDK_HOST/bin"
[ -d "$NDK_CLANG_BIN" ] || fail "NDK prebuilt toolchain not found at $NDK_CLANG_BIN"
log "using NDK: $NDK"

# Build the in-project UniFFI CLI
cargo build --bin cargo-uniffi-bindgen

for target in "${ANDROID_TARGETS[@]}"; do
    abi="$(target_abi "$target")"
    [ "$abi" = "unknown" ] && fail "no ABI mapping for target '$target'"
    log "building $target (abi: $abi)"

    # Install the target toolchain if missing
    rustup target add "$target"

    # Link the cdylib with the NDK clang (knows the Bionic sysroot)
    upper="$(echo "$target" | tr 'a-z-' 'A-Z_')"
    export "CARGO_TARGET_${upper}_LINKER=$NDK_CLANG_BIN/${target}21-clang"
    cargo build --release --target "$target"

    # Install the .so into jniLibs for the app
    cd "$REPO_ROOT"
    mkdir -p "$JNI_LIBS_DIR/$abi"
    cp "core/target/$target/release/libapp_core.so" "$JNI_LIBS_DIR/$abi/"
    cd core
done

# Generate Kotlin bindings from the aarch64 cdylib (library mode)
./target/debug/cargo-uniffi-bindgen generate \
    --library \
    --crate app_core \
    --language kotlin \
    --out-dir "../target/uniffi/android/" \
    "target/${ANDROID_TARGETS[0]}/release/libapp_core.so"

cd "$REPO_ROOT"

# ---------------------------------------------------------------------------
# Artifact generation documentation + metrics
# ---------------------------------------------------------------------------
END_TS="$(date +%s)"
DURATION=$((END_TS - START_TS))
FINISHED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

mkdir -p "$(dirname "$METRICS_FILE")"
cat > "$METRICS_FILE" << EOF
{
  "platform": "android",
  "environment": "$BUILD_ENVIRONMENT",
  "build_type": "$BUILD_TYPE",
  "status": "success",
  "duration_seconds": $DURATION,
  "finished_at": "$FINISHED_AT"
}
EOF

log "generated artifacts:"
log "  - $JNI_LIBS_DIR/arm64-v8a/libapp_core.so"
log "  - $JNI_LIBS_DIR/x86_64/libapp_core.so"
log "  - target/uniffi/android/ (UniFFi Kotlin bindings)"
log "  - $METRICS_FILE (build metrics)"
if [ "$DURATION" -gt 480 ]; then
    log "WARNING: build took ${DURATION}s, above the 8-minute clean-build target in $SPEC_FILE"
fi
log "Android build completed in ${DURATION}s"
