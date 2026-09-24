-- What a new session starts with unless it is told otherwise: per provider,
-- the endpoint profile, thinking depth and permission mode. One JSON document,
-- validated by the server; stale references (a deleted profile) are ignored
-- where they are used rather than rewritten here.
CREATE TABLE IF NOT EXISTS preferences (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    value TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
