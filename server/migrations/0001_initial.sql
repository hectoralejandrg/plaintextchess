-- Initial schema (add-sqlite-persistence D4/D5): per-device Glicko-2
-- state, finished games, and per-game rating history. Table and SQL
-- choices stay standard so the later Postgres cutover is a driver +
-- URL change, not a rewrite.

CREATE TABLE players (
    device_id        TEXT PRIMARY KEY,
    rating           REAL NOT NULL,
    rating_deviation REAL NOT NULL,
    volatility       REAL NOT NULL,
    updated_at_ms    INTEGER NOT NULL
);

CREATE TABLE games (
    game_id             TEXT PRIMARY KEY,   -- UUIDv4, idempotency key
    room_code           TEXT NOT NULL,
    time_control        TEXT NOT NULL,      -- preset label, e.g. "15+10"
    white_device        TEXT NOT NULL,
    black_device        TEXT NOT NULL,
    status              TEXT NOT NULL,      -- terminal status wire name
    winner              TEXT,               -- 'white' | 'black' | NULL (draw)
    move_count          INTEGER NOT NULL,
    white_rating_before REAL NOT NULL,
    black_rating_before REAL NOT NULL,
    white_rating_after  REAL NOT NULL,
    black_rating_after  REAL NOT NULL,
    ended_at_ms         INTEGER NOT NULL    -- wall clock, unix ms
);
CREATE INDEX games_white_device_idx ON games (white_device, ended_at_ms);
CREATE INDEX games_black_device_idx ON games (black_device, ended_at_ms);

CREATE TABLE rating_history (
    device_id     TEXT NOT NULL,
    game_id       TEXT NOT NULL REFERENCES games (game_id),
    rating_before REAL NOT NULL,
    rating_after  REAL NOT NULL,
    PRIMARY KEY (device_id, game_id)
);
