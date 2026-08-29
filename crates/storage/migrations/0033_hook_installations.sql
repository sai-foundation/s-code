CREATE TABLE hook_installations (
    organization_id TEXT NOT NULL,
    team_id TEXT NOT NULL,
    hook_id TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    spec_json TEXT NOT NULL,
    permissions_sha256 TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    revision INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (organization_id, team_id, hook_id)
);

CREATE INDEX hook_installations_scope
ON hook_installations(organization_id, team_id, enabled, updated_at DESC);
