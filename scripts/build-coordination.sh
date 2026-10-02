#!/bin/bash
#
# build-coordination.sh — Orchestrates the iOS and Android builds.
#
# Spec:   openspec/specs/shared/build-coordination.md
# Config: config/shared-build.yaml
#
# Usage:
#   ./scripts/build-coordination.sh [--platform ios|android|both] [--parallel]
#
# Behavior (per openspec/specs/shared/build-coordination.md):
#   - Sequential by default (iOS, then Android); stops at the first failure.
#   - --parallel runs both platforms concurrently and reports all failures.
#   - Writes target/build-metrics/coordination-last.json with per-platform
#     status, durations, and the overall coordinated result.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

PLATFORM="both"
PARALLEL=0

log()  { printf '[build-coordination] %s\n' "$*"; }
fail() { printf '[build-coordination][ERROR] %s\n' "$*" >&2; exit 1; }

while [ "$#" -gt 0 ]; do
    case "$1" in
        --platform)
            [ "$#" -ge 2 ] || fail "--platform requires a value (ios|android|both)"
            PLATFORM="$2"; shift 2 ;;
        --parallel)
            PARALLEL=1; shift ;;
        -h|--help)
            grep '^#' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *)
            fail "unknown argument: $1 (see --help)" ;;
    esac
done

case "$PLATFORM" in
    ios|android|both) ;;
    *) fail "invalid --platform value: $PLATFORM (expected ios|android|both)" ;;
esac

# ---------------------------------------------------------------------------
# Pre-flight: shared specification validation
# ---------------------------------------------------------------------------
[ -f "openspec/specs/shared/build-coordination.md" ] || fail "spec not found: openspec/specs/shared/build-coordination.md"
[ -f "config/shared-build.yaml" ] || fail "config not found: config/shared-build.yaml"

if [ "$PLATFORM" = "ios" ] || [ "$PLATFORM" = "both" ]; then
    [ -f "openspec/specs/ios/build-specification.md" ]   || fail "iOS spec not found"
    [ -f "config/ios-build.yaml" ]                        || fail "iOS config not found"
fi
if [ "$PLATFORM" = "android" ] || [ "$PLATFORM" = "both" ]; then
    [ -f "openspec/specs/android/build-specification.md" ] || fail "Android spec not found"
    [ -f "config/android-build.yaml" ]                       || fail "Android config not found"
fi

IOS_METRICS="target/build-metrics/ios-last.json"
ANDROID_METRICS="target/build-metrics/android-last.json"
COORD_METRICS="target/build-metrics/coordination-last.json"

# ---------------------------------------------------------------------------
# Run the selected platform builds
# ---------------------------------------------------------------------------
START_TS="$(date +%s)"
IOS_FAILED=0
ANDROID_FAILED=0

run_ios()     { ./scripts/build-ios.sh; }
run_android() { ./scripts/build-android.sh; }

if [ "$PARALLEL" -eq 1 ]; then
    log "starting parallel coordinated build (platform: $PLATFORM)"
    PIDS=()
    NAMES=()
    if [ "$PLATFORM" = "ios" ] || [ "$PLATFORM" = "both" ]; then
        run_ios & PIDS+=($!); NAMES+=("ios")
    fi
    if [ "$PLATFORM" = "android" ] || [ "$PLATFORM" = "both" ]; then
        run_android & PIDS+=($!); NAMES+=("android")
    fi
    for i in "${!PIDS[@]}"; do
        if wait "${PIDS[$i]}"; then
            log "${NAMES[$i]}: success"
        else
            log "${NAMES[$i]}: FAILED"
            [ "${NAMES[$i]}" = "ios" ] && IOS_FAILED=1
            [ "${NAMES[$i]}" = "android" ] && ANDROID_FAILED=1
        fi
    done
else
    log "starting sequential coordinated build (platform: $PLATFORM)"
    if [ "$PLATFORM" = "ios" ] || [ "$PLATFORM" = "both" ]; then
        log "=== iOS build ==="
        if ! run_ios; then
            IOS_FAILED=1
            if [ "$PLATFORM" = "both" ]; then
                fail "iOS build failed; stopping coordinated build. Recovery: re-run ./scripts/build-ios.sh after fixing the cause (see openspec/specs/shared/build-coordination.md)."
            fi
        fi
    fi
    if [ "$PLATFORM" = "android" ] || [ "$PLATFORM" = "both" ]; then
        log "=== Android build ==="
        if ! run_android; then
            ANDROID_FAILED=1
            if [ "$PLATFORM" = "both" ]; then
                fail "Android build failed; stopping coordinated build. Recovery: re-run ./scripts/build-android.sh after fixing the cause (see openspec/specs/shared/build-coordination.md)."
            fi
        fi
    fi
fi

# ---------------------------------------------------------------------------
# Status reporting
# ---------------------------------------------------------------------------
END_TS="$(date +%s)"
TOTAL=$((END_TS - START_TS))
FINISHED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

# Read a field from a flat metrics JSON file (schema owned by this change).
metric_field() {
    # $1: file, $2: key
    [ -f "$1" ] || return 1
    sed -n "s/.*\"$2\":[[:space:]]*\"\{0,1\}\([^\",}]*\).*/\1/p" "$1" | head -1
}

report_platform() {
    # $1: label, $2: metrics file, $3: failed flag (0|1)
    # NOTE: JSON lines go to stdout (captured into the report file); human
    # status lines go to stderr so they never contaminate the JSON.
    local label="$1" file="$2" failed="$3"
    if [ "$failed" -eq 1 ] || ! [ -f "$file" ]; then
        printf '[build-coordination] report: %s: failed\n' "$label" >&2
        printf '  "%s": {"status": "failed"},\n' "$label"
        return
    fi
    local status duration finished
    status="$(metric_field "$file" status || echo failed)"
    duration="$(metric_field "$file" duration_seconds || echo 0)"
    finished="$(metric_field "$file" finished_at || echo unknown)"
    printf '[build-coordination] report: %s: %s (%ss, finished %s)\n' "$label" "$status" "$duration" "$finished" >&2
    printf '  "%s": {"status": "%s", "duration_seconds": %s, "finished_at": "%s"},\n' "$label" "$status" "$duration" "$finished"
}

OVERALL="success"
{ [ "$IOS_FAILED" -eq 1 ] || [ "$ANDROID_FAILED" -eq 1 ]; } && OVERALL="failed"

mkdir -p "$(dirname "$COORD_METRICS")"
{
    echo "{"
    printf '  "coordination": {"platforms": "%s", "parallel": %s, "status": "%s", "duration_seconds": %s },\n' \
        "$PLATFORM" "$PARALLEL" "$OVERALL" "$TOTAL"
    if [ "$PLATFORM" = "ios" ] || [ "$PLATFORM" = "both" ]; then
        report_platform "ios" "$IOS_METRICS" "$IOS_FAILED"
    fi
    if [ "$PLATFORM" = "android" ] || [ "$PLATFORM" = "both" ]; then
        report_platform "android" "$ANDROID_METRICS" "$ANDROID_FAILED"
    fi
    echo '  "finished_at": "'"$FINISHED_AT"'"'
    echo "}"
} > "$COORD_METRICS"

log "coordinated build finished in ${TOTAL}s (target < 900s sequential) — report: $COORD_METRICS"
if [ "$TOTAL" -gt 900 ] && [ "$OVERALL" = "success" ]; then
    log "WARNING: coordinated build took ${TOTAL}s, above the 15-minute target in openspec/specs/shared/build-coordination.md"
fi

[ "$OVERALL" = "success" ] || fail "coordinated build failed (overall status: $OVERALL)"
