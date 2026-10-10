# Tasks

## 1. Placement

- [x] 1.1 In the create-room path of `transport_handler.rs`, select the target logic node by round-robin over the live node list (skipping nodes that do not accept), leaving single-node behavior identical; verify with a unit test of the selection function (`placement_round_robins_over_nodes`)
- [x] 1.2 Ensure the chosen node still records ownership before a join can route, and verify the ownership/failover tests pass unchanged (`owner_lost_while_active_is_reclaimed`, `failover_has_no_double_owner`)

## 2. Integration

- [x] 2.1 Verify with two logic nodes that many created rooms are distributed across both (test `rooms_distributed_across_logic_nodes`) and that joins/moves still reach the room's owner
- [x] 2.2 Verify the split suite still passes (`cargo test --test split_e2e -- --ignored --test-threads=1`)

## 3. Docs

- [x] 3.1 Document room placement in `docs/tiered-architecture.md` (new rooms are distributed across live logic nodes) and verify the documented behavior matches the code

## Workflow follow-up

- Archive the change after review, and verify the archived result and the updated `room-routing` spec (balanced placement).
