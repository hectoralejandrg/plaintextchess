#!/bin/bash
#
# build-ios.sh — Builds the Rust core into the iOS XCFramework.
#
# Spec:   openspec/specs/ios/build-specification.md
# Config: config/ios-build.yaml (shared settings: config/shared-build.yaml)
#
# Environment overrides (see openspec/specs/shared/cross-platform-requirements.md):
#   BUILD_ENVIRONMENT  development|staging|production (default: production)
#   BUILD_TYPE         debug|release                  (default: release)
#   PLATFORM_TARGET    ios                            (default: ios)

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

# ---------------------------------------------------------------------------
# Environment configuration
# ---------------------------------------------------------------------------
BUILD_ENVIRONMENT="${BUILD_ENVIRONMENT:-production}"
PLATFORM_TARGET="${PLATFORM_TARGET:-ios}"
BUILD_TYPE="${BUILD_TYPE:-release}"

IOS_TARGET="aarch64-apple-ios"
IOS_SIM_TARGET="aarch64-apple-ios-sim"
XCFRAMEWORK_DIR="ios/Frameworks/AppCore.xcframework"
METRICS_FILE="target/build-metrics/ios-last.json"

log()  { printf '[build-ios] %s\n' "$*"; }
fail() { printf '[build-ios][ERROR] %s\n' "$*" >&2; exit 1; }

# ---------------------------------------------------------------------------
# Specification validation
# ---------------------------------------------------------------------------
SPEC_FILE="openspec/specs/ios/build-specification.md"
CONFIG_FILE="config/ios-build.yaml"

[ -f "$SPEC_FILE" ]     || fail "spec not found: $SPEC_FILE"
[ -f "$CONFIG_FILE" ]   || fail "config not found: $CONFIG_FILE"
[ -f "core/Cargo.toml" ] || fail "core/Cargo.toml not found"
grep -q "$IOS_TARGET" "$CONFIG_FILE" || fail "target '$IOS_TARGET' is not declared in $CONFIG_FILE"

command -v cargo  >/dev/null 2>&1 || fail "cargo not found. Install the Rust toolchain (docs/environment-setup.md)."
command -v rustup >/dev/null 2>&1 || fail "rustup not found. Install the Rust toolchain (docs/environment-setup.md)."

log "env: BUILD_ENVIRONMENT=$BUILD_ENVIRONMENT BUILD_TYPE=$BUILD_TYPE PLATFORM_TARGET=$PLATFORM_TARGET"

# ---------------------------------------------------------------------------
# Build
# ---------------------------------------------------------------------------
START_TS="$(date +%s)"
log "starting iOS build (targets: $IOS_TARGET $IOS_SIM_TARGET, type: $BUILD_TYPE)"

# Build Rust libraries for iOS (device + Apple simulator slices)
mkdir -p target/uniffi/ios
cd core

# Install the rustup targets if missing (idempotent)
rustup target add "$IOS_TARGET" "$IOS_SIM_TARGET"

# Build the in-project UniFFI CLI
cargo build --bin cargo-uniffi-bindgen

cargo build --release --target "$IOS_TARGET"
cargo build --release --target "$IOS_SIM_TARGET"

# Generate Swift bindings from the compiled device staticlib (library mode;
# the FFI metadata is slice-independent)
./target/debug/cargo-uniffi-bindgen generate \
    --library \
    --crate app_core \
    --language swift \
    --out-dir "../target/uniffi/ios/" \
    "target/$IOS_TARGET/release/libapp_core.a"

# Create the two-slice XCFramework in ios/Frameworks (per build specification)
cd "$REPO_ROOT"
command -v xcodebuild >/dev/null 2>&1 || fail "xcodebuild not found. Install Xcode with the iOS SDK (docs/environment-setup.md)."
rm -rf "$XCFRAMEWORK_DIR"
xcodebuild -create-xcframework \
    -library "core/target/$IOS_TARGET/release/libapp_core.a" \
    -library "core/target/$IOS_SIM_TARGET/release/libapp_core.a" \
    -output "$XCFRAMEWORK_DIR"

# ---------------------------------------------------------------------------
# Artifact generation documentation + metrics
# ---------------------------------------------------------------------------
END_TS="$(date +%s)"
DURATION=$((END_TS - START_TS))
FINISHED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

mkdir -p "$(dirname "$METRICS_FILE")"
cat > "$METRICS_FILE" << EOF
{
  "platform": "ios",
  "environment": "$BUILD_ENVIRONMENT",
  "build_type": "$BUILD_TYPE",
  "status": "success",
  "duration_seconds": $DURATION,
  "finished_at": "$FINISHED_AT"
}
EOF

log "generated artifacts:"
log "  - $XCFRAMEWORK_DIR/ (Info.plist, ios-arm64 + ios-arm64-simulator slices)"
log "  - target/uniffi/ios/ (UniFFi Swift bindings)"
log "  - $METRICS_FILE (build metrics)"
if [ "$DURATION" -gt 300 ]; then
    log "WARNING: build took ${DURATION}s, above the 5-minute clean-build target in $SPEC_FILE"
fi
log "iOS build completed in ${DURATION}s"
