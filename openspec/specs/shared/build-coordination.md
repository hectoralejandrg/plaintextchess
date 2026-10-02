# Build Coordination Specification

## Overview

This document defines how iOS and Android builds are orchestrated together: execution order, parallelism, status reporting, and error handling. It governs `scripts/build-coordination.sh` and the shared build metrics. The delta requirements for this capability were introduced by the `setup-cross-platform-build` change.

## Build Orchestration

### Default Execution Order (sequential)
1. Pre-flight checks for both platforms (spec + config files, toolchain availability).
2. **iOS build** — `scripts/build-ios.sh`
3. **Android build** — `scripts/build-android.sh`

Sequential execution is the default because both builds share the same machine resources (CPU, disk, `~/.cargo`) and the Rust target cache.

### Parallel Execution
- `scripts/build-coordination.sh --parallel` runs both platform builds concurrently and waits for both to finish.
- Parallel mode is intended for CI runners or machines with ≥ 4 cores; it MUST NOT be used when only one Rust target cache exists and disk is constrained (documented trade-off).

### Platform Selection
- `--platform ios` / `--platform android`: run a single platform build (useful for CI matrix jobs).
- `--platform both` (default): run both.

## Build Status Reporting

### Per-Platform Reports
Each platform script writes `target/build-metrics/<platform>-last.json`:
```json
{
  "platform": "ios",
  "environment": "production",
  "build_type": "release",
  "status": "success",
  "duration_seconds": 180,
  "finished_at": "2026-10-01T10:00:00Z"
}
```
- `status` is one of `success` or `failed`.
- The script also prints a human-readable summary of artifacts and timings to stdout.

### Coordinated Report
After all selected platforms finish, `scripts/build-coordination.sh` writes `target/build-metrics/coordination-last.json` containing:
- Per-platform `status`, `duration_seconds`, and `finished_at` (copied from the platform metrics files).
- Overall `status`: `success` when every selected platform succeeded, `failed` otherwise.
- Total wall-clock `duration_seconds` of the coordinated run.

## Error Handling and Recovery

### Failure Behavior
- **Sequential mode:** the run stops at the first failing platform. The coordinating script prints which platform failed, the recorded status, and the recovery steps below, then exits non-zero.
- **Parallel mode:** both builds run to completion; the coordinator reports every failure and exits non-zero if any platform failed.

### Recovery Procedures
1. **Missing toolchain** (e.g. `cargo` not installed or a target not installed): the platform script fails fast with `[ERROR]` pointing to `docs/environment-setup.md`; recover by following the setup guide, then re-run.
2. **Stale/failed artifacts:** re-run the failing platform script (builds are idempotent); use `cargo clean` only when the Rust cache is suspected corrupted.
3. **Configuration drift:** run `tests/validate-cross-platform.sh` to detect which shared value diverged; fix the config file, not the script.
4. **Coordinated run interrupted:** re-run with the same flags; each platform build is independent and safe to restart.

## Coordinated Performance Target

| Metric | Target |
| --- | --- |
| Total coordinated build duration (sequential, both platforms) | < 15 minutes |

The coordinator reports the total elapsed time in the coordinated report so regressions are visible.
