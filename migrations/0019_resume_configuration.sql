-- A terminal continuation owns a fixed conversation and configuration home.
-- Older terminals followed the current source revision; pin that value once.
ALTER TABLE sessions ADD COLUMN resume_configuration_revision INTEGER;
UPDATE sessions SET resume_configuration_revision = (
    SELECT source.configuration_revision FROM sessions AS source
    WHERE source.id = sessions.resume_source_id
) WHERE resume_source_id IS NOT NULL;

-- Imported conversations are unique; terminal views of them are not owners.
DROP INDEX sessions_native_identity;
CREATE UNIQUE INDEX sessions_native_identity
ON sessions(workspace_id, native_source_id, provider_session_id)
WHERE native_source_id IS NOT NULL AND resume_source_id IS NULL;
