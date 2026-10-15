CREATE TABLE background_loads (
    id UUID PRIMARY KEY,
    username TEXT NOT NULL,
    competition_id BIGINT CHECK (competition_id > 0),
    state TEXT NOT NULL CHECK (state IN ('running', 'succeeded', 'failed')),
    error_code TEXT,
    started_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    heartbeat_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at TIMESTAMPTZ
);
CREATE UNIQUE INDEX background_loads_one_running
    ON background_loads (username, COALESCE(competition_id, 0)) WHERE state = 'running';
