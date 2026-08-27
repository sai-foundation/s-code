CREATE TABLE artifacts (
    id TEXT PRIMARY KEY,
    item_id TEXT NOT NULL UNIQUE,
    organization_id TEXT NOT NULL,
    team_id TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    goal_id TEXT,
    task_id TEXT,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    turn_id TEXT NOT NULL REFERENCES turns(id),
    title TEXT NOT NULL,
    media_type TEXT NOT NULL,
    content_json TEXT NOT NULL,
    byte_length INTEGER NOT NULL CHECK (byte_length >= 0),
    created_at TEXT NOT NULL
);

CREATE INDEX artifacts_session_created
ON artifacts(session_id, created_at);
CREATE INDEX artifacts_team_created
ON artifacts(team_id, created_at);
