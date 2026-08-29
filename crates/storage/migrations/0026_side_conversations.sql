CREATE TABLE side_conversations (
    id TEXT PRIMARY KEY NOT NULL,
    organization_id TEXT NOT NULL,
    team_id TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    goal_id TEXT,
    task_id TEXT,
    source_session_id TEXT NOT NULL,
    session_id TEXT NOT NULL UNIQUE,
    source_turn_id TEXT,
    status TEXT NOT NULL CHECK (status IN ('active', 'promoted', 'closed')),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY (source_session_id) REFERENCES sessions(id),
    FOREIGN KEY (session_id) REFERENCES sessions(id),
    FOREIGN KEY (source_turn_id) REFERENCES turns(id)
);

CREATE INDEX idx_side_conversations_scope
ON side_conversations (organization_id, team_id, updated_at DESC);

CREATE INDEX idx_side_conversations_source
ON side_conversations (source_session_id, status, created_at);
