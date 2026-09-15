-- Archiving only organizes lists. All runtime, resume and canvas bindings remain.
ALTER TABLE sessions ADD COLUMN archived_at TEXT;
