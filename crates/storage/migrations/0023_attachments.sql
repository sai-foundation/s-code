CREATE TABLE attachments (
    id TEXT PRIMARY KEY,
    organization_id TEXT NOT NULL,
    team_id TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    turn_id TEXT REFERENCES turns(id),
    file_name TEXT NOT NULL,
    media_type TEXT NOT NULL,
    content_base64 TEXT NOT NULL,
    byte_length INTEGER NOT NULL CHECK (byte_length >= 0),
    sha256 TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE INDEX attachments_session_created
ON attachments(session_id, created_at);

CREATE INDEX attachments_turn_created
ON attachments(turn_id, created_at);

CREATE INDEX attachments_draft_owner
ON attachments(session_id, actor_id, created_at)
WHERE turn_id IS NULL;
