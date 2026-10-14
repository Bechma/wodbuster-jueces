ALTER TABLE wodbuster_competitions ADD COLUMN judge_url TEXT;
ALTER TABLE wodbuster_competitions ADD COLUMN admin_url TEXT;

CREATE TABLE competition_details (
    username TEXT NOT NULL,
    competition_id BIGINT NOT NULL,
    snapshot JSONB NOT NULL,
    loaded_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (username, competition_id),
    FOREIGN KEY (username, competition_id) REFERENCES wodbuster_competitions (username, id) ON DELETE CASCADE
);
CREATE TABLE competition_categories (
    username TEXT NOT NULL,
    competition_id BIGINT NOT NULL,
    id BIGINT NOT NULL,
    name TEXT NOT NULL,
    metadata JSONB NOT NULL,
    PRIMARY KEY (username, competition_id, id),
    FOREIGN KEY (username, competition_id) REFERENCES competition_details ON DELETE CASCADE
);
CREATE TABLE competition_teams (
    username TEXT NOT NULL,
    competition_id BIGINT NOT NULL,
    category_id BIGINT NOT NULL,
    reference TEXT NOT NULL,
    registration_id BIGINT,
    gid TEXT,
    name TEXT NOT NULL,
    data JSONB NOT NULL,
    PRIMARY KEY (username, competition_id, category_id, reference),
    FOREIGN KEY (username, competition_id, category_id) REFERENCES competition_categories ON DELETE CASCADE
);
CREATE TABLE competition_wods (
    username TEXT NOT NULL,
    competition_id BIGINT NOT NULL,
    category_id BIGINT NOT NULL,
    id BIGINT NOT NULL,
    name TEXT NOT NULL,
    data JSONB NOT NULL,
    PRIMARY KEY (username, competition_id, category_id, id),
    FOREIGN KEY (username, competition_id, category_id) REFERENCES competition_categories ON DELETE CASCADE
);
CREATE TABLE competition_scores (
    username TEXT NOT NULL,
    competition_id BIGINT NOT NULL,
    category_id BIGINT NOT NULL,
    team_reference TEXT NOT NULL,
    wod_id BIGINT NOT NULL,
    result TEXT,
    rank TEXT,
    points TEXT,
    data JSONB NOT NULL,
    PRIMARY KEY (username, competition_id, category_id, team_reference, wod_id),
    FOREIGN KEY (username, competition_id, category_id, team_reference) REFERENCES competition_teams ON DELETE CASCADE,
    FOREIGN KEY (username, competition_id, category_id, wod_id) REFERENCES competition_wods ON DELETE CASCADE
);
