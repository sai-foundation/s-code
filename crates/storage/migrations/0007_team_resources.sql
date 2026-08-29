CREATE TABLE team_ownership (
    id TEXT PRIMARY KEY,
    organization_id TEXT NOT NULL,
    team_id TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    resource_type TEXT NOT NULL CHECK(resource_type IN ('repository','service','environment')),
    resource_uri TEXT NOT NULL,
    service_tier TEXT,
    on_call TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(organization_id, resource_type, resource_uri)
);
CREATE INDEX team_ownership_team_type ON team_ownership(team_id, resource_type);

CREATE TABLE team_capacity (
    organization_id TEXT NOT NULL,
    team_id TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    human_available_hours REAL NOT NULL CHECK(human_available_hours >= 0),
    agent_concurrency INTEGER NOT NULL CHECK(agent_concurrency >= 0),
    wip_limit INTEGER NOT NULL CHECK(wip_limit > 0),
    updated_at TEXT NOT NULL,
    PRIMARY KEY(organization_id, team_id)
);

CREATE TABLE team_budgets (
    id TEXT PRIMARY KEY,
    organization_id TEXT NOT NULL,
    team_id TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    period_start TEXT NOT NULL,
    period_end TEXT NOT NULL,
    model_limit_micros INTEGER NOT NULL,
    runner_limit_micros INTEGER NOT NULL,
    model_consumed_micros INTEGER NOT NULL DEFAULT 0,
    runner_consumed_micros INTEGER NOT NULL DEFAULT 0,
    hard_limit INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(team_id, period_start, period_end)
);
CREATE INDEX team_budgets_active ON team_budgets(team_id, period_start, period_end);

CREATE TABLE team_budget_consumptions (
    budget_id TEXT NOT NULL REFERENCES team_budgets(id),
    idempotency_key TEXT NOT NULL,
    model_micros INTEGER NOT NULL,
    runner_micros INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY(budget_id, idempotency_key)
);
