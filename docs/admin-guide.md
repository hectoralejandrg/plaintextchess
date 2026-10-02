# Admin Guide: Build Operations

Operations documentation for managing the cross-platform build system: CI, quality metrics, configuration ownership, and maintenance.

## Configuration Ownership

| File | Owner | Change procedure |
| --- | --- | --- |
| `config/shared-build.yaml` | Build owner | Requires review; every target/variable here applies to both platforms |
| `config/ios-build.yaml` | iOS owner | Must keep `build_targets` a subset of shared targets |
| `config/android-build.yaml` | Android owner | Must keep `build_targets` a subset of shared targets |

Configuration drift is detected automatically by `./tests/validate-cross-platform.sh` (and in CI by the `validate` job). Never edit a script to compensate for a config problem — fix the config.

## CI/CD

The pipeline lives in `.github/workflows/build-validation.yml`:

| Job | Runner | Purpose |
| --- | --- | --- |
| `validate` | ubuntu-latest | Runs all three validation test scripts |
| `build-ios` | macos-latest | Runs `./scripts/build-ios.sh`, uploads XCFramework + metrics |
| `build-android` | ubuntu-latest | Runs `./scripts/build-android.sh`, uploads jniLibs + metrics |
| `quality` | ubuntu-latest | Downloads both metric files, prints a quality report, fails on `status: failed` |

Triggered on push to `main`, on PRs, and manually. Concurrency cancels superseded runs per ref.

### CI Expectations
- The `validate` job must pass before any build job runs (it is a prerequisite of the workflow graph by job ordering; gate merges on it).
- Build jobs report elapsed time; compare against the performance targets in `config/shared-build.yaml` (5 min iOS, 8 min Android).

## Build Quality Metrics

Every build writes a metrics file (schema in `docs/support/build-metrics.md`):

- `target/build-metrics/ios-last.json`
- `target/build-metrics/android-last.json`
- `target/build-metrics/coordination-last.json` (written by `scripts/build-coordination.sh`)

The CI `quality` job publishes these as the `build-quality-report` artifact. Review the report after each pipeline run; sustained regressions against the targets should be tracked as issues (see maintenance schedule).

## Release Operations

| Step | iOS | Android |
| --- | --- | --- |
| 1. Coordinated release build | `BUILD_ENVIRONMENT=production BUILD_TYPE=release ./scripts/build-coordination.sh` | same command |
| 2. Validate | `./tests/validate-cross-platform.sh` | same |
| 3. Package | `xcodebuild archive` + export IPA from the Xcode project | `gradle :app:bundleRelease` |
| 4. Sign | App Store / enterprise profile | Play signing key / internal keystore |
| 5. Distribute | App Store Connect | Play Console (AAB) / internal track (APK) |

## Error Handling Summary

| Situation | Behavior | Recovery |
| --- | --- | --- |
| Missing spec/config file | Script fails fast before compiling | Restore the file from VCS; run the validator |
| Missing toolchain | `[ERROR] cargo not found…` | `docs/environment-setup.md`; re-run |
| iOS build fails (sequential) | Coordinated run stops | Fix cause, re-run `./scripts/build-ios.sh` |
| Both fail (parallel) | All failures reported, non-zero exit | Fix each; re-run with `--parallel` |
| Config drift | Validator / CI `validate` job fails | `docs/support/troubleshooting.md` § Config drift |

Full failure catalogue: [Troubleshooting](./support/troubleshooting.md).
