-- Whether one session gets AgentDock's own agent tools, overriding the
-- preference for all sessions. No row means the session follows the
-- preference; a row says on (1) or off (0). Applied at the session's next
-- launch, when the tools are injected.
CREATE TABLE IF NOT EXISTS session_agent_tools (
    session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1))
);
