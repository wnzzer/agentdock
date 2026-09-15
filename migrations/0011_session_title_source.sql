-- Where a session's title came from, so an automatic one can be replaced by a
-- better automatic one while a name the user typed never is.
--
-- Existing sessions are marked 'manual'. Their titles are whatever they have
-- been showing, and silently rewriting a name someone may have chosen is worse
-- than leaving the generic ones that prompted this.
ALTER TABLE sessions ADD COLUMN title_source TEXT NOT NULL DEFAULT 'manual'
  CHECK(title_source IN ('auto','derived','manual'));
