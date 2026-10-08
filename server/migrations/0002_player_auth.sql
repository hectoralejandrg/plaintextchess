-- Player accounts, device-to-account links, and sessions
-- (add-player-login-sessions D1/D2): three new tables in the same SQLite
-- file as the ratings. Nothing in 0001_initial.sql is altered, so this
-- migration is additive: there is no data rewrite and no backfill, and
-- rolling it back means dropping these three tables (the rating and game
-- history are untouched by that).

CREATE TABLE accounts (
    account_id    TEXT PRIMARY KEY,   -- UUIDv4, server-generated
    username      TEXT NOT NULL,      -- as entered, for display
    username_key  TEXT NOT NULL,      -- case-folded; unique (D3)
    password_hash TEXT NOT NULL,      -- argon2id PHC string, never plaintext
    display_name  TEXT,               -- NULL means "report the username"
    created_at_ms INTEGER NOT NULL
);
-- Uniqueness is case-insensitive, but SQLite's built-in NOCASE collation
-- folds ASCII only, so the fold is materialized in `username_key` instead
-- (D3). This also keeps the rule visible to the eventual Postgres cutover.
CREATE UNIQUE INDEX accounts_username_key_idx ON accounts (username_key);

-- PK on device_id is what makes "a device belongs to at most one account"
-- a database guarantee rather than an application convention. The column
-- is the same client-asserted identifier that `players.device_id` uses for
-- ratings, which is deliberately left keyed by device (D1).
CREATE TABLE profiles (
    device_id     TEXT PRIMARY KEY,
    account_id    TEXT NOT NULL REFERENCES accounts (account_id),
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);
CREATE INDEX profiles_account_idx ON profiles (account_id);

CREATE TABLE sessions (
    session_id    TEXT PRIMARY KEY,   -- UUIDv4, for logs only
    account_id    TEXT NOT NULL REFERENCES accounts (account_id),
    device_id     TEXT NOT NULL,      -- the device that logged in
    -- Only the hash of the token is stored (D6). A token carries 256 bits of
    -- entropy, so a fast hash is the right choice here and there is nothing
    -- to brute-force; the unique index makes the lookup a point read.
    token_hash    TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    expires_at_ms INTEGER NOT NULL,   -- wall clock unix ms, checked at use (D7)
    revoked_at_ms INTEGER             -- NULL while the session is live
);
CREATE UNIQUE INDEX sessions_token_hash_idx ON sessions (token_hash);
CREATE INDEX sessions_account_idx ON sessions (account_id);
