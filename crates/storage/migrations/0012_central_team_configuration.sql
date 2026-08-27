CREATE TABLE central_team_configurations (
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
