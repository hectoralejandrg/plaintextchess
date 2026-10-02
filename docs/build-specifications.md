# Build Specifications

This document indexes and summarizes the build specifications for the plaintextchess project. The authoritative files are listed under "Spec Files"; this page is the quick-reference overview.

## Spec Files

| File | Capability | Covers |
| --- | --- | --- |
| [`openspec/specs/ios/build-specification.md`](../openspec/specs/ios/build-specification.md) | `ios` | iOS build configuration, XCFramework assembly, deployment (App Store / enterprise), performance targets |
| [`openspec/specs/android/build-specification.md`](../openspec/specs/android/build-specification.md) | `android` | Android build configuration, JNI libraries, deployment (Google Play / enterprise), performance targets |
| [`openspec/specs/shared/cross-platform-requirements.md`](../openspec/specs/shared/cross-platform-requirements.md) | `shared` | Unified configuration, environment variables, dependency management, consistency standards, validation criteria |
| [`openspec/specs/shared/build-coordination.md`](../openspec/specs/shared/build-coordination.md) | `shared` | Build orchestration (sequential/parallel), status reporting, error handling and recovery |

## Configuration Files

| File | Purpose |
| --- | --- |
| [`config/shared-build.yaml`](../config/shared-build.yaml) | Toolchain targets, shared environment variables, performance targets, required files |
| [`config/ios-build.yaml`](../config/ios-build.yaml) | iOS targets, deployment settings, UniFFi binding parameters, validation rules |
| [`config/android-build.yaml`](../config/android-build.yaml) | Android targets/ABIs, deployment settings, UniFFi binding parameters, validation rules |

Platform configurations extend the shared configuration: any value in `config/shared-build.yaml` applies to both platforms unless a platform file overrides it.

## Performance Targets

| Metric | iOS | Android | Coordinated |
| --- | --- | --- | --- |
| Clean build time | < 5 min | < 8 min | < 15 min total |
| Build memory usage | < 2 GB | < 4 GB | — |
| Artifact size | < 100 MB (IPA) | < 200 MB (APK/AAB) | — |

The build scripts report elapsed time and warn when a build exceeds its target.

## Related Specifications

- [`openspec/specs/ios/ui-spec.md`](../openspec/specs/ios/ui-spec.md) — iOS UI (consumes the built XCFramework)
- [`openspec/specs/android/ui-spec.md`](../openspec/specs/android/ui-spec.md) — Android UI (consumes the built JNI libraries)
- [`openspec/specs/shared/ffi-contracts.md`](../openspec/specs/shared/ffi-contracts.md) — FFI surface consumed by both platforms
- [`openspec/specs/rust-core/architecture.md`](../openspec/specs/rust-core/architecture.md) — Rust core architecture
