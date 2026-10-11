# Tasks

## 1. Measure the transport egress path (gate)

- [x] 1.1 Instrument the transport write path to report, per move and per connection, socket writes, bytes, serialization time, and blocked time; record the baseline on the reference hardware.
      Instrumented `ws_socket_writes`/`ws_serialize_ns`/`ws_write_ns` (transport `/healthz`) and added `server/examples/load_gen.rs`. Baseline recorded in design.md: 2 writes/move, ~374 B/move, serialize ~1-1.4 us/move, write ~52-62 us/move.
- [x] 1.2 Confirm from the measurement which component dominates the per-move cost (syscall count, byte copy, serialization, framing); if the write path is not material, stop and revisit the proposal (see D3).
      The socket write time dominates (~52-62 us/move vs ~1-1.4 us serialize), and the write path is material (~2.5 cores at 3k games) - so the change proceeds.
- [x] 1.3 Record whether the client load harness on the same host caps the measured ceiling, so the server ceiling is not misattributed.
      Yes: at 4,000 games the harness seated only 169 before setup failed, so the measurable ceiling (~3,300-3,500 games) is bounded by the load generator; the true server ceiling is a lower bound.

## 2. Coalesce a connection's egress writes

- [x] 2.1 Write a connection's ready frames with one flush per drained batch (feed all frames, then a single flush) instead of one flush per frame, and verify the client receives exactly the same frames in the same order (`frames_unchanged_under_coalescing`)
      Implemented in `write_loop` (feed + one `flush` per drained batch). The 39-test split e2e suite passes, so frames/order are unchanged.
- [x] 2.2 Verify state-class coalescing and close/keepalive behavior are unchanged (`outbox_semantics_unchanged`)
      `state_frames_coalesce_to_the_newest_...` and `control_frames_...` pass; split e2e passes.
- [x] 2.3 Verify the flush-coalescing change is behavior-preserving and does not regress p95 (`egress_flushes_do_not_regress_p95`).
      p95 does not regress (1000/3000 games: 17.3/77.5 ms vs baseline 18.9/78.2). A flush *reduction* could not be observed: frames/move == flushes/move == 2.0, because each connection has one ready frame per move and `Update` is state-class, so multi-frame bursts do not arise. The mechanism (one flush per drained batch) is implemented; see design.md.

## 3. Fallback: colocate the room hot path (decided; split into a new change)

- [x] 3.1 Decide from group 1 whether the egress fix is sufficient. Decision: **not sufficient** - the per-move cost is the inherent per-connection socket write, which frame coalescing cannot remove. The follow-on design lives in the new change `colocate-room-hot-path`.
- [x] 3.2 Verify the client protocol and per-connection order are unchanged under colocation. N/A here: this change implements no colocation; protocol/order invariance is verified for the flush change by the split suite, and colocation's own verification is in `colocate-room-hot-path`.

## 4. Integration verification

- [x] 4.1 Verify the client wire protocol, message fields, and per-connection order are unchanged, and that the split suite passes (`cargo test --test split_e2e -- --ignored --test-threads=1`).
      Verified: the split e2e suite passes (39/39 with Redis) after the flush-coalescing change.
- [x] 4.2 Re-run the load harness on one logic node and report the before/after: socket writes per move, bytes per move, move-ack p95, and per-node concurrent games at p95 = 100 ms.
      Report in design.md "After the flush coalescing": p95 1000/3000 games 18.9/78.2 -> 17.3/77.5 ms; frames/move == flushes/move == 2.0; bytes 374/move. Neutral (no regression, no flush reduction); the crossing is harness-bounded (~3.3k-3.5k games).

## Workflow follow-up

- Archive the change after review, and verify the archived result and the updated `realtime-transport` spec (bounded egress cost, unchanged frames).
