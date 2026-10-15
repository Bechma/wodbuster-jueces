CREATE TABLE score_photos (
    id UUID PRIMARY KEY,
    username TEXT NOT NULL,
    competition_id BIGINT NOT NULL,
    session_id UUID NOT NULL,
    image BYTEA NOT NULL CHECK (octet_length(image) BETWEEN 1 AND 1572864),
    media_type TEXT NOT NULL CHECK (media_type IN ('image/jpeg','image/png','image/webp')),
    width INTEGER NOT NULL CHECK (width BETWEEN 1 AND 4096),
    height INTEGER NOT NULL CHECK (height BETWEEN 1 AND 4096),
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'queued' CHECK (state IN
        ('queued','parsing','parsed','submitting','succeeded','incomplete','failed','unconfirmed','saved_local_failed')),
    reading JSONB,
    prepared JSONB,
    validation_errors JSONB NOT NULL DEFAULT '[]',
    result JSONB,
    error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    FOREIGN KEY (username,competition_id) REFERENCES wodbuster_competitions(username,id) ON DELETE CASCADE
);
CREATE INDEX score_photos_gallery ON score_photos(username,competition_id,created_at DESC);
CREATE INDEX score_photos_queue ON score_photos(created_at) WHERE state IN ('queued','parsed');
