CREATE TABLE skill_installations (
    organization_id TEXT NOT NULL,
    team_id TEXT NOT NULL,
    skill_id TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    spec_json TEXT NOT NULL,
    instructions TEXT NOT NULL,
    content_sha256 TEXT NOT NULL,
    permissions_sha256 TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    revision INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (organization_id, team_id, skill_id)
);

CREATE INDEX skill_installations_scope
ON skill_installations(organization_id, team_id, enabled, updated_at DESC);
