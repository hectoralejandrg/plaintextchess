# Tasks

## 1. New capability: profile spec and design artifacts

- [x] 1.1 Create `proposal.md` (why new capability: persistent player profile tied to device id for ratings/sessions/history)
- [x] 1.2 Create `specs/player-auth/profiles/spec.md` (new capability delta: profile table, load at startup, create on first connect, survive restart, no FFI change, no wire change)
- [x] 1.3 Create `design.md` (reuses SQLite pool, new `profiles` table via `0002_profiles.sql`, `ProfileStore` adapter, profile creation in `handle_connect`, same D7 failure behavior: log + never block connection)
- [x] 1.4 Create `tasks.md` (capability definition tasks only — implementation awaits `/opsx-apply`)

## 2. Verification of proposed artifacts

- [x] 2.1 Verify proposal is coherent with the `add-sqlite-persistence` persistence layer (reuses DB, pool, device id, no new wire protocol)
- [x] 2.2 Verify spec uses the new-capability delta format (`## Purpose` + `## Requirements` with level-4 scenarios, no `ADDED` header that conflicts with main spec format)
- [x] 2.3 Verify `openspec validate add-player-auth-profiles` passes (exit 0)
- [x] 2.4 Verify the new capability does not conflict with existing `specs/rust-core/spec.md` or `specs/server/spec.md`
