ALTER TABLE sessions ADD COLUMN interaction_mode TEXT NOT NULL DEFAULT 'pty' CHECK(interaction_mode IN ('pty','structured'));
ALTER TABLE sessions ADD COLUMN configuration_revision INTEGER NOT NULL DEFAULT 0;
CREATE TABLE conversation_meta (
    session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
    next_seq INTEGER NOT NULL DEFAULT 1,
    truncated INTEGER NOT NULL DEFAULT 0,
    anchors TEXT NOT NULL DEFAULT '{}'
);
CREATE TABLE conversation_events (
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    seq INTEGER NOT NULL,
    event_json TEXT NOT NULL,
    byte_count INTEGER NOT NULL,
    PRIMARY KEY(session_id,seq)
);
CREATE TABLE conversation_submissions (
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    message_id TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    PRIMARY KEY(session_id,message_id)
);
