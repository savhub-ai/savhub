-- B1 Phase 1: store SHA-256 hash of bearer tokens at rest.
-- Existing rows are backfilled from the plaintext column. The plaintext
-- `token` column is preserved for one release so a rollback is possible;
-- it will be dropped in a follow-up migration after every server is on
-- the new code.

ALTER TABLE user_tokens
    ADD COLUMN token_hash TEXT,
    ADD COLUMN token_prefix TEXT;

-- digest() lives in pgcrypto. Enable it before the first backfill on
-- databases that don't already have the extension.
CREATE EXTENSION IF NOT EXISTS pgcrypto;

-- Backfill: SHA-256 hex of the existing plaintext token, plus a short
-- non-secret prefix for UI display.
UPDATE user_tokens
SET
    token_hash = encode(digest(token, 'sha256'), 'hex'),
    token_prefix = substr(token, 1, 12)
WHERE token_hash IS NULL;

ALTER TABLE user_tokens
    ALTER COLUMN token_hash SET NOT NULL,
    ALTER COLUMN token_prefix SET NOT NULL;

CREATE UNIQUE INDEX user_tokens_token_hash_key ON user_tokens (token_hash);
