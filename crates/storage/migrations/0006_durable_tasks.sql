CREATE TABLE durable_tasks (
    id TEXT PRIMARY KEY,
    organization_id TEXT NOT NULL,
    team_id TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    goal_id TEXT,
    task_id TEXT,
    kind TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('queued','leased','running','paused','succeeded','failed','cancelled')),
    attempt INTEGER NOT NULL DEFAULT 0,
    max_attempts INTEGER NOT NULL,
    max_runtime_seconds INTEGER NOT NULL,
    max_cost_micros INTEGER NOT NULL,
    consumed_cost_micros INTEGER NOT NULL DEFAULT 0,
    lease_owner TEXT,
    lease_token TEXT,
    lease_expires_at TEXT,
    checkpoint_json TEXT,
    result_json TEXT,
    error TEXT,
    cancel_requested INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    started_at TEXT,
    updated_at TEXT NOT NULL,
    completed_at TEXT,
    UNIQUE(organization_id, team_id, idempotency_key)
);
CREATE INDEX durable_tasks_claim ON durable_tasks(status, lease_expires_at, created_at);
CREATE INDEX durable_tasks_team_updated ON durable_tasks(team_id, updated_at DESC);

CREATE TABLE durable_task_controls (
    organization_id TEXT NOT NULL,
    team_id TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    updated_at TEXT NOT NULL,
    updated_by TEXT NOT NULL,
    PRIMARY KEY(organization_id, team_id)
);
