# Design

## Context

See proposal.md - Why. Today the iOS game screen (`ContentView.swift`, the `GameView` struct) and the Android one (`MainActivity.kt`, `GameScreen`) are a vertical stack: title, "You are X" label, status row, optional online clock row, optional waiting banner, board, a vertical move list, and a controls row. Both view models (`GameViewModel.swift` / `.kt`) already hold the board, status, side to move, and full `moveList`; online games replay server moves into a mirror core session. The specs that shape this are `ios`/`android` "Playable Game Screen" and the online clock/controls requirements.

## Goals / Non-Goals

**Goals:**
- A denser, information-rich game layout on both platforms: top bar, horizontal move strip, player rows (name/rating/clock), board, bottom action bar.
- Move-history browsing: any earlier position can be shown on the board and returned from.
- One shared shape on iOS and Android; works for online and local/CPU.

**Non-Goals:**
- Chat (no server support), server/protocol/persistence changes.
- Changing clock or rating *semantics* (only where they are displayed).
- Changing the board widget, drag & drop, promotion picker, or the rules (still the core).

## Decisions

- **D1 — A `viewPly` navigation state.** Both view models gain `viewPly: Int?` where `nil` means "live / last ply". The board renders the position at `viewPly`; every other input path checks it first and no-ops while a past ply is shown.
- **D2 — Per-ply board snapshots.** As each move is applied (local play, undo, and each replayed online move/update), store the resulting board snapshot in an array indexed by ply. Navigating reads the cached snapshot in O(1) instead of replaying the core session. If a snapshot is missing (e.g., after undo trims the tail) the cache is rebuilt by replaying the move list into a scratch session once. Alternatives considered: replay-from-scratch per navigation (rejected: O(n) per tap) and keeping a second always-current FEN (rejected: same cost).
- **D3 — Browsing is local and non-blocking.** While `viewPly` is not live, move input is disabled, the strip highlights the viewed ply, and a not-live affordance plus a "jump to live" control are shown. `next`/`previous` and tapping a ply change `viewPly`; reaching the last ply returns to live. New moves (a local/CPU reply or a server update) are appended to the strip and the snapshot cache and do **not** move `viewPly` unless it was already live.
- **D4 — Layout composition.** Top bar: the time-control label and an overflow menu (New game, Flip board, and — local/CPU — Undo). Move strip: a horizontally scrollable list of plies, current ply highlighted. Opponent row (above the board) and player row (below): the side's name and rating (online) or a generic label (local/CPU: "You", "CPU", "Opponent"), plus that side's clock when a time control exists. Bottom action bar: Resign and the history controls (previous, jump to live/last, next). The existing `ControlsView`/control row is folded into the overflow menu and the action bar.
- **D5 — Player identity per mode.** Online: usernames/display names and the server-provided ratings, with clocks from the existing authoritative clock interpolation. Local/CPU: generic labels, no rating, and no clock (those games have no time control). The board orientation continues to follow the player's own color (online) or White (local/CPU).

## Risks / Trade-offs

- [Snapshot cache memory] → one small FEN/snapshot per ply; bounded by game length, negligible for a chess game.
- [Browsing while the opponent moves online] → D3 keeps the viewed ply stable and appends; the not-live indicator makes the state explicit.
- [Undo interacts with history] → Undo trims the snapshot cache to the new length; if browsing, return to live first or clamp `viewPly`.
- [Two platforms drifting] → keep the same structure and the same named controls on both; the platform specs already run in parallel.

## Migration Plan

1. Add the navigation state and per-ply snapshots to both view models with tests, before touching the layout.
2. Rebuild the game screen layout on each platform around the shared shape.
3. Wire the move strip and action bar to the navigation state.
4. Rollback: the layout is a view-only change; reverting restores the previous stack without data migration.

## Open Questions

- None that change the specs, the approach, or the task breakdown.
