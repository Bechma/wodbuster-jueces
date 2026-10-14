CREATE TABLE wodbuster_sessions (
    id UUID PRIMARY KEY,
    username TEXT NOT NULL,
    -- AES-256-GCM: nonce de 12 bytes seguido del JSON cifrado y su tag.
    auth_cookie BYTEA NOT NULL,
    saved_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
