-- A temporary session is declared at creation and stays that way. The flag is
-- never set on an existing session, so a session that was safe to keep can
-- never become one that closing a window destroys.
ALTER TABLE sessions ADD COLUMN ephemeral INTEGER NOT NULL DEFAULT 0;
