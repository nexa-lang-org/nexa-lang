-- Refresh tokens for stateless re-authentication (S01 fix).
-- Only the SHA-256 hash of the raw token value is stored.
-- Tokens expire after 30 days; expired rows can be pruned via delete_expired().

CREATE TABLE refresh_tokens (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id     UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token_hash  TEXT        NOT NULL UNIQUE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at  TIMESTAMPTZ NOT NULL
);

CREATE INDEX refresh_tokens_hash_idx ON refresh_tokens(token_hash);
CREATE INDEX refresh_tokens_expires_idx ON refresh_tokens(expires_at);
