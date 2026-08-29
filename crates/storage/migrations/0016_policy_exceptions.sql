CREATE TABLE policy_exception_grants (
    exception_id TEXT PRIMARY KEY,
    organization_id TEXT NOT NULL,
    team_id TEXT NOT NULL,
    sequence INTEGER NOT NULL CHECK(sequence >= 0),
    key_id TEXT NOT NULL,
    envelope_json TEXT NOT NULL,
    active INTEGER NOT NULL,
    tool TEXT NOT NULL,
    actor_id TEXT,
    workspace_uri TEXT,
    reason TEXT NOT NULL,
    requested_by TEXT NOT NULL,
    approved_by TEXT NOT NULL,
    issued_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    applied_at TEXT NOT NULL
);

CREATE INDEX policy_exception_active_match
    ON policy_exception_grants(organization_id, team_id, tool, active, expires_at);
