# Tasks

## 1. Measure the per-move budget (gate)

- [ ] 1.1 Instrument a representative load to count, per move, the inbound publishes, outbound publishes, WebSocket frames, and bytes, plus the transport's socket syscalls; verify the numbers against the code path (`deliver_outbound` per seat) and record the baseline in the design doc
- [ ] 1.2 Confirm from the measurement alone which cost dominates (outbound-per-seat fan-out vs inbound/WS writes); if the outbound fan-out is not material, stop and revisit the proposal before implementing D2-D4

## 2. Outbound batch envelope

- [ ] 2.1 Add an internal `Batch { node_id, items }` outbound envelope (versioned like the existing envelopes) and have the transport dispatcher fan a batch out to each connection's `Outbox` in order; verify a batch decodes to the same per-connection messages (`batch_dispatches_to_each_connection`)
- [ ] 2.2 Keep the single-item envelope working (rollout compatibility) and verify a single-item publish behaves exactly as today (`single_item_envelope_unchanged`)

## 3. Logic forwarder batching

- [ ] 3.1 Group outbound messages for the same transport node within a small, bounded window (size/byte cap; a lone message is flushed immediately) and publish one batch; verify two seats on one node now cost one outbound fabric message (`two_seats_one_fabric_message`)
- [ ] 3.2 Verify per-connection order and state-class coalescing are unchanged (`per_connection_order_preserved_under_batching`, reusing the `Outbox` unit tests)
- [ ] 3.3 Verify move-ack p95 does not regress from batching on the load harness (`batching_does_not_regress_p95`)

## 4. Remove redundant serialization

- [ ] 4.1 Forward the already-decoded inbound frame without re-encoding it (send the original frame payload so the logic decodes once), and verify the logic applies the identical message (`inbound_frame_decoded_once`)

## 5. Integration verification

- [ ] 5.1 Verify the client wire protocol, message fields, and per-connection order are unchanged, and that the split suite passes (`cargo test --test split_e2e -- --ignored --test-threads=1`)
- [ ] 5.2 Re-run the load harness on one logic node and report the before/after: outbound fabric messages per move, transport socket syscalls, and per-node concurrent games at p95 = 100 ms

## Workflow follow-up

- Archive the change after review, and verify the archived result and the updated `realtime-transport` spec (coalesced fan-out, no redundant re-serialization).
