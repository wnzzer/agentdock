ALTER TABLE endpoint_profiles ADD COLUMN native_source_id TEXT;
ALTER TABLE endpoint_profiles ADD COLUMN native_config_dir TEXT;
ALTER TABLE endpoint_profiles ADD COLUMN native_config_env TEXT;

-- Native profiles are references to client-owned configuration, not copies of
-- credentials. A repeat import reopens the same profile for this exact source.
CREATE UNIQUE INDEX endpoint_profiles_native_identity
ON endpoint_profiles(provider, native_source_id, native_config_dir, COALESCE(native_config_env, ''))
WHERE native_source_id IS NOT NULL AND native_config_dir IS NOT NULL;
