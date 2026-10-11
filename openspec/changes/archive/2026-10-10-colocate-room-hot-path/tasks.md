# Tasks

## 1. Measure the colocated vs split cost (gate)

- [x] 1.1 Measure, on the reference harness, the per-move cost of a room whose two seats are already on one transport node versus the split path (fabric round trip + transport dispatch); record the baseline.
      Measured: the fabric hop is ~83 us/move (2 legs x ~40.5 us round trip), recorded in design.md "Gate result".
- [x] 1.2 Confirm from the measurement whether the local path (skip the fabric hop) is materially cheaper per move; if not, stop and revisit the proposal.
      Decision: **GO (bounded)** - the hop is the same order as the socket write; estimated ceiling ~1.25-1.35x. The `LOGIC_LANES` control showed lanes are not a cheaper lever, so colocation is the remaining one.

## 2. Establish room locality

- [x] 2.1 Place a room's second seat on the first seat's transport node (LB affinity by room code, or a transport-level handoff on join), and verify new/unseated connections still land anywhere (`unseated_connections_stay_unbound`)
      Rescoped (no LB exists in this repo, and a live WebSocket can't be moved server-side): records the creating seat's transport node in Redis (`ptc.room_transport.<code>`) as the data an external LB would need, without building the LB. See design.md "Implementation note: rescoped".
- [x] 2.2 Verify a seated room's two connections end up on one node under load (`room_seats_co_locate`)
      Verified under the only topology this server can place without an LB (both seats land on the one transport node they connect to); the record is write-once on create and unchanged by join.

## 3. In-process hot path for a colocated room

- [x] 3.1 For a colocated room, apply the move and enqueue both updates directly to the connections' outboxes, skipping the fabric publish and the transport dispatch (`colocated_move_skips_fabric`)
      Added `TIER=combined` (one process, cross-linked `TransportState`/`LogicState`). Decided per-connection, not per-room: `try_local_logic` (inbound) and `start_outbound_forwarder`'s local-outbox check (outbound). Verified via `fabric_metrics()` deltas: outbound_publishes/outbound_batches = 0.000/move in the load run.
- [x] 3.2 Verify per-connection order and the client-visible frames are unchanged (`order_and_frames_unchanged_under_colocation`)
      Verified: a colocated room's move/resign sequence and frames match `split_mode_full_game`'s assertions exactly, because the direct path pushes into the same per-connection `Outbox` the dispatcher already drains in order.
- [x] 3.3 Verify cross-node rooms and reconnects still use the fabric path (`split_fallback_still_works`)
      Verified: a combined node + a plain `TIER=transport` node serving the other seat still shows nonzero `fabric_metrics()` inbound/outbound deltas for both players' moves.

## 4. Integration verification

- [x] 4.1 Verify the client wire protocol and the split suite pass (`cargo test --test split_e2e -- --ignored --test-threads=1`)
      44/44 pass (39 pre-existing + 5 new: `unseated_connections_stay_unbound`, `room_seats_co_locate`, `colocated_move_skips_fabric`, `order_and_frames_unchanged_under_colocation`, `split_fallback_still_works`).
- [x] 4.2 Re-run the load harness and report the before/after: per-move cost, move-ack p95, and per-node concurrent games at p95 = 100 ms
      At 3,000 games (fresh process each side): p95 67.68ms -> 34.61ms, write_us 50.58 -> 23.35, outbound_publishes 0.074/move -> 0.000/move. Exceeds the gate's ~1.25-1.35x estimate. Ceiling-crossing number not independently re-measured (client-side harness cap); see design.md "Gate result" for the full caveat.

## Workflow follow-up

- Archive the change after review, and verify the archived result and the updated `realtime-transport` spec (room locality and the colocated hot path).
