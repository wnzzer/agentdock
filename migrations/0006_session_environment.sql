ALTER TABLE endpoint_profiles ADD COLUMN environment TEXT NOT NULL DEFAULT '{}';
ALTER TABLE sessions ADD COLUMN environment TEXT NOT NULL DEFAULT '{}';
