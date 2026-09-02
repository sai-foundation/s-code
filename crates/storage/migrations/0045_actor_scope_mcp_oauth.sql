CREATE TABLE mcp_oauth_credentials_actor_scoped (
    organization_id TEXT NOT NULL,
    team_id TEXT NOT NULL,
    server_id TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    credential_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (organization_id, team_id, actor_id, server_id)
);

INSERT INTO mcp_oauth_credentials_actor_scoped
    (organization_id, team_id, server_id, actor_id, credential_json, created_at, updated_at)
SELECT organization_id, team_id, server_id, actor_id, credential_json, created_at, updated_at
FROM mcp_oauth_credentials;

DROP TABLE mcp_oauth_credentials;
ALTER TABLE mcp_oauth_credentials_actor_scoped RENAME TO mcp_oauth_credentials;

CREATE INDEX idx_mcp_oauth_credentials_scope
    ON mcp_oauth_credentials (organization_id, team_id, actor_id, updated_at DESC);
