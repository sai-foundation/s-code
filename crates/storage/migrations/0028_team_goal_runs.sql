CREATE TABLE team_goal_runs (
    id TEXT PRIMARY KEY NOT NULL,
    organization_id TEXT NOT NULL,
    team_id TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    goal_id TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('active', 'paused', 'cancelled', 'awaiting_verification')),
    workspace_uri TEXT NOT NULL,
    model TEXT NOT NULL,
    max_attempts INTEGER NOT NULL,
    max_runtime_seconds INTEGER NOT NULL,
    max_cost_micros INTEGER NOT NULL,
    current_task_id TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    revision INTEGER NOT NULL DEFAULT 1,
    UNIQUE (organization_id, team_id, actor_id, goal_id, idempotency_key),
    FOREIGN KEY (goal_id) REFERENCES team_goals(id),
    FOREIGN KEY (current_task_id) REFERENCES team_tasks(id)
);

CREATE INDEX idx_team_goal_runs_scope
    ON team_goal_runs (organization_id, team_id, actor_id, goal_id, updated_at DESC);
