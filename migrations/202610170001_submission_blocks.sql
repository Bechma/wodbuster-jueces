-- Independiente del detalle: refrescar Arena no elimina los bloqueos locales.
CREATE TABLE score_submission_blocks (
    username TEXT NOT NULL,
    competition_id BIGINT NOT NULL,
    category_id BIGINT NOT NULL DEFAULT 0 CHECK (category_id >= 0),
    wod_id BIGINT NOT NULL DEFAULT 0 CHECK (wod_id >= 0),
    CHECK (category_id > 0 OR wod_id > 0),
    PRIMARY KEY (username, competition_id, category_id, wod_id),
    FOREIGN KEY (username, competition_id) REFERENCES wodbuster_competitions(username, id) ON DELETE CASCADE
);
