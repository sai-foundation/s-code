CREATE TABLE mcp_oauth_credentials (
    organization_id TEXT NOT NULL,
    team_id TEXT NOT NULL,
    server_id TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    credential_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (organization_id, team_id, server_id)
);

CREATE INDEX idx_mcp_oauth_credentials_scope
    ON mcp_oauth_credentials (organization_id, team_id, updated_at DESC);

CREATE TABLE mcp_oauth_pending (
    state TEXT PRIMARY KEY,
    organization_id TEXT NOT NULL,
    team_id TEXT NOT NULL,
    server_id TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    pending_json TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE INDEX idx_mcp_oauth_pending_expiry
    ON mcp_oauth_pending (expires_at);
