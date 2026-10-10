# Design

## Context

See `proposal.md` - Why. In `transport_handler.rs`, a frame with no room owner
(a `create_room`) builds a candidate list from `broker.logic_nodes()` and probes
them in order; the first live node accepts, so all new rooms land on it. Joins
and room-scoped frames are unaffected (they already route to the recorded
owner). Placement is only the new-room decision.

## Goals / Non-Goals

**Goals:**
- Distribute new rooms across live logic instances so M instances give ~M x
  capacity.
- Keep the ownership record, one-owner guarantee, and failover unchanged.
- Leave single-instance and client behavior identical.

**Non-Goals:**
- Re-balancing existing rooms (a room stays where it is created).
- A global load metric or controller (start with a cheap local strategy).
- Changing routing of joins/moves (owner-based, unchanged).

## Decisions

### D1 - Round-robin over the live node list
For a create-room frame, pick the next live node in round-robin order using a
per-transport-process counter. It is O(1), needs no extra Redis reads beyond the
node list already fetched for the fallback path, and balances well when the
node set is stable. Alternative: least-loaded by counting `ptc.room_owner.*` per
node - more accurate but adds Redis reads per create and a race; recorded as an
open question, to adopt only if round-robin proves imbalanced.

### D2 - Placement only on the create path
The selection change lives where a frame has no owner (create). A node that
accepts the frame records ownership as today, so joins and moves keep routing to
the owner and no new channel or state is introduced.

### D3 - Skip nodes that cannot take work
Only live nodes (present in `logic_nodes()` and accepting a publish) are
eligible; a stale entry is skipped as it is today, so placement never sends a
new room to a dead instance.

### D4 - Single node unchanged
With one live node, round-robin degrades to always choosing it - identical to
today, so existing deployments and tests are unaffected.

## Risks / Trade-offs

- [Round-robin is blind to node load] -> adequate while nodes are homogeneous
  and rooms are similar; least-loaded is a follow-up if not.
- [Node set order is not stable] -> round-robin over an unstable order still
  balances statistically; the goal is distribution, not exact parity.
- [Placement races with failover] -> ownership is still claimed by the chosen
  node with the existing NX/reclaim path; placement only picks a candidate.

## Open Questions

- Round-robin vs least-loaded (a per-node room count) if distribution is uneven
  in practice.
- Whether multiple transports need a shared counter to smooth placement further.
