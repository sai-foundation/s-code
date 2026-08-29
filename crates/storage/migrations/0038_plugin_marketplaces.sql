CREATE TABLE marketplace_installations (
    organization_id TEXT NOT NULL,
    team_id TEXT NOT NULL,
    marketplace_name TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    source_json TEXT NOT NULL,
    manifest_sha256 TEXT NOT NULL,
    revision INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (organization_id, team_id, marketplace_name)
);

CREATE INDEX idx_marketplace_installations_scope
    ON marketplace_installations (organization_id, team_id, updated_at DESC);

CREATE TABLE plugin_installations (
    organization_id TEXT NOT NULL,
    team_id TEXT NOT NULL,
    plugin_id TEXT NOT NULL,
    marketplace_name TEXT NOT NULL,
    plugin_name TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    bundle_json TEXT NOT NULL,
    permissions_sha256 TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    revision INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (organization_id, team_id, plugin_id),
    FOREIGN KEY (organization_id, team_id, marketplace_name)
        REFERENCES marketplace_installations (organization_id, team_id, marketplace_name)
        ON DELETE RESTRICT
);

CREATE INDEX idx_plugin_installations_scope
    ON plugin_installations (organization_id, team_id, enabled, updated_at DESC);
