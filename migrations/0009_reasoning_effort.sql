-- Reasoning effort is a per-profile default. It is only ever applied when the
-- client reports that the selected model supports that level.
ALTER TABLE endpoint_profiles ADD COLUMN effort TEXT;
