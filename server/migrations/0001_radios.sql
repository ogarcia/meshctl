-- The radios meshctl keeps connected to. Kinds that this build cannot talk to
-- yet are refused by the API, not here, so that adding support for one never
-- needs a migration.
CREATE TABLE radio (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    kind TEXT NOT NULL,
    address TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL
);
