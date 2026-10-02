# iOS Build Training

Training material for the iOS cross-platform build setup of `plaintextchess`.
Target audience: developers who need to build, validate, and troubleshoot the iOS
Rust artifacts (`ChessCore.xcframework` + UniFFi Swift bindings).

## 1. Concepts

| Concept | What it means here |
| --- | --- |
| `chess-core` | The Rust crate in `core/` containing all game logic (board, moves, rating). |
| UniFFI (library mode) | The FFI framework. No UDL files: the surface is declared with `#[uniffi::export]` / `#[derive(uniffi::Object)]` in `core/src/lib.rs`, and metadata is extracted from the **compiled** library. |
| XCFramework | `ios/Frameworks/ChessCore.xcframework` containing the staticlib `libchess_core.a` + `Info.plist`. |
| Swift bindings | Generated into `target/uniffi/ios/` (`chess_core.swift`, `chess_coreFFI.h`, `chess_coreFFI.modulemap`). |
| In-project CLI | `core/src/bin/cargo-uniffi-bindgen.rs` — the UniFFI 0.28 CLI as a cargo binary target (uniffi does not publish a standalone CLI crate). |

## 2. Prerequisites (checklist)

```bash
source "$HOME/.cargo/env"
cargo --version                          # stable toolchain
rustup target list | grep aarch64-apple-ios
ls /opt/homebrew/bin/ld.lld 2>/dev/null  # not required for iOS, only Android
```

No Xcode project files are required for the Rust artifacts; Xcode 14+ is only
needed to consume the XCFramework in an app.

## 3. Building from scratch

```bash
./scripts/build-ios.sh
```

Expected steps (in order):

1. Validate `openspec/specs/ios/build-specification.md`, `config/ios-build.yaml`, `core/Cargo.toml`.
2. `cargo build --bin cargo-uniffi-bindgen` (host, debug).
3. `cargo build --release --target aarch64-apple-ios` (cdylib + staticlib).
4. `./target/debug/cargo-uniffi-bindgen generate --library --crate chess_core --language swift ...`
5. Assemble `ios/Frameworks/ChessCore.xcframework/`.
6. Write `target/build-metrics/ios-last.json`.

On a clean machine the first run compiles the whole dependency tree
(≈ 2–5 min). Cached runs finish in seconds.

## 4. Exercise: manual build and inspection

```bash
cd core
cargo build --bin cargo-uniffi-bindgen
cargo build --release --target aarch64-apple-ios
./target/debug/cargo-uniffi-bindgen generate \
    --library --crate chess_core --language swift \
    --out-dir ../target/uniffi/ios/ \
    target/aarch64-apple-ios/release/libchess_core.a
```

Then verify:

```bash
python3 -m json.tool target/build-metrics/ios-last.json   # after running the script
file ios/Frameworks/ChessCore.xcframework/ios-aarch64/libchess_core.a
head -5 target/uniffi/ios/chess_core.swift
./tests/validate-ios-build.sh
```

## 5. Exercise: debug/development builds

```bash
BUILD_ENVIRONMENT=development BUILD_TYPE=debug ./scripts/build-ios.sh
```

All three environment variables (`BUILD_ENVIRONMENT`, `BUILD_TYPE`,
`PLATFORM_TARGET`) are honored; the defaults are `production`/`release`/`ios`.

## 6. Common mistakes

| Symptom | Cause | Fix |
| --- | --- | --- |
| `no such command: uniffi-bindgen` | Tried the cargo-subcommand form. Cargo passes the subcommand name as an argument and the target-dir lookup is unreliable. | The scripts call the binary directly: `./target/debug/cargo-uniffi-bindgen generate ...`. |
| `Crate chess-core not found in ...libchess_core.a` | Used `--crate chess-core` (hyphen). The metadata stores the crate identifier with an underscore. | Use `--crate chess_core`. |
| `failed to open file ...libchess_core.a` | Build before the compile step, or `staticlib` missing from `crate-type`. | Order: build CLI → release build → generate. Keep `crate-type = ["rlib", "cdylib", "staticlib"]`. |
| `associated functions are not currently supported` | Constructor declared inside the impl block without the right pattern. | Use a free function returning `Arc<GameSession>` (`new_game_session`). |
| `cannot borrow data in an Arc as mutable` | `&mut self` methods exported through the FFI. | UniFFI objects are shared via `Arc`; use `Mutex` interior mutability and `&self` methods. |

## 7. Self-check

After completing this training you should be able to:

1. Run `./scripts/build-ios.sh` and read its output.
2. Locate and inspect the three generated Swift files.
3. Read `target/build-metrics/ios-last.json` and explain each field.
4. Run `./tests/validate-ios-build.sh` and interpret a failing check.
5. Explain why the bindings are generated from the compiled staticlib and not from source.
