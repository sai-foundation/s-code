ALTER TABLE team_goals ADD COLUMN central_managed INTEGER NOT NULL DEFAULT 0;
ALTER TABLE team_goals ADD COLUMN central_sequence INTEGER;
ALTER TABLE team_tasks ADD COLUMN central_managed INTEGER NOT NULL DEFAULT 0;
ALTER TABLE team_tasks ADD COLUMN central_sequence INTEGER;

CREATE TABLE central_team_work_snapshots (
    organization_id TEXT NOT NULL,
    team_id TEXT NOT NULL,
    sequence INTEGER NOT NULL CHECK(sequence >= 0),
    key_id TEXT NOT NULL,
    envelope_json TEXT NOT NULL,
    issued_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    applied_at TEXT NOT NULL,
    PRIMARY KEY(organization_id, team_id)
);

CREATE INDEX team_goals_central_source
    ON team_goals(organization_id, team_id, central_managed, central_sequence);
CREATE INDEX team_tasks_central_source
    ON team_tasks(organization_id, team_id, central_managed, central_sequence);
