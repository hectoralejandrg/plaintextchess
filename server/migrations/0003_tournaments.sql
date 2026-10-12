-- Arena tournaments (add-tournaments phase 1, server-only): the tournament
-- itself, a per-tournament points leaderboard separate from Glicko-2 rating,
-- and the join table attributing a game to its tournament. Additive: nothing
-- in 0001/0002 is altered.

CREATE TABLE tournaments (
    tournament_id TEXT PRIMARY KEY,   -- UUIDv4, server-generated
    time_control  TEXT NOT NULL,      -- wire label, e.g. "5+0"
    starts_at_ms  INTEGER NOT NULL,
    duration_ms   INTEGER NOT NULL,
    status        TEXT NOT NULL       -- 'scheduled' | 'running' | 'finished'
);

-- Points are doubled (win=2, draw=1, loss=0) so they stay integer.
CREATE TABLE tournament_standings (
    tournament_id TEXT NOT NULL REFERENCES tournaments (tournament_id),
    device_id     TEXT NOT NULL,
    points        INTEGER NOT NULL DEFAULT 0,
    games_played  INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (tournament_id, device_id)
);

-- `game_id` is a freshly generated id, not a foreign key into `games`: a
-- tournament game deliberately never writes a `games` row (that table is the
-- Glicko-2/casual-game path; tournament points are tracked separately, by
-- design, in `tournament_standings`).
CREATE TABLE tournament_games (
    game_id       TEXT PRIMARY KEY,
    tournament_id TEXT NOT NULL REFERENCES tournaments (tournament_id),
    white_device  TEXT NOT NULL,
    black_device  TEXT NOT NULL
);
