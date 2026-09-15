ALTER TABLE sessions ADD COLUMN native_source_id TEXT;
ALTER TABLE sessions ADD COLUMN native_config_dir TEXT;
CREATE UNIQUE INDEX IF NOT EXISTS sessions_native_identity ON sessions(workspace_id, native_source_id, provider_session_id) WHERE native_source_id IS NOT NULL;
