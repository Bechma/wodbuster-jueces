CREATE TABLE wodbuster_competitions (
    username TEXT NOT NULL,
    id BIGINT NOT NULL CHECK (id > 0),
    name TEXT NOT NULL,
    PRIMARY KEY (username, id)
);
