CREATE TABLE session_preferences (
    session_id TEXT PRIMARY KEY REFERENCES sessions(id),
    organization_id TEXT NOT NULL,
    team_id TEXT NOT NULL,
    permission_mode TEXT NOT NULL CHECK (permission_mode IN ('manual', 'accept_edits', 'plan')),
    source TEXT NOT NULL,
    locked_reason TEXT,
    updated_at TEXT NOT NULL
);

CREATE INDEX session_preferences_team
    ON session_preferences(organization_id, team_id, updated_at DESC);
