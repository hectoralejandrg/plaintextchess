# Tasks

## 1. Measure the per-move budget (gate)

- [x] 1.1 Instrument a representative load to count, per move, the inbound publishes, outbound publishes, WebSocket frames, and bytes, plus the transport's socket syscalls; verify the numbers against the code path (`deliver_outbound` per seat) and record the baseline in the design doc.
      Measured: 3,000 games, outbound_publishes=0.074/move, outbound_batches=0.003/move (down from 2.0/move pre-batching). See design.md "Gate result".
- [x] 1.2 Confirm from the measurement alone which cost dominates (outbound-per-seat fan-out vs inbound/WS writes); if the outbound fan-out is not material, stop and revisit the proposal before implementing D2-D4.
      Confirmed: outbound fan-out was the material cost and is now ~27x smaller; write cost (egress) is unchanged and matches `reduce-realtime-egress-cost`'s independent measurement.

## 2. Outbound batch envelope

- [x] 2.1 Add an internal `Batch { node_id, items }` outbound envelope (versioned like the existing envelopes) and have the transport dispatcher fan a batch out to each connection's `Outbox` in order; verify a batch decodes to the same per-connection messages (`batch_dispatches_to_each_connection`)
- [x] 2.2 Keep the single-item envelope working (rollout compatibility) and verify a single-item publish behaves exactly as today (`single_item_envelope_unchanged`)

## 3. Logic forwarder batching

- [x] 3.1 Group outbound messages for the same transport node within a small, bounded window (size/byte cap; a lone message is flushed immediately) and publish one batch; verify two seats on one node now cost one outbound fabric message (`two_seats_one_fabric_message`)
      Redesigned after a performance QA: publishes are sharded per node (keyed by room, so both seats coalesce) with backpressure instead of a single per-node task with a dropping channel. See design D3.
- [x] 3.2 Verify per-connection order and state-class coalescing are unchanged (`per_connection_order_preserved_under_batching`, reusing the `Outbox` unit tests)
- [x] 3.3 Verify move-ack p95 does not regress from batching on the load harness (`batching_does_not_regress_p95`).
      The first design regressed the per-node ceiling ~45% (single task serialization + `try_send` drops) per the performance QA; the sharded/backpressure redesign measures p95=67.68ms at 3,000 games, no regression. See design.md "Gate result".

## 4. Remove redundant serialization

- [x] 4.1 Forward the already-decoded inbound frame without re-encoding it (send the original frame payload so the logic decodes once), and verify the logic applies the identical message (`inbound_frame_decoded_once`)

## 5. Integration verification

- [x] 5.1 Verify the client wire protocol, message fields, and per-connection order are unchanged, and that the split suite passes (`cargo test --test split_e2e -- --ignored --test-threads=1`).
      Verified: unit coverage (`batch_dispatches_to_each_connection`, `single_item_envelope_unchanged`, `two_seats_one_fabric_message`, `per_connection_order_preserved_under_batching`, `inbound_frame_decoded_once`, plus the normalization/batcher tests) passes, and the ignored `split_e2e` suite passes with Redis (38/38).
- [x] 5.2 Re-run the load harness on one logic node and report the before/after: outbound fabric messages per move, transport socket syscalls, and per-node concurrent games at p95 = 100 ms.
      At 3,000 games: outbound_publishes=0.074/move (vs 2.0/move pre-batching), ws_socket_writes=2.000/move (unchanged, floor cost), p95=67.68ms (under the 100ms threshold). See design.md "Gate result".

## Workflow follow-up

- Archive the change after review, and verify the archived result and the updated `realtime-transport` spec (coalesced fan-out, no redundant re-serialization).
