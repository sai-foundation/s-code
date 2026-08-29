CREATE TABLE session_goal_checkpoints (
    turn_id TEXT PRIMARY KEY NOT NULL,
    session_id TEXT NOT NULL,
    organization_id TEXT NOT NULL,
    team_id TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    before_json TEXT,
    after_sha256 TEXT,
    state TEXT NOT NULL CHECK (state IN ('pending','active','undone')),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    undone_at TEXT,
    FOREIGN KEY (turn_id) REFERENCES turns(id) ON DELETE CASCADE,
    FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
);

CREATE INDEX session_goal_checkpoints_session_state
ON session_goal_checkpoints(session_id, state, updated_at);
