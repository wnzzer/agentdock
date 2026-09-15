-- The shared canvas is independent of project-owned legacy layouts. Do not
-- copy, merge, replace, or delete any layouts/workspaces/sessions/profiles here.
CREATE TABLE shared_canvas (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    layout_json TEXT,
    revision INTEGER NOT NULL DEFAULT 0 CHECK (revision >= 0 AND typeof(revision) = 'integer'),
    updated_at TEXT NOT NULL
);

INSERT INTO shared_canvas (singleton, layout_json, revision, updated_at)
VALUES (1, NULL, 0, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));
