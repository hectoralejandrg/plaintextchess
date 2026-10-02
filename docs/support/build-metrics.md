# Build Metrics, Monitoring & Optimization

How build performance is measured in this repo, how to monitor it, and how to
optimize when it degrades.

## 1. Metrics files

Every build script writes a JSON metrics file on completion:

| File | Written by | Schema |
| --- | --- | --- |
| `target/build-metrics/ios-last.json` | `scripts/build-ios.sh` | see 2 |
| `target/build-metrics/android-last.json` | `scripts/build-android.sh` | see 2 |
| `target/build-metrics/coordination-last.json` | `scripts/build-coordination.sh` | see 3 |

### 2. Platform metrics schema

```json
{
  "platform": "ios",
  "environment": "production",
  "build_type": "release",
  "status": "success",
  "duration_seconds": 1,
  "finished_at": "2026-10-02T16:11:50Z"
}
```

| Field | Meaning |
| --- | --- |
| `platform` | `ios` or `android`. |
| `environment` | Value of `BUILD_ENVIRONMENT` (`development`/`production`). |
| `build_type` | Value of `BUILD_TYPE` (`debug`/`release`). |
| `status` | `success` (only written on success; failed runs exit non-zero without a new file). |
| `duration_seconds` | Wall-clock seconds from script start to artifacts assembled. |
| `finished_at` | UTC timestamp (ISO 8601). |

### 3. Coordination metrics schema

```json
{
  "coordination": {
    "platforms": "both",
    "parallel": 0,
    "status": "success",
    "duration_seconds": 1
  },
  "ios":     { "status": "success", "duration_seconds": 1, "finished_at": "…" },
  "android": { "status": "success", "duration_seconds": 0, "finished_at": "…" },
  "finished_at": "…"
}
```

`parallel` is `1` when `--parallel` was used (platforms built concurrently,
duration ≈ max of the two), `0` otherwise (sequential, duration ≈ sum).

## 2. Monitoring

### Baselines (from the specs)

| Build | Target |
| --- | --- |
| iOS clean build | < 300 s (5 min) |
| Android clean build | < 480 s (8 min) |
| Coordinated (sequential) | < 900 s (15 min) |

These targets live in `config/*.yaml` and the specs; the validators
(`tests/validate-*-build.sh`) fail if config and spec drift apart.

### Reading the data

The metrics files are "last run only" (they are overwritten each build). To
build a history, append to a log after each run:

```bash
jq -c . target/build-metrics/ios-last.json >> target/build-metrics/ios-history.jsonl
```

(`jq` is available on the build machine; Python works too: `python3 -c 'import json,sys; print(json.dumps(json.load(open(sys.argv[1]))))' target/build-metrics/ios-last.json >> …`)

Useful checks:

```python
# duration drift: last run vs. mean of previous 5
import json
rows = [json.loads(l) for l in open('target/build-metrics/ios-history.jsonl')]
prev, last = rows[-6:-1], rows[-1]
mean = sum(r['duration_seconds'] for r in prev) / len(prev)
print(f"last={last['duration_seconds']}s mean_prev={mean:.0f}s drift={ (last['duration_seconds']-mean)/mean:+.0% }")
```

### Alert threshold (recommended)

- Single platform run > 2× its baseline → investigate (see 3).
- Coordinated run > 900 s → investigate dependency or toolchain changes.
- Any `failed` status in the coordination report → open a support ticket per
  `docs/support/troubleshooting.md`.

## 3. Optimization playbook

Apply in this order; re-measure after each step:

1. **Isolate the slow stage.** A clean run shows which phase dominates:
   - CLI build (`cargo build --bin cargo-uniffi-bindgen`) — one-time; cached afterwards.
   - Core release build — dependency compilation dominates on the first run.
   - Binding generation — always < 1 s; ignore.
2. **Keep the target directories warm.** The dominant cost on repeat builds is
   re-linking; `rm -rf core/target` is the most common cause of slow builds.
   Only the weekly clean-build in the maintenance schedule should do this.
3. **Use `--parallel` for coordinated builds** on machines with ≥ 4 cores:
   `./scripts/build-coordination.sh --parallel`.
4. **Pin the toolchain.** Surprising rustc/dependency versions cause full
   rebuilds; `rust-toolchain.toml` (if added later) plus a committed
   `core/Cargo.lock` keep builds reproducible.
5. **Profile cargo** if a single crate regresses:
   `CARGO_LOG=cargo::core::compiler=debug cargo build …` or `cargo build -v`.
6. **Profile the core crate itself** if game-logic code grows:
   `cargo build --release` then `flamegraph`/`samply` on the test binary.

## 4. Updating the specifications as requirements evolve

- Any change to a baseline, target list, or artifact path must update **all** of:
  the spec (`openspec/specs/<platform>/build-specification.md`), the config
  (`config/<platform>-build.yaml`), the script, and the validator — the
  validators enforce consistency across these four artifacts.
- Propose the change as an OpenSpec change (`/opsx-propose`), implement it
  (`/opsx-apply`), then archive (`/opsx-archive`). This keeps the spec-driven
  flow the repo already uses.
