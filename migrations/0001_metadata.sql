CREATE TABLE IF NOT EXISTS workspaces (id TEXT PRIMARY KEY, name TEXT NOT NULL, root_path TEXT NOT NULL, created_at TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS sessions (id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL REFERENCES workspaces(id), provider TEXT NOT NULL, title TEXT NOT NULL, status TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, endpoint_profile_id TEXT, provider_session_id TEXT, error TEXT, endpoint_snapshot TEXT);
CREATE TABLE IF NOT EXISTS layouts (workspace_id TEXT PRIMARY KEY REFERENCES workspaces(id), layout_json TEXT NOT NULL, updated_at TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS endpoint_profiles (id TEXT PRIMARY KEY, name TEXT NOT NULL, provider TEXT NOT NULL, endpoint_url TEXT, model TEXT, permission_mode TEXT NOT NULL, secret_ref TEXT, created_at TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS sessions_workspace_idx ON sessions(workspace_id);
