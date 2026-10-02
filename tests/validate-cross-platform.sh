#!/bin/bash
#
# validate-cross-platform.sh — Validates cross-platform build consistency.
#
# Checks (per openspec/specs/shared/cross-platform-requirements.md and
# openspec/specs/shared/build-coordination.md):
#   - both platform validators pass
#   - environment variables have a single shared definition
#   - compatibility baselines agree between shared config and platform specs
#   - performance targets agree between shared config and platform configs
#   - the coordination script is present and syntactically valid
#
# Exit code: 0 when all checks pass, 1 otherwise.

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

FAILURES=0

pass() { printf '  [PASS] %s\n' "$*"; }
fail() { printf '  [FAIL] %s\n' "$*"; FAILURES=$((FAILURES + 1)); }

echo "Validating cross-platform build consistency..."

# --- Platform validators ---
if ./tests/validate-ios-build.sh >/dev/null 2>&1; then
    pass "iOS build validation passes"
else
    fail "iOS build validation passes"
fi
if ./tests/validate-android-build.sh >/dev/null 2>&1; then
    pass "Android build validation passes"
else
    fail "Android build validation passes"
fi

# --- Shared specs ---
check_file() { # $1: path, $2: description
    if [ -s "$1" ]; then pass "$2"; else fail "$2"; fi
}
check_file openspec/specs/shared/cross-platform-requirements.md \
    "shared spec openspec/specs/shared/cross-platform-requirements.md exists"
check_file openspec/specs/shared/build-coordination.md \
    "shared spec openspec/specs/shared/build-coordination.md exists"

# --- Single shared definition of environment variables ---
if grep -q "BUILD_ENVIRONMENT" config/shared-build.yaml \
   && grep -q "PLATFORM_TARGET" config/shared-build.yaml \
   && grep -q "BUILD_TYPE" config/shared-build.yaml; then
    pass "shared configuration defines BUILD_ENVIRONMENT, PLATFORM_TARGET, BUILD_TYPE"
else
    fail "shared configuration defines BUILD_ENVIRONMENT, PLATFORM_TARGET, BUILD_TYPE"
fi

# --- Compatibility baselines agree ---
if grep -q "ios_min_version: \"15.0\"" config/shared-build.yaml \
   && grep -q "min_ios_version: \"15.0\"" config/ios-build.yaml \
   && grep -q "15.0" openspec/specs/ios/build-specification.md; then
    pass "iOS minimum version (15.0) agrees across shared config, iOS config, and iOS spec"
else
    fail "iOS minimum version (15.0) agrees across shared config, iOS config, and iOS spec"
fi

if grep -q "android_min_sdk: 24" config/shared-build.yaml \
   && grep -q "min_sdk: 24" config/android-build.yaml \
   && grep -q "API 24" openspec/specs/android/build-specification.md; then
    pass "Android minimum SDK (24) agrees across shared config, Android config, and Android spec"
else
    fail "Android minimum SDK (24) agrees across shared config, Android config, and Android spec"
fi

# --- Performance targets agree between shared and platform configs ---
if ruby -ryaml -e '
  shared = YAML.load_file("config/shared-build.yaml")["performance_targets"]
  ios    = YAML.load_file("config/ios-build.yaml")["validation"]["performance"]
  android = YAML.load_file("config/android-build.yaml")["validation"]["performance"]
  ok = shared["ios"]["clean_build_minutes"] == ios["clean_build_minutes"]
  ok &&= shared["ios"]["artifact_size_mb"] == ios["artifact_size_mb"]
  ok &&= shared["android"]["clean_build_minutes"] == android["clean_build_minutes"]
  ok &&= shared["android"]["artifact_size_mb"] == android["artifact_size_mb"]
  exit(ok ? 0 : 1)
' 2>/dev/null; then
    pass "performance targets agree between config/shared-build.yaml and platform configs"
else
    fail "performance targets agree between config/shared-build.yaml and platform configs"
fi

# --- Coordination script ---
check_file scripts/build-coordination.sh "scripts/build-coordination.sh exists"
if bash -n scripts/build-coordination.sh 2>/dev/null; then
    pass "scripts/build-coordination.sh is syntactically valid"
else
    fail "scripts/build-coordination.sh is syntactically valid"
fi

# --- Coordinated performance target documented ---
if grep -q "total_build_minutes: 15" config/shared-build.yaml \
   && grep -q "< 15 min" openspec/specs/shared/build-coordination.md; then
    pass "coordinated build target (15 min) is consistent between config and spec"
else
    fail "coordinated build target (15 min) is consistent between config and spec"
fi

# --- Summary ---
if [ "$FAILURES" -eq 0 ]; then
    echo "Cross-platform validation: ALL CHECKS PASSED"
    exit 0
else
    echo "Cross-platform validation: $FAILURES check(s) FAILED"
    exit 1
fi
