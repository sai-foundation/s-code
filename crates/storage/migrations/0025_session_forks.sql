CREATE TABLE session_forks (
    session_id TEXT PRIMARY KEY REFERENCES sessions(id),
    parent_session_id TEXT NOT NULL REFERENCES sessions(id),
    source_turn_id TEXT REFERENCES turns(id),
    organization_id TEXT NOT NULL,
    team_id TEXT NOT NULL,
    created_at TEXT NOT NULL,
    CHECK (session_id != parent_session_id)
);

CREATE INDEX session_forks_parent
    ON session_forks(organization_id, team_id, parent_session_id, created_at);
