CREATE TABLE file_protection (
    organization_id TEXT NOT NULL,
    team_id TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision > 0),
    changed_at TEXT NOT NULL,
    rules_json TEXT NOT NULL,
    PRIMARY KEY (organization_id, team_id, actor_id)
);
