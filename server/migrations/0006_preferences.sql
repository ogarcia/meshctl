-- What is set from the interface and kept for every browser that opens it,
-- one group of settings to a row, as JSON: the interface's look and language
-- under 'interface' today.
CREATE TABLE preference (
    key TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
);
