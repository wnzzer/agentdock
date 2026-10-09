-- The endpoint models a profile offers in a session's menu, as a JSON array of
-- IDs. Empty offers every model the endpoint serves.
ALTER TABLE endpoint_profiles ADD COLUMN models TEXT NOT NULL DEFAULT '[]';
