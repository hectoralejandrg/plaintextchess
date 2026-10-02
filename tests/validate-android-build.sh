#!/bin/bash
#
# validate-android-build.sh — Validates the Android build setup.
#
# Checks (per openspec/specs/android/build-specification.md and
# openspec/specs/shared/cross-platform-requirements.md):
#   - specification and configuration files exist and are well-formed
#   - configured targets/ABIs are declared in the shared configuration
#   - the build script is syntactically valid and honors the shared env vars
#   - performance targets are consistent between config and spec
#   - built artifacts exist when a build has already run
#
# Exit code: 0 when all checks pass, 1 otherwise.

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

FAILURES=0

pass() { printf '  [PASS] %s\n' "$*"; }
fail() { printf '  [FAIL] %s\n' "$*"; FAILURES=$((FAILURES + 1)); }
check() { # $1: description, $2: command...
    local desc="$1"; shift
    if "$@" >/dev/null 2>&1; then pass "$desc"; else fail "$desc"; fi
}

echo "Validating Android build setup..."

# --- Specification files ---
check "spec file openspec/specs/android/build-specification.md exists" \
    test -s openspec/specs/android/build-specification.md
check "shared spec openspec/specs/shared/cross-platform-requirements.md exists" \
    test -s openspec/specs/shared/cross-platform-requirements.md

# --- Configuration files ---
check "config file config/android-build.yaml exists" test -s config/android-build.yaml
check "shared config config/shared-build.yaml exists" test -s config/shared-build.yaml
if command -v ruby >/dev/null 2>&1; then
    check "config/android-build.yaml is valid YAML" \
        ruby -ryaml -e 'YAML.load_file("config/android-build.yaml")'
    check "config/shared-build.yaml is valid YAML" \
        ruby -ryaml -e 'YAML.load_file("config/shared-build.yaml")'
fi

# --- Target consistency (platform config must be a subset of shared) ---
if ruby -ryaml -e '
  shared = YAML.load_file("config/shared-build.yaml")["toolchain"]["rust"]["targets"]
  android = YAML.load_file("config/android-build.yaml")["build_targets"]
  missing = android.reject { |t| shared.include?(t) }
  exit(missing.empty? ? 0 : 1)
' 2>/dev/null; then
    pass "Android build targets are declared in config/shared-build.yaml"
else
    fail "Android build targets are declared in config/shared-build.yaml"
fi

# --- Build script ---
check "scripts/build-android.sh exists" test -s scripts/build-android.sh
check "scripts/build-android.sh is syntactically valid" bash -n scripts/build-android.sh
for v in BUILD_ENVIRONMENT PLATFORM_TARGET BUILD_TYPE; do
    check "scripts/build-android.sh honors \$$v" grep -q "\${$v:-" scripts/build-android.sh
done

# --- Performance consistency (config vs spec) ---
if grep -q "clean_build_minutes: 8" config/android-build.yaml \
   && grep -q "< 8 min" openspec/specs/android/build-specification.md; then
    pass "Android clean-build target (8 min) is consistent between config and spec"
else
    fail "Android clean-build target (8 min) is consistent between config and spec"
fi

# --- Built artifacts (only when a build has already run) ---
if [ -f "target/build-metrics/android-last.json" ]; then
    check "arm64-v8a native library exists" \
        test -s android/app/src/main/jniLibs/arm64-v8a/libchess_core.so
    check "x86_64 native library exists" \
        test -s android/app/src/main/jniLibs/x86_64/libchess_core.so
else
    echo "  [SKIP] jniLibs not built yet (run ./scripts/build-android.sh first)"
fi

# --- Summary ---
if [ "$FAILURES" -eq 0 ]; then
    echo "Android build validation: ALL CHECKS PASSED"
    exit 0
else
    echo "Android build validation: $FAILURES check(s) FAILED"
    exit 1
fi
