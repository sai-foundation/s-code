CREATE TABLE remote_grant_revocations (
    grant_id TEXT PRIMARY KEY,
    organization_id TEXT NOT NULL,
    team_id TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    device_id TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    revoked_at TEXT NOT NULL
);

CREATE INDEX remote_grant_revocations_expiry_idx
    ON remote_grant_revocations(expires_at);
