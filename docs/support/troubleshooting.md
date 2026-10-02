# Build Support & Troubleshooting

Support processes for the cross-platform build system: how to report problems,
how to troubleshoot the most common failures, and the maintenance schedule.

## 1. Reporting build problems

This repository has no external issue tracker configured (it is not a git
remote host project). Until one is set up, follow this convention:

1. **Reproduce** — run the failing script from a clean shell:
   ```bash
   source "$HOME/.cargo/env"
   ./scripts/build-ios.sh        # or build-android.sh / build-coordination.sh
   ```
2. **Capture context** (attach to the report):
   - `target/build-metrics/<platform>-last.json`
   - The last 50 lines of the build log
   - `cargo --version`, `rustup show`, `sw_vers` / `uname -a`
   - For Android: the NDK path printed in the log (`[build-android] using NDK: …`)
3. **Classify** the failure using the decision table in section 3.
4. **File the report** with: title = `<platform>: <one-line symptom>`, body =
   repro + context + classification. Keep one report per distinct root cause.

## 2. First-line checks (run in order)

```bash
# 1. Toolchain
source "$HOME/.cargo/env" && cargo --version && rustup show

# 2. Rust core compiles and tests pass
(cd core && cargo test)

# 3. In-project CLI present
ls core/target/debug/cargo-uniffi-bindgen

# 4. Platform validation scripts
./tests/validate-ios-build.sh
./tests/validate-android-build.sh
./tests/validate-cross-platform.sh

# 5. Metrics are valid JSON
python3 -m json.tool target/build-metrics/ios-last.json
python3 -m json.tool target/build-metrics/android-last.json
python3 -m json.tool target/build-metrics/coordination-last.json
```

If step 2 fails, the problem is in `core/` (Rust source), not the build
infrastructure. Everything else assumes the core crate compiles.

## 3. Failure decision table

| Symptom | Platform | Root cause | Resolution |
| --- | --- | --- | --- |
| `error[E0583]: file not found for module` | both | Missing source file in `core/src/` | Restore the module; run `cargo test` first. |
| `associated functions are not currently supported` | both | UniFFI export shape changed | Constructors are free functions returning `Arc<T>`; check `core/src/lib.rs`. |
| `cannot borrow data in an Arc as mutable` | both | `&mut self` FFI method | Use `Mutex` interior mutability; exported methods take `&self`. |
| `Crate chess-core not found in …` | both | `--crate` with a hyphen | Use `--crate chess_core` (crate identifier, not package name). |
| `failed to open file …libchess_core.a` | iOS | Binding generation ran before the release build, or `staticlib` missing | Restore script order (CLI → release build → generate) and keep `crate-type` containing `staticlib`. |
| `no such command: uniffi-bindgen` | both | cargo subcommand lookup | Call `./target/debug/cargo-uniffi-bindgen` directly. |
| `linking with cc failed` / `--version-script` | Android | Default Apple linker used for ELF | NDK clang must be the target linker (script sets `CARGO_TARGET_<TRIPLE>_LINKER`). |
| `unable to find library -lc/-llog/-lunwind` | Android | Bare `ld.lld` without Bionic sysroot | Use the NDK cross-clang as linker, not standalone lld. |
| `no Android NDK found` | Android | NDK missing/not discoverable | Install an NDK or set `ANDROID_NDK_HOME`. |
| `aarch64: unbound variable` | Android | bash 4+ syntax on macOS bash 3.2 | Keep scripts bash 3.2 compatible (no `declare -A`). |
| Malformed `coordination-last.json` | both | Log lines leaked into the JSON block | JSON is written on stdout of the redirected block; human logs must go to stderr. |
| Build "succeeds" but artifacts missing | both | Script skipped a step after an edit | Run the matching `tests/validate-*-build.sh`; it checks artifacts. |

## 4. Regular maintenance schedule

| Cadence | Action | Owner |
| --- | --- | --- |
| Every core-crate change | `cd core && cargo test` | developer |
| Every change to scripts/config/specs | All three `tests/validate-*.sh` + `openspec validate --all` | developer |
| Weekly | Clean full build: `rm -rf core/target ios/Frameworks target/uniffi target/build-metrics` then `./scripts/build-coordination.sh` | build owner |
| On rustup update (`rustup update`) | Re-run clean build + both platform scripts; note the new toolchain version in `docs/environment-setup.md` | build owner |
| On NDK upgrade | Point `ANDROID_NDK_HOME` at the new NDK, run `./scripts/build-android.sh`, verify ELF archs with `file` | build owner |
| Monthly | Review `target/build-metrics/*` for duration drift (> 50% vs. the last 5 runs); investigate regressions (see `build-metrics.md`) | build owner |
| Quarterly | Re-validate specs against reality: `openspec validate --all`, re-read the "Build Process" sections in `openspec/specs/*` vs. the actual scripts | tech lead |
