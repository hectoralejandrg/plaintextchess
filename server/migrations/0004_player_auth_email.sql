-- The account's email identity, its verification state, and the tokens the
-- reset and verification flows use (lichess-like-auth): three additions to
-- the same SQLite file as the ratings and the accounts.
--
-- Nothing in the earlier migrations is altered, so this one is additive:
-- there is no data rewrite and no backfill. The accounts that existed
-- before this migration keep their rows, keep their usernames, and simply
-- have no email address yet. Rolling back means dropping what this
-- migration added; the ratings, the games, and the accounts are untouched by
-- that.

-- The email is the account's private identity: unique across accounts, and
-- never published to another player (spec "Email Identity"). It is NULL for
-- the accounts created before it existed, and NULL rows are not a
-- uniqueness problem for a SQLite unique index.
ALTER TABLE accounts ADD COLUMN email TEXT;
-- The case-folded address is the uniqueness key, so two addresses that
-- differ only in letter case are one address. The fold is materialized here
-- rather than left to SQLite's NOCASE collation, which folds ASCII only —
-- the same trick the username index uses.
ALTER TABLE accounts ADD COLUMN email_key TEXT;
CREATE UNIQUE INDEX accounts_email_key_idx ON accounts (email_key);
-- Whether the address has been confirmed. The accounts created before this
-- migration are unverified: they have no address to confirm.
ALTER TABLE accounts ADD COLUMN email_verified INTEGER NOT NULL DEFAULT 0;
-- The address the account is moving to, while the current one stays in force
-- (spec "Email Change"). The pending address is reserved the moment it is
-- asked for, so two accounts cannot claim the same one.
ALTER TABLE accounts ADD COLUMN pending_email TEXT;
ALTER TABLE accounts ADD COLUMN pending_email_key TEXT;
-- A pending address is reserved the moment it is asked for, so two accounts
-- cannot claim the same one while their change is pending.
CREATE UNIQUE INDEX accounts_pending_email_key_idx ON accounts (pending_email_key);

-- The token that confirms an address. One row per account: asking again
-- replaces the row, which is what retires the previous token.
CREATE TABLE email_verifications (
    account_id    TEXT PRIMARY KEY REFERENCES accounts (account_id),
    -- The address the token was sent to: a token confirms the address it was
    -- mailed to, and no other (spec "Email Change").
    address       TEXT NOT NULL,
    -- Only the hash of the token is stored. The token itself exists once, in
    -- the message that carries it.
    token_hash    TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    expires_at_ms INTEGER NOT NULL,   -- wall clock unix ms, checked at use
    used_at_ms    INTEGER             -- NULL while the token is live
);
CREATE UNIQUE INDEX email_verifications_token_hash_idx ON email_verifications (token_hash);

-- The token that allows a password to be reset. One row per issued token,
-- so a second request adds a row and the first token still works until it
-- is used or expires.
CREATE TABLE password_reset_tokens (
    token_hash    TEXT PRIMARY KEY,   -- only the hash is stored
    account_id    TEXT NOT NULL REFERENCES accounts (account_id),
    created_at_ms INTEGER NOT NULL,
    expires_at_ms INTEGER NOT NULL,
    used_at_ms    INTEGER             -- NULL while the token is live
);
CREATE INDEX password_reset_tokens_account_idx ON password_reset_tokens (account_id);
