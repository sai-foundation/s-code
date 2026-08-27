CREATE TABLE tool_calls (
    id TEXT PRIMARY KEY,
    organization_id TEXT NOT NULL,
    team_id TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    goal_id TEXT,
    task_id TEXT,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    turn_id TEXT NOT NULL,
    tool TEXT NOT NULL,
    arguments_json TEXT NOT NULL,
    policy_decision TEXT NOT NULL CHECK (policy_decision IN ('allow','ask','deny')),
    policy_id TEXT NOT NULL,
    policy_version TEXT NOT NULL,
    policy_reason TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('proposed','awaiting_approval','running','completed','denied','failed','cancelled')),
    result_json TEXT,
    error TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX tool_calls_session_created ON tool_calls(session_id, created_at);
CREATE INDEX tool_calls_team_status ON tool_calls(team_id, status, updated_at);

CREATE TABLE approvals (
    id TEXT PRIMARY KEY,
    tool_call_id TEXT NOT NULL UNIQUE REFERENCES tool_calls(id),
    organization_id TEXT NOT NULL,
    team_id TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    goal_id TEXT,
    task_id TEXT,
    approval_scope TEXT NOT NULL CHECK (approval_scope IN ('once','session')),
    status TEXT NOT NULL CHECK (status IN ('pending','approved','rejected','expired')),
    requested_at TEXT NOT NULL,
    decided_at TEXT,
    decided_by TEXT
);
CREATE INDEX approvals_team_status ON approvals(team_id, status, requested_at);
