ALTER TABLE endpoint_profiles ADD COLUMN proxy_url TEXT;
ALTER TABLE endpoint_profiles ADD COLUMN model_aliases TEXT NOT NULL DEFAULT '{}';
