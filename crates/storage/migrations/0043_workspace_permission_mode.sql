CREATE TABLE session_preferences_next (
    session_id TEXT PRIMARY KEY REFERENCES sessions(id),
    organization_id TEXT NOT NULL,
    team_id TEXT NOT NULL,
    permission_mode TEXT NOT NULL CHECK (
        permission_mode IN ('manual', 'accept_edits', 'workspace', 'plan')
    ),
    source TEXT NOT NULL,
    locked_reason TEXT,
    updated_at TEXT NOT NULL,
    assistant_alias TEXT NOT NULL DEFAULT 'Opencoding'
);

INSERT INTO session_preferences_next (
    session_id,
    organization_id,
    team_id,
    permission_mode,
    source,
    locked_reason,
    updated_at,
    assistant_alias
)
SELECT
    session_id,
    organization_id,
    team_id,
    permission_mode,
    source,
    locked_reason,
    updated_at,
    assistant_alias
FROM session_preferences;

DROP TABLE session_preferences;
ALTER TABLE session_preferences_next RENAME TO session_preferences;

CREATE INDEX session_preferences_team
    ON session_preferences(organization_id, team_id, updated_at DESC);
