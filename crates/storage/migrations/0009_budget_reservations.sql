ALTER TABLE team_budgets ADD COLUMN model_reserved_micros INTEGER NOT NULL DEFAULT 0;
ALTER TABLE team_budgets ADD COLUMN runner_reserved_micros INTEGER NOT NULL DEFAULT 0;

CREATE TABLE team_budget_reservations (
    durable_task_id TEXT PRIMARY KEY REFERENCES durable_tasks(id),
    budget_id TEXT NOT NULL REFERENCES team_budgets(id),
    reserved_remaining_micros INTEGER NOT NULL CHECK(reserved_remaining_micros >= 0),
    settled_micros INTEGER NOT NULL DEFAULT 0 CHECK(settled_micros >= 0),
    status TEXT NOT NULL CHECK(status IN ('active','settled','released')),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX team_budget_reservations_budget_status
    ON team_budget_reservations(budget_id, status);
