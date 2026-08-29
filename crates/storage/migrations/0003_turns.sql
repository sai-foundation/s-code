CREATE TABLE turns (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    organization_id TEXT NOT NULL,
    team_id TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    goal_id TEXT,
    task_id TEXT,
    status TEXT NOT NULL CHECK (status IN ('idle','preparing_context','calling_model','awaiting_approval','running_tool','completed','failed','cancelled')),
    checkpoint_json TEXT,
    error_code TEXT,
    started_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    completed_at TEXT
);
CREATE INDEX turns_session_started ON turns(session_id, started_at);
CREATE INDEX turns_team_status ON turns(team_id, status, updated_at);
