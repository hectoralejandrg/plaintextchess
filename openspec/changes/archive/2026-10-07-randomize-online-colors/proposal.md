# Proposal

## Why

Every online game currently fixes the colors by role: the player who creates the room always plays White and the player who joins always plays Black. That is predictable and lets a player always choose the side with the initiative (White moves first) simply by creating the room. Chess servers normally assign sides randomly so neither player can pick the first move.

## What Changes

- The server assigns the creator a **random color** when the room is created; the second player takes the other color. The color is already carried per seat in every state snapshot (`your_color`), so the wire protocol is unchanged.
- The creator learns its color immediately in the `room_ready` snapshot; the joiner learns the complementary color when the game starts. White still moves first.
- Both clients already read `your_color` and orient the board to the player's own color, so no client protocol change is required; only the documented behavior and the server's seat assignment change.
- Determinism for tests: the color choice becomes injectable (fixed for existing deterministic tests, random in production), with a dedicated test that exercises the random path and asserts both colors occur.

## Capabilities

### New Capabilities

- None.

### Modified Capabilities

- `server`: the room-management requirement changes from "creator takes White, joiner takes Black" to random colors assigned at creation (the joiner takes the other seat).
- `ios`: the online-multiplayer join scenario changes from "the creator plays White and the player plays Black" to the player's color being whichever the server assigned, with the board rendered and oriented from `your_color`.
- `android`: same change as `ios`.

## Impact

- **Server**: `server/src/application/room_actor.rs` seat assignment (`handle_connect` creator/joiner branches, `started()`, the hijack guard), and the injectable color choice.
- **Tests**: room/actor/clock/auth/persistence integration tests that assume the creator is White need a deterministic path; a new test covers the random assignment.
- **Clients**: no functional change (they already use `your_color`); the DEBUG online scripts that assume creator=White may need to read the assigned color.
- **Specs**: `server`, `ios`, `android` requirements above.
- No wire-protocol, database, or rating changes.
