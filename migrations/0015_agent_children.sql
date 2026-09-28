-- Sessions an agent started through AgentDock's own tools (agentdock_spawn),
-- each with the session that started it. A session may read and message only
-- the sessions it started, and a started session cannot start more; this is
-- what those rules look up. Kept apart from `sessions` because it describes
-- who asked, not what a session is.
CREATE TABLE IF NOT EXISTS agent_children (
    session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
    parent_session_id TEXT NOT NULL,
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS agent_children_parent ON agent_children(parent_session_id);
