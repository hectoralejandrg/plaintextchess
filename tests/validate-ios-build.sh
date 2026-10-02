#!/bin/bash
#
# validate-ios-build.sh — Validates the iOS build setup.
#
# Checks (per openspec/specs/ios/build-specification.md and
# openspec/specs/shared/cross-platform-requirements.md):
#   - specification and configuration files exist and are well-formed
#   - configured targets are declared in the shared configuration
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

echo "Validating iOS build setup..."

# --- Specification files ---
check "spec file openspec/specs/ios/build-specification.md exists" \
    test -s openspec/specs/ios/build-specification.md
check "shared spec openspec/specs/shared/cross-platform-requirements.md exists" \
    test -s openspec/specs/shared/cross-platform-requirements.md

# --- Configuration files ---
check "config file config/ios-build.yaml exists" test -s config/ios-build.yaml
check "shared config config/shared-build.yaml exists" test -s config/shared-build.yaml
if command -v ruby >/dev/null 2>&1; then
    check "config/ios-build.yaml is valid YAML" \
        ruby -ryaml -e 'YAML.load_file("config/ios-build.yaml")'
    check "config/shared-build.yaml is valid YAML" \
        ruby -ryaml -e 'YAML.load_file("config/shared-build.yaml")'
fi

# --- Target consistency (platform config must be a subset of shared) ---
if ruby -ryaml -e '
  shared = YAML.load_file("config/shared-build.yaml")["toolchain"]["rust"]["targets"]
  ios    = YAML.load_file("config/ios-build.yaml")["build_targets"]
  missing = ios.reject { |t| shared.include?(t) }
  exit(missing.empty? ? 0 : 1)
' 2>/dev/null; then
    pass "iOS build targets are declared in config/shared-build.yaml"
else
    fail "iOS build targets are declared in config/shared-build.yaml"
fi

# --- Build script ---
check "scripts/build-ios.sh exists" test -s scripts/build-ios.sh
check "scripts/build-ios.sh is syntactically valid" bash -n scripts/build-ios.sh
for v in BUILD_ENVIRONMENT PLATFORM_TARGET BUILD_TYPE; do
    check "scripts/build-ios.sh honors \$$v" grep -q "\${$v:-" scripts/build-ios.sh
done

# --- Performance consistency (config vs spec) ---
if grep -q "clean_build_minutes: 5" config/ios-build.yaml \
   && grep -q "< 5 min" openspec/specs/ios/build-specification.md; then
    pass "iOS clean-build target (5 min) is consistent between config and spec"
else
    fail "iOS clean-build target (5 min) is consistent between config and spec"
fi

# --- Built artifacts (only when a build has already run) ---
if [ -f "target/build-metrics/ios-last.json" ]; then
    check "XCFramework Info.plist exists" \
        test -s ios/Frameworks/AppCore.xcframework/Info.plist
    check "XCFramework device slice library exists" \
        test -s ios/Frameworks/AppCore.xcframework/ios-arm64/libapp_core.a
    check "XCFramework simulator slice library exists" \
        test -s ios/Frameworks/AppCore.xcframework/ios-arm64-simulator/libapp_core.a
else
    echo "  [SKIP] XCFramework not built yet (run ./scripts/build-ios.sh first)"
fi

# --- Summary ---
if [ "$FAILURES" -eq 0 ]; then
    echo "iOS build validation: ALL CHECKS PASSED"
    exit 0
else
    echo "iOS build validation: $FAILURES check(s) FAILED"
    exit 1
fi
