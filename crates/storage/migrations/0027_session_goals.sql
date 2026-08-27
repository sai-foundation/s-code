CREATE TABLE session_goals (
    id TEXT PRIMARY KEY NOT NULL,
    session_id TEXT NOT NULL UNIQUE,
    organization_id TEXT NOT NULL,
    team_id TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    objective TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('active', 'paused', 'completed', 'blocked')),
    auto_continue INTEGER NOT NULL DEFAULT 1,
    token_budget INTEGER,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    continuation_count INTEGER NOT NULL DEFAULT 0,
    last_turn_id TEXT,
    blocked_reason TEXT,
    revision INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    completed_at TEXT,
    FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE,
    FOREIGN KEY (last_turn_id) REFERENCES turns(id)
);

CREATE INDEX idx_session_goals_scope_status
ON session_goals (organization_id, team_id, actor_id, status, updated_at DESC);
