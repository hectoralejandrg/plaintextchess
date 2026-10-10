# Tasks

## 1. Measure the transport egress path (gate)

- [ ] 1.1 Instrument the transport write path to report, per move and per connection, socket writes, bytes, serialization time, and blocked time; record the baseline on the reference hardware
- [ ] 1.2 Confirm from the measurement which component dominates the per-move cost (syscall count, byte copy, serialization, framing); if the write path is not material, stop and revisit the proposal (see D3)
- [ ] 1.3 Record whether the client load harness on the same host caps the measured ceiling, so the server ceiling is not misattributed

## 2. Coalesce a connection's egress writes

- [ ] 2.1 Write a connection's ready frames in one scatter-gather write instead of one `send` per frame, and verify the client receives exactly the same frames in the same order (`frames_unchanged_under_coalescing`)
- [ ] 2.2 Verify state-class coalescing and close/keepalive behavior are unchanged (`outbox_semantics_unchanged`)
- [ ] 2.3 Verify socket writes per move drop and p95 does not regress (`egress_writes_do_not_regress_p95`)

## 3. Fallback: colocate the room hot path (only if the gate requires it)

- [ ] 3.1 Decide from group 1 whether the egress fix is sufficient; if not, design sticky per-room placement so a room's two seats share a transport+logic node
- [ ] 3.2 Verify the client protocol and per-connection order are unchanged under colocation

## 4. Integration verification

- [ ] 4.1 Verify the client wire protocol, message fields, and per-connection order are unchanged, and that the split suite passes (`cargo test --test split_e2e -- --ignored --test-threads=1`)
- [ ] 4.2 Re-run the load harness on one logic node and report the before/after: socket writes per move, bytes per move, move-ack p95, and per-node concurrent games at p95 = 100 ms

## Workflow follow-up

- Archive the change after review, and verify the archived result and the updated `realtime-transport` spec (bounded egress cost, unchanged frames).
