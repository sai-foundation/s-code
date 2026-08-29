ALTER TABLE durable_tasks ADD COLUMN max_runner_cost_micros INTEGER NOT NULL DEFAULT 0;
ALTER TABLE durable_tasks ADD COLUMN consumed_runner_cost_micros INTEGER NOT NULL DEFAULT 0;
ALTER TABLE team_budget_reservations ADD COLUMN reserved_runner_remaining_micros INTEGER NOT NULL DEFAULT 0;
ALTER TABLE team_budget_reservations ADD COLUMN settled_runner_micros INTEGER NOT NULL DEFAULT 0;
