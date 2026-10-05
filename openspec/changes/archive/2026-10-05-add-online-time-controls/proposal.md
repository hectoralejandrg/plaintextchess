# Proposal

## Why

Online games currently have no time pressure: a connected player can sit on
their turn forever (the only ending is disconnect → 120 s forfeit). Every
real online chess experience offers time controls (rapid/blitz/bullet), and
the player base expects a clock on the board. The clock is also the most
time-sensitive, stateful feature the server can have (deadline timers,
in-flight moves, reconnect with time remaining), and the current `server/`
crate is a flat set of modules that mixes transport, protocol mapping,
room bookkeeping, and authority in one layer — adding a clock there without
structure would entangle timing logic with socket handling. So this change
first restructures the server into clean-architecture layers (behavior
preserving) and then implements server-authoritative Fischer time controls
on top.

## What Changes

- **Server — clean architecture (foundation, behavior-preserving):**
  `server/` is reorganized into layered modules — `domain` (room, seat,
  time control, clock policy), `application` (use cases / room actor),
  `infrastructure` (axum, tokio timers, config), `interface` (protocol
  DTOs and mapping to the domain) — with ports as traits and a strict
  dependency direction (domain depends on nothing; upper layers may depend
  on lower ones). All existing behavior and the pinned wire form stay
  intact; the 44-test suite stays green.
- **Server — Fischer time controls:** room creation now carries a time
  control chosen from presets (15+10, 10+0, 5+0, 3+2, 1+0; Fischer
  increment — added on move completion). The server is the authority for
  time: snapshots carry both players' remaining time; when the side to
  move's time runs out the game ends by **flag fall** (`timed_out` with
  the connected player as winner — except a time draw when the winner has
  insufficient material to checkmate). A move in flight is honored when
  applied before the flag; a reconnecting player resumes with their
  remaining time, and a flag that fell while they were disconnected is
  settled on re-attach or when the disconnect grace expires (whichever
  comes first, connected player wins). Flag-fall updates ratings like a
  forfeit (1.0/0.0); a time draw splits 0.5/0.5.
- **Wire protocol (additive, still `v:1`):** `create_room` carries
  `time_control`; `room_ready`/`state` carry `time_control`,
  `white_time_ms`, `black_time_ms`; `Status` gains `timed_out(winner)`.
  The pinned wire-form test and both client codecs update accordingly.
- **iOS + Android (mirror):** create flow gains the time-control presets
  (creator picks; the joiner sees the chosen control in the lobby and may
  leave instead of accepting); a live clock UI shows both players'
  remaining time with the active clock highlighted (time is snapshot-
  authoritative with display-only local interpolation between
  snapshots); the online game-end modal gains the "on time" wordings
  ("You won on time." / "You lost on time.", "Draw." on the
  insufficient-material case) with the updated-ratings footer.
- **Out of scope:** clocks in local two-player or CPU modes (follow-up),
  Köhler/custom time controls, time-control negotiation in the lobby,
  turn timers that pause the opponent's clock.

## Capabilities

### New Capabilities

(none — the clock is a requirement-level extension of the existing
`server`, `shared`, `ios`, and `android` capabilities; the architecture
restructure is a design-level change with no behavior contract of its own)

### Modified Capabilities

- `server`: Room Management (creation carries a time control; joiner is
  told of it), Game Authority (server-authoritative clock, flag-fall
  endings, in-flight move rule), Disconnect/Reconnect/Forfeit (re-attach
  resumes with remaining time; deferred flag-fall while disconnected),
  Online Rating (flag-fall and time-draw results update ratings).
- `shared`: Online Multiplayer Protocol (additive `time_control` /
  `white_time_ms` / `black_time_ms` fields, `timed_out` status variant,
  time in every state snapshot).
- `ios`: Game Mode Selection (time-control choice at create), Game-End
  Dialog (on-time wordings), and a new Online Clock UI requirement
  (clock display, active-clock highlight, snapshot-driven time).
- `android`: same as `ios` (mirror).

## Impact

- `server/` — full restructure (all modules move into layered packages)
  plus the new clock/time-control code; `tests/` reorganized with the
  pinned wire-form test updated for the additive fields; new clock unit
  and integration tests (flag fall, in-flight, reconnect-with-time,
  time draw).
- `ios/app/PlainTextChess/` — `OnlineProtocol.swift` (codec + 12 tests),
  `GameViewModel.swift` (time state, on-time messages, clock model),
  `ContentView.swift` (clock UI + create-flow presets).
- `android/app/...` — `OnlineProtocol.kt` (codec + 15 tests),
  `GameViewModel.kt`, `MainActivity.kt` (mirror of the iOS changes).
- Local two-player/CPU modes, the core FFI surface, the 120 s
  disconnect-grace semantics, and the CI `build-server` job are
  unaffected (the job compiles/tests the restructured crate unchanged).
- E2E: new timed cross-platform runs (a preset game with clocks visible on
  both clients, and a flag-fall run on the shortest preset).
